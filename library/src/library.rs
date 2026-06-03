use std::{
    error::Error,
    fmt::Display,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use isolang::Language;
use itertools::Itertools;
use log::{info, trace};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use crate::{
    book::{
        book_metadata::BookMetadata, translation_import, translation_metadata::TranslationMetadata,
    },
    cache::WeakLruCache,
    card::{Card, extract_card_updates},
    epub_importer::EpubBook,
    library::{
        file_watcher::LibraryFileChange,
        library_book::{LibraryBook, load_book_user_state},
        library_card::LibraryCardStore,
    },
    tla_trace::mutex::TracedMutex,
};

pub mod file_watcher;
pub mod library_book;
pub mod library_card;

/// Default number of books to pin in the warm LRU. Books accessed beyond this
/// count are still reachable via the weak index while any holder keeps them
/// alive; once the last holder drops, they unload.
pub const DEFAULT_BOOKS_CACHE_CAPACITY: usize = 8;

#[derive(Debug)]
pub enum LibraryError {
    DuplicateTitle(String),
}

impl Display for LibraryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LibraryError::DuplicateTitle(title) => {
                write!(f, "Failed to create book: duplicate title ({title})")
            }
        }
    }
}

impl Error for LibraryError {}

pub struct LibraryTranslationMetadata {
    pub id: Uuid,
    pub source_langugage: String,
    pub target_language: String,
    pub translated_paragraphs_count: usize,
    pub main_path: PathBuf,
    pub conflicting_paths: Vec<PathBuf>,
}

pub struct LibraryBookMetadata {
    pub id: Uuid,
    pub title: String,
    pub main_path: PathBuf,
    pub conflicting_paths: Vec<PathBuf>,
    pub chapters_count: usize,
    pub paragraphs_count: usize,
    pub translations_metadata: Vec<LibraryTranslationMetadata>,
    pub folder_path: Vec<String>,
    /// `chapter_summaries.dat` for this book, if present. `None` for
    /// legacy books that predate the sidecar; the summary generation queue
    /// creates one on first enqueue.
    pub chapter_summaries_main_path: Option<PathBuf>,
    /// Sibling `chapter_summaries~*.dat` files left behind by an
    /// interrupted save. Merged into the main file at load time.
    pub chapter_summaries_conflicting_paths: Vec<PathBuf>,
}

impl LibraryBookMetadata {
    pub async fn load(path: &Path) -> anyhow::Result<Self> {
        let book_dat = path.join("book.dat");

        let book_metadata = {
            let mut file = tokio::fs::File::open(&book_dat).await?;
            let mut buffer = vec![0u8; 65536];
            let n = file.read(&mut buffer).await?;
            buffer.truncate(n);
            let mut cursor = std::io::Cursor::new(buffer);
            BookMetadata::read_metadata(&mut cursor)?
        };

        let conflicting_paths = {
            let mut conflicting_paths = Vec::new();
            let mut read_dir = tokio::fs::read_dir(path).await?;

            while let Some(entry) = read_dir.next_entry().await? {
                let p = entry.path();
                if let Some(name) = p.file_name().and_then(|n| n.to_str())
                    && name.starts_with("book")
                    && name.ends_with(".dat")
                    && name != "book.dat"
                {
                    conflicting_paths.push(p);
                }
            }

            let mut result = Vec::new();

            for path in conflicting_paths {
                let metadata = {
                    let mut file = tokio::fs::File::open(&path).await?;
                    let mut buffer = vec![0u8; 65536];
                    let n = file.read(&mut buffer).await?;
                    buffer.truncate(n);
                    let mut cursor = std::io::Cursor::new(buffer);
                    BookMetadata::read_metadata(&mut cursor)
                };

                match metadata {
                    Ok(metadata) => {
                        if metadata.id == book_metadata.id {
                            result.push(path);
                        } else {
                            println!(
                                "Conflicting version ({:?}) with different book id, skipping...",
                                path
                            );
                        }
                    }
                    Err(err) => {
                        println!("Failed to read metadata from {:?}, skipping: {}", path, err);
                    }
                }
            }

            result
        };

        let mut all_translations = Vec::new();

        let mut read_dir = tokio::fs::read_dir(path).await?;

        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            if path.is_file()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
                && name.starts_with("translation_")
                && name.ends_with(".dat")
            {
                let metadata = {
                    let mut file = tokio::fs::File::open(&path).await?;
                    let mut buffer = vec![0u8; 65536];
                    let n = file.read(&mut buffer).await?;
                    buffer.truncate(n);
                    let mut cursor = std::io::Cursor::new(buffer);
                    TranslationMetadata::read_metadata(&mut cursor)?
                };
                all_translations.push((path, metadata));
            }
        }

        let grouped_translations = all_translations
            .into_iter()
            .chunk_by(|(_, translation)| translation.id);
        let grouped_translations = grouped_translations
            .into_iter()
            .map(|(id, chunk)| (id, chunk.sorted_by_key(|(p, _)| p.as_os_str().len())));

        let mut translations_metadata = Vec::new();

        for (_, mut translations) in grouped_translations {
            let (main_path, main_translation) = translations.next().unwrap(); // There is always at least one translation in chunk

            let conflicting_iterations = translations.map(|(p, _)| p).collect();

            translations_metadata.push(LibraryTranslationMetadata {
                id: main_translation.id,
                source_langugage: main_translation.source_language,
                target_language: main_translation.target_language,
                translated_paragraphs_count: main_translation.translated_paragraphs_count,
                main_path,
                conflicting_paths: conflicting_iterations,
            })
        }

        let folder_path = match load_book_user_state(path).await {
            Ok(state) => state.folder_path,
            Err(err) => {
                println!(
                    "Failed to load state for {:?}, continuing with empty folder path: {}",
                    path, err
                );
                Vec::new()
            }
        };

        // Discover chapter_summaries.dat (main) plus any crash-conflict
        // siblings (chapter_summaries~*.dat). Mirrors the book / translation
        // conflict-file discovery above.
        let chapter_summaries_path = path.join("chapter_summaries.dat");
        let chapter_summaries_main_path = if tokio::fs::try_exists(&chapter_summaries_path).await? {
            Some(chapter_summaries_path)
        } else {
            None
        };
        let mut chapter_summaries_conflicting_paths = Vec::new();
        let mut summaries_dir = tokio::fs::read_dir(path).await?;
        while let Some(entry) = summaries_dir.next_entry().await? {
            let p = entry.path();
            if let Some(name) = p.file_name().and_then(|n| n.to_str())
                && name.starts_with("chapter_summaries~")
                && name.ends_with(".dat")
            {
                chapter_summaries_conflicting_paths.push(p);
            }
        }

        info!("Loaded metadata for {path:?}");
        Ok(LibraryBookMetadata {
            id: book_metadata.id,
            title: book_metadata.title,
            main_path: book_dat,
            conflicting_paths,
            chapters_count: book_metadata.chapters_count,
            paragraphs_count: book_metadata.paragraphs_count,
            translations_metadata,
            folder_path,
            chapter_summaries_main_path,
            chapter_summaries_conflicting_paths,
        })
    }
}

pub struct Library {
    library_root: PathBuf,
    pub(crate) books_cache: WeakLruCache<Uuid, TracedMutex<LibraryBook>>,
    card_store: Arc<LibraryCardStore>,
}

impl Library {
    pub async fn open(library_root: PathBuf) -> anyhow::Result<Self> {
        Self::open_with_capacity(library_root, DEFAULT_BOOKS_CACHE_CAPACITY).await
    }

    pub async fn open_with_capacity(
        library_root: PathBuf,
        cache_capacity: usize,
    ) -> anyhow::Result<Self> {
        if !tokio::fs::try_exists(&library_root).await? {
            tokio::fs::create_dir_all(&library_root).await?;
        }

        let card_store = Arc::new(LibraryCardStore::new(&library_root));

        Ok(Library {
            library_root,
            books_cache: WeakLruCache::new(cache_capacity),
            card_store,
        })
    }

    pub fn card_store(&self) -> &Arc<LibraryCardStore> {
        &self.card_store
    }

    pub async fn apply_paragraph_to_cards(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
        paragraph: &translation_import::ParagraphTranslation,
        target_language: Language,
    ) -> anyhow::Result<()> {
        let book_arc = self.get_book(&book_id).await?;
        let (source_language, chapter_index) = {
            let book = book_arc.lock().await;
            let source_language = Language::from_639_3(&book.book.language).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown source language code on book {book_id}: {}",
                    book.book.language
                )
            })?;
            let chapter_index = book
                .book
                .chapter_views()
                .find(|ch| {
                    (0..ch.paragraph_count()).any(|i| ch.paragraph_view(i).id == paragraph_id)
                })
                .map(|ch| ch.idx)
                .ok_or_else(|| {
                    anyhow::anyhow!("paragraph {paragraph_id} is not attached to any chapter")
                })?;
            (source_language, chapter_index)
        };

        let updates = extract_card_updates(
            paragraph,
            source_language,
            target_language,
            book_id,
            chapter_index,
            paragraph_id,
        );

        for update in updates {
            let id = update.key.id();
            let lock = self.card_store.lock_for(&id).await;
            let _guard = lock.lock().await;

            let result = async {
                let existing = self
                    .card_store
                    .load(
                        &update.key.source_language,
                        &update.key.target_language,
                        &update.key.slug,
                    )
                    .await?;
                let card = match existing {
                    Some(mut card) => {
                        card.apply_update(&update);
                        card
                    }
                    None => Card::new_from_update(&update),
                };
                self.card_store
                    .save(
                        &card,
                        &update.key.source_language,
                        &update.key.target_language,
                    )
                    .await?;
                anyhow::Ok(())
            }
            .await;

            if let Err(err) = result {
                log::warn!("Failed to persist card {id}: {err}");
            }
        }

        Ok(())
    }

    pub async fn list_books(&self) -> anyhow::Result<Vec<LibraryBookMetadata>> {
        let mut library_root_content = tokio::fs::read_dir(&self.library_root).await?;

        let mut books = Vec::new();

        while let Some(entry) = library_root_content.next_entry().await? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let book = LibraryBookMetadata::load(&path).await;
            match book {
                Ok(book) => books.push(book),
                Err(err) => {
                    println!("Failed to load book at path {:?}: error {}", path, err)
                } // TODO logging
            };
        }

        Ok(books)
    }

    pub async fn get_book(&self, uuid: &Uuid) -> anyhow::Result<Arc<TracedMutex<LibraryBook>>> {
        if let Some(book) = self.books_cache.get(uuid).await {
            return Ok(book);
        }

        let path = self.library_root.join(uuid.to_string());
        let metadata = LibraryBookMetadata::load(&path).await?;
        let book = Arc::new(TracedMutex::new(
            LibraryBook::load_from_metadata(metadata).await?,
        ));

        Ok(self.books_cache.insert(*uuid, book).await)
    }

    pub async fn create_book_plain(
        &self,
        title: &str,
        text: &str,
        language: &Language,
    ) -> anyhow::Result<Uuid> {
        let book = self.create_book(title, language).await?;
        let mut book = book.lock().await;
        let chapter_index = book.book.push_chapter(None);
        let paragraphs = split_paragraphs(text);

        for paragraph in paragraphs {
            book.book.push_paragraph(chapter_index, paragraph, None);
        }

        book.save().await?;

        Ok(book.book.id)
    }

    pub async fn create_book_epub(
        &self,
        epub: &EpubBook,
        language: &Language,
    ) -> anyhow::Result<Uuid> {
        let book = self.create_book(&epub.title, language).await?;
        let mut book = book.lock().await;

        for ch in &epub.chapters {
            let ch_idx = book.book.push_chapter(Some(&ch.title));
            for p in &ch.paragraphs {
                book.book.push_paragraph(ch_idx, &p.text, Some(&p.html));
            }
        }

        book.save().await?;

        Ok(book.book.id)
    }

    pub async fn backfill_cards_from_translations(&self) -> anyhow::Result<()> {
        let books = self.list_books().await?;
        info!("Card backfill starting: {} book(s)", books.len());

        for book_meta in books {
            let book_arc = match self.get_book(&book_meta.id).await {
                Ok(arc) => arc,
                Err(err) => {
                    log::warn!("Backfill: failed to load book {}: {err}", book_meta.id);
                    continue;
                }
            };

            for translation_meta in &book_meta.translations_metadata {
                if let Err(err) = Language::from_str(&translation_meta.source_langugage) {
                    log::warn!(
                        "Backfill: unknown source language {:?} on book {}: {err}",
                        translation_meta.source_langugage,
                        book_meta.id
                    );
                    continue;
                }
                let target_language = match Language::from_str(&translation_meta.target_language) {
                    Ok(lang) => lang,
                    Err(err) => {
                        log::warn!(
                            "Backfill: unknown target language {:?} on book {}: {err}",
                            translation_meta.target_language,
                            book_meta.id
                        );
                        continue;
                    }
                };

                let collected: Vec<(usize, translation_import::ParagraphTranslation)> = {
                    let mut book = book_arc.lock().await;
                    let translation_arc = book.get_or_create_translation(&target_language).await;
                    let translation = translation_arc.lock().await;
                    let mut out = Vec::new();
                    for chapter in book.book.chapter_views() {
                        for paragraph in chapter.paragraphs() {
                            if let Some(view) = translation.paragraph_view(paragraph.id) {
                                out.push((paragraph.id, view.to_import()));
                            }
                        }
                    }
                    out
                };

                for (paragraph_id, paragraph) in collected {
                    if let Err(err) = self
                        .apply_paragraph_to_cards(
                            book_meta.id,
                            paragraph_id,
                            &paragraph,
                            target_language,
                        )
                        .await
                    {
                        log::warn!(
                            "Backfill: failed to apply paragraph {paragraph_id} of book {}: {err}",
                            book_meta.id
                        );
                    }
                }
            }
        }

        info!("Card backfill complete");
        Ok(())
    }

    pub async fn save_all(&self) {
        let books = self.books_cache.live_values().await;
        for book_arc in books {
            let mut book = book_arc.lock().await;
            if book.has_unsaved_changes().await
                && let Err(err) = book.save().await
            {
                log::warn!("Failed to save book on shutdown: {err}");
            }
        }
    }

    pub async fn handle_file_change_event(
        &self,
        event: &LibraryFileChange,
    ) -> anyhow::Result<bool> {
        trace!("Starting file change event handling: {:?}...", event);
        let result = Ok(match event {
            LibraryFileChange::BookChanged { modified, uuid } => {
                let book = self.books_cache.get(uuid).await;
                if let Some(book) = book {
                    book.lock().await.reload_book(*modified).await?
                } else {
                    false
                }
            }
            LibraryFileChange::TranslationChanged {
                modified,
                from,
                to,
                uuid,
            } => {
                let book = self.books_cache.get(uuid).await;
                if let Some(book) = book {
                    book.lock()
                        .await
                        .reload_translations(*modified, *from, *to)
                        .await?
                } else {
                    false
                }
            }
            // Drop the store's cached familiarity for this card so the next
            // read repopulates from disk; the frontend separately refreshes
            // via the cards_updated event emitted by AppState. Returns false:
            // there is no in-memory book/translation state to reload here.
            LibraryFileChange::CardChanged {
                from,
                to,
                lemma_slug,
                ..
            } => {
                self.card_store().invalidate_familiarity(
                    from.to_639_3(),
                    to.to_639_3(),
                    lemma_slug,
                );
                false
            }
        });
        trace!("Finish file change event handling");
        result
    }
}

fn split_paragraphs(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|p| !p.is_empty())
}

#[cfg(test)]
mod library_tests {
    use super::*;
    use crate::book::translation_import;
    use crate::test_utils::TempDir;

    fn full_word(
        original: &str,
        lemma_src: &str,
        lemma_tgt: &str,
        part_of_speech: &str,
        translations: &[&str],
        is_punctuation: bool,
    ) -> translation_import::Word {
        translation_import::Word {
            original: original.into(),
            contextual_translations: translations.iter().map(|s| (*s).into()).collect(),
            note: None,
            is_punctuation,
            grammar: translation_import::Grammar {
                original_initial_form: lemma_src.into(),
                target_initial_form: lemma_tgt.into(),
                part_of_speech: part_of_speech.into(),
                plurality: None,
                person: None,
                tense: None,
                case: None,
                other: None,
            },
        }
    }

    fn paragraph_with(
        full_translation: &str,
        words: Vec<translation_import::Word>,
    ) -> translation_import::ParagraphTranslation {
        translation_import::ParagraphTranslation {
            timestamp: 0,
            total_tokens: None,
            sentences: vec![translation_import::Sentence {
                full_translation: full_translation.into(),
                words,
            }],
        }
    }

    async fn library_with_one_paragraph_book(
        library_path: PathBuf,
        paragraph_text: &str,
    ) -> (Library, Uuid) {
        let library = Library::open(library_path).await.unwrap();
        let book = library
            .create_book("Test Book", &Language::from_639_3("spa").unwrap())
            .await
            .unwrap();
        let book_id = {
            let mut b = book.lock().await;
            b.book.push_chapter(Some("Intro"));
            b.book.push_paragraph(0, paragraph_text, None);
            b.save().await.unwrap();
            b.book.id
        };
        (library, book_id)
    }

    #[tokio::test]
    async fn library_open_newdirectory() {
        let temp_dir = TempDir::new("flts_test");
        let library_path = temp_dir.path.join("test");
        _ = Library::open(library_path.clone()).await.unwrap();

        assert!(library_path.exists());
        assert!(library_path.is_dir());
    }

    #[tokio::test]
    async fn list_books_empty_library() {
        let temp_dir = TempDir::new("flts_test");
        let library_path = temp_dir.path.join("lib");
        let library = Library::open(library_path).await.unwrap();

        let books = library.list_books().await.unwrap();
        assert!(books.is_empty(), "Expected no books, got {:?}", books.len());
    }

    #[tokio::test]
    async fn list_books_multiple_empty_books() {
        let temp_dir = TempDir::new("flts_test");
        let library_path = temp_dir.path.join("lib");
        let library = Library::open(library_path.clone()).await.unwrap();

        let book1 = library
            .create_book("First Book", &Language::from_639_3("eng").unwrap())
            .await
            .unwrap();
        book1.lock().await.save().await.unwrap();
        let book2 = library
            .create_book("Second Book", &Language::from_639_3("eng").unwrap())
            .await
            .unwrap();
        book2.lock().await.save().await.unwrap();

        let mut books = library.list_books().await.unwrap();
        assert_eq!(books.len(), 2);
        books.sort_by(|a, b| a.title.cmp(&b.title));
        assert_eq!(books[0].title, "First Book");
        assert_eq!(books[0].paragraphs_count, 0);
        assert!(books[0].translations_metadata.is_empty());
        assert_eq!(books[1].title, "Second Book");
        assert_eq!(books[1].paragraphs_count, 0);
        assert!(books[1].translations_metadata.is_empty());
    }

    #[tokio::test]
    async fn list_books_includes_folder_path() {
        let temp_dir = TempDir::new("flts_test");
        let library_path = temp_dir.path.join("lib");
        let library = Library::open(library_path.clone()).await.unwrap();

        let book = library
            .create_book("Categorized", &Language::from_639_3("eng").unwrap())
            .await
            .unwrap();
        {
            let mut book = book.lock().await;
            book.save().await.unwrap();
            book.update_folder_path(vec!["Shelf".into(), "Modern".into()])
                .await
                .unwrap();
        }

        let books = library.list_books().await.unwrap();
        assert_eq!(books.len(), 1);
        assert_eq!(
            books[0].folder_path,
            vec!["Shelf".to_string(), "Modern".to_string()]
        );
    }

    #[test]
    fn split_paragraphs_js_equivalence_basic() {
        let input = "Hello\n\n  world  \r\n\nNext line\n";
        let result: Vec<_> = split_paragraphs(input).collect();
        assert_eq!(result, vec!["Hello", "world", "Next line"]);
    }

    #[test]
    fn split_paragraphs_whitespace_only() {
        let input = "  \n\n\t\n\r\n";
        let result: Vec<_> = split_paragraphs(input).collect();
        assert!(result.is_empty());
    }

    async fn make_saved_book(library: &Library, title: &str) -> Uuid {
        let book = library
            .create_book(title, &Language::from_639_3("eng").unwrap())
            .await
            .unwrap();
        let id = {
            let mut guard = book.lock().await;
            guard.save().await.unwrap();
            guard.book.id
        };
        id
    }

    #[tokio::test]
    async fn evicted_uuid_returns_same_arc_while_held() {
        let temp_dir = TempDir::new("flts_test");
        let library = Library::open_with_capacity(temp_dir.path.join("lib"), 2)
            .await
            .unwrap();

        let id = make_saved_book(&library, "Pinned").await;
        let held = library.get_book(&id).await.unwrap();

        for i in 0..4 {
            let _ = make_saved_book(&library, &format!("Filler {i}")).await;
        }

        let re_fetched = library.get_book(&id).await.unwrap();
        assert!(
            Arc::ptr_eq(&held, &re_fetched),
            "expected same Arc instance while caller still holds it",
        );
    }

    #[tokio::test]
    async fn evicted_uuid_reloads_after_holder_drops() {
        let temp_dir = TempDir::new("flts_test");
        let library = Library::open_with_capacity(temp_dir.path.join("lib"), 2)
            .await
            .unwrap();

        let id = make_saved_book(&library, "Dropped").await;
        let first = library.get_book(&id).await.unwrap();
        let first_ptr = Arc::as_ptr(&first);

        for i in 0..4 {
            let _ = make_saved_book(&library, &format!("Filler {i}")).await;
        }

        drop(first);

        let reloaded = library.get_book(&id).await.unwrap();
        assert_ne!(
            Arc::as_ptr(&reloaded),
            first_ptr,
            "expected a fresh Arc after the last holder dropped",
        );
    }

    #[tokio::test]
    async fn weak_entry_cleared_when_book_dropped() {
        let temp_dir = TempDir::new("flts_test");
        let library = Library::open_with_capacity(temp_dir.path.join("lib"), 1)
            .await
            .unwrap();

        let id = make_saved_book(&library, "Ephemeral").await;
        let first_ptr = Arc::as_ptr(&library.get_book(&id).await.unwrap());

        for i in 0..3 {
            let _ = make_saved_book(&library, &format!("Other {i}")).await;
        }

        let reloaded = library.get_book(&id).await.unwrap();
        assert_ne!(
            Arc::as_ptr(&reloaded),
            first_ptr,
            "evicted book without holder must reload as a fresh instance",
        );
        assert_eq!(
            Arc::strong_count(&reloaded),
            2,
            "expected exactly two strong refs: caller + LRU pin",
        );
    }

    #[tokio::test]
    async fn integration_translate_paragraph_writes_cards() {
        let tmp = TempDir::new("flts_card_int");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "No puedo más.").await;

        let paragraph = paragraph_with(
            "Я больше не могу.",
            vec![
                full_word("No", "no", "не", "adv", &["не"], false),
                full_word("puedo", "poder", "мочь", "verb", &["могу"], false),
                full_word("más", "más", "больше", "adv", &["больше"], false),
                full_word(".", ".", ".", "punct", &[], true),
            ],
        );

        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, Language::from_639_3("rus").unwrap())
            .await
            .unwrap();

        let deck = tmp.path.join("lib").join("cards").join("spa-rus");
        let mut names: Vec<String> = std::fs::read_dir(&deck)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "más.json".to_string(),
                "no.json".to_string(),
                "poder.json".to_string(),
            ]
        );

        let poder = std::fs::read_to_string(deck.join("poder.json")).unwrap();
        let card: Card = serde_json::from_str(&poder).unwrap();
        assert_eq!(card.lemma, "poder");
        assert_eq!(card.translations_flat(), vec!["мочь"]);
        assert_eq!(card.examples.len(), 1);
        assert_eq!(card.examples[0].book_id, book_id);
        assert_eq!(card.examples[0].chapter, 0);
        assert_eq!(card.examples[0].paragraph, 0);
    }

    #[tokio::test]
    async fn integration_idempotent_on_repeat() {
        let tmp = TempDir::new("flts_card_idemp");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "No puedo más.").await;

        let paragraph = paragraph_with(
            "Я больше не могу.",
            vec![full_word(
                "puedo",
                "poder",
                "мочь",
                "verb",
                &["могу"],
                false,
            )],
        );
        let tgt = Language::from_639_3("rus").unwrap();

        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, tgt)
            .await
            .unwrap();
        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, tgt)
            .await
            .unwrap();

        let card_path = tmp
            .path
            .join("lib")
            .join("cards")
            .join("spa-rus")
            .join("poder.json");
        let body = std::fs::read_to_string(&card_path).unwrap();
        let card: Card = serde_json::from_str(&body).unwrap();
        assert_eq!(card.translations_flat(), vec!["мочь"]);
        assert_eq!(card.examples.len(), 1);
    }

    #[tokio::test]
    async fn integration_new_paragraph_appends_example() {
        let tmp = TempDir::new("flts_card_append");
        let library = Library::open(tmp.path.join("lib")).await.unwrap();
        let book = library
            .create_book("Two-Paragraph Book", &Language::from_639_3("spa").unwrap())
            .await
            .unwrap();
        let book_id = {
            let mut b = book.lock().await;
            b.book.push_chapter(Some("Intro"));
            b.book.push_paragraph(0, "No puedo más.", None);
            b.book.push_paragraph(0, "Pueden venir mañana.", None);
            b.save().await.unwrap();
            b.book.id
        };

        let tgt = Language::from_639_3("rus").unwrap();
        let p_a = paragraph_with(
            "Я больше не могу.",
            vec![full_word(
                "puedo",
                "poder",
                "мочь",
                "verb",
                &["могу"],
                false,
            )],
        );
        let p_b = paragraph_with(
            "Они могут прийти завтра.",
            vec![full_word(
                "pueden",
                "poder",
                "мочь",
                "verb",
                &["могут"],
                false,
            )],
        );

        library
            .apply_paragraph_to_cards(book_id, 0, &p_a, tgt)
            .await
            .unwrap();
        library
            .apply_paragraph_to_cards(book_id, 1, &p_b, tgt)
            .await
            .unwrap();

        let card_path = tmp
            .path
            .join("lib")
            .join("cards")
            .join("spa-rus")
            .join("poder.json");
        let card: Card =
            serde_json::from_str(&std::fs::read_to_string(&card_path).unwrap()).unwrap();
        assert_eq!(card.translations_flat(), vec!["мочь"]);
        assert_eq!(card.examples.len(), 2);
        assert_eq!(card.examples[0].paragraph, 0);
        assert_eq!(card.examples[1].paragraph, 1);
    }

    #[tokio::test]
    async fn integration_apply_paragraph_merges_existing_conflict_file() {
        use crate::card::Example;
        let tmp = TempDir::new("flts_card_conflict_int");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "No puedo más.").await;

        let deck = tmp.path.join("lib").join("cards").join("spa-rus");
        tokio::fs::create_dir_all(&deck).await.unwrap();

        let other_book = Uuid::new_v4();
        let mut canonical_translations: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        canonical_translations.insert("verb".into(), vec!["мочь".into()]);
        let canonical = Card {
            version: 2,
            id: "flts_spa_rus_poder".into(),
            lemma: "poder".into(),
            translations: canonical_translations,
            examples: vec![Example {
                source: "puedo".into(),
                translation: "могу".into(),
                book_id: other_book,
                chapter: 0,
                paragraph: 0,
            }],
            anki_data: None,
        };
        tokio::fs::write(
            deck.join("poder.json"),
            serde_json::to_vec_pretty(&canonical).unwrap(),
        )
        .await
        .unwrap();

        let mut conflict_translations: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        conflict_translations.insert("verb".into(), vec!["уметь".into()]);
        let conflict = Card {
            version: 2,
            id: "flts_spa_rus_poder".into(),
            lemma: "poder".into(),
            translations: conflict_translations,
            examples: vec![Example {
                source: "pueden".into(),
                translation: "могут".into(),
                book_id: other_book,
                chapter: 1,
                paragraph: 5,
            }],
            anki_data: None,
        };
        let conflict_path = deck.join("poder.sync-conflict-20260520-test.json");
        tokio::fs::write(
            &conflict_path,
            serde_json::to_vec_pretty(&conflict).unwrap(),
        )
        .await
        .unwrap();

        let paragraph = paragraph_with(
            "Я больше не могу.",
            vec![full_word(
                "puedo",
                "poder",
                "мочь",
                "verb",
                &["могу_новое"],
                false,
            )],
        );
        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, Language::from_639_3("rus").unwrap())
            .await
            .unwrap();

        // Sibling must be gone, no temp files.
        assert!(!conflict_path.exists(), "conflict sibling must be deleted");
        let names: Vec<String> = std::fs::read_dir(&deck)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(
            names.iter().all(|n| !n.contains('~')),
            "found stray temp file in {names:?}"
        );
        assert_eq!(names, vec!["poder.json".to_string()]);

        // Merged card has translations from canonical + conflict + new update.
        let on_disk: Card =
            serde_json::from_slice(&tokio::fs::read(deck.join("poder.json")).await.unwrap())
                .unwrap();
        assert_eq!(on_disk.translations_flat(), vec!["мочь", "уметь"]);
        assert_eq!(on_disk.examples.len(), 3);

        let provenances: std::collections::HashSet<_> = on_disk
            .examples
            .iter()
            .map(|e| (e.book_id, e.chapter, e.paragraph))
            .collect();
        assert!(provenances.contains(&(other_book, 0, 0)));
        assert!(provenances.contains(&(other_book, 1, 5)));
        assert!(provenances.contains(&(book_id, 0, 0)));
    }

    #[tokio::test]
    async fn integration_noisy_pos_persists_to_disk() {
        // Regression for the os-error-2 the user observed when the LLM
        // returned multi-PoS values like
        //   "Существительное / Прилагательное"
        //   "глагол (герундий/причастие настоящего времени)"
        // The slashes were interpreted as path separators and the card
        // file never landed. v2 schema keys cards by lemma only — the
        // noisy PoS lives inside the translations map keys, so the card
        // file lands at a safe lemma-only filename.
        let tmp = TempDir::new("flts_card_noisy_pos");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "good judge").await;
        let paragraph = paragraph_with(
            "хорошо судить",
            vec![
                full_word(
                    "good",
                    "good",
                    "хорошо",
                    "Существительное / Прилагательное",
                    &["хорошо"],
                    false,
                ),
                full_word(
                    "judge",
                    "judge",
                    "судить",
                    "глагол (герундий/причастие настоящего времени)",
                    &["судить"],
                    false,
                ),
            ],
        );

        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, Language::from_639_3("rus").unwrap())
            .await
            .unwrap();

        let deck = tmp.path.join("lib").join("cards").join("spa-rus");
        let names: std::collections::HashSet<String> = std::fs::read_dir(&deck)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(
            names.contains("good.json"),
            "expected the 'good' card at a lemma-only filename, got {names:?}"
        );
        assert!(
            names.contains("judge.json"),
            "expected the 'judge' card at a lemma-only filename, got {names:?}"
        );

        // The noisy PoS strings survive as keys inside the card's
        // translations map (the canonicalized form, slashes preserved).
        let good: Card =
            serde_json::from_slice(&std::fs::read(deck.join("good.json")).unwrap()).unwrap();
        assert!(
            good.translations
                .keys()
                .any(|k| k.contains("существительное")),
            "expected noisy PoS as a key inside translations, got {:?}",
            good.translations.keys().collect::<Vec<_>>()
        );
        let judge: Card =
            serde_json::from_slice(&std::fs::read(deck.join("judge.json")).unwrap()).unwrap();
        assert!(
            judge
                .translations
                .keys()
                .any(|k| k.contains("глагол")),
            "expected noisy PoS as a key inside translations, got {:?}",
            judge.translations.keys().collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn integration_uses_dash_separator_for_directory() {
        let tmp = TempDir::new("flts_card_dash");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "hola").await;
        let paragraph = paragraph_with(
            "привет",
            vec![full_word(
                "hola",
                "hola",
                "привет",
                "interj",
                &["привет"],
                false,
            )],
        );
        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, Language::from_639_3("rus").unwrap())
            .await
            .unwrap();
        assert!(tmp.path.join("lib").join("cards").join("spa-rus").exists());
        assert!(!tmp.path.join("lib").join("cards").join("spa_rus").exists());
    }

    #[tokio::test]
    async fn lru_capacity_respected() {
        let temp_dir = TempDir::new("flts_test");
        let capacity = 3;
        let library = Library::open_with_capacity(temp_dir.path.join("lib"), capacity)
            .await
            .unwrap();

        for i in 0..(capacity + 3) {
            let _ = make_saved_book(&library, &format!("Book {i}")).await;
        }

        assert_eq!(
            library.books_cache.live_values().await.len(),
            capacity,
            "warm LRU must not exceed capacity",
        );
    }

    async fn seed_translation(
        library: &Library,
        book_id: Uuid,
        paragraph_id: usize,
        paragraph: &translation_import::ParagraphTranslation,
        target_language: Language,
    ) {
        use crate::translator::TranslationModel;
        let book_arc = library.get_book(&book_id).await.unwrap();
        let mut book = book_arc.lock().await;
        let translation_arc = book.get_or_create_translation(&target_language).await;
        translation_arc.lock().await.add_paragraph_translation(
            paragraph_id,
            paragraph,
            TranslationModel::Gemini25Flash,
        );
        book.save().await.unwrap();
    }

    #[tokio::test]
    async fn backfill_empty_library() {
        let tmp = TempDir::new("flts_backfill_empty");
        let library_path = tmp.path.join("lib");
        let library = Library::open(library_path.clone()).await.unwrap();

        library.backfill_cards_from_translations().await.unwrap();

        let cards_dir = library_path.join("cards");
        assert!(
            !cards_dir.exists() || std::fs::read_dir(&cards_dir).unwrap().next().is_none(),
            "expected no cards after backfill of empty library",
        );
    }

    #[tokio::test]
    async fn backfill_book_without_translations() {
        let tmp = TempDir::new("flts_backfill_notrans");
        let library_path = tmp.path.join("lib");
        let (library, _book_id) =
            library_with_one_paragraph_book(library_path.clone(), "No puedo más.").await;

        library.backfill_cards_from_translations().await.unwrap();

        let cards_dir = library_path.join("cards");
        assert!(
            !cards_dir.exists() || std::fs::read_dir(&cards_dir).unwrap().next().is_none(),
            "expected no cards for a book without translations",
        );
    }

    #[tokio::test]
    async fn backfill_book_with_one_translation_creates_cards() {
        let tmp = TempDir::new("flts_backfill_one");
        let library_path = tmp.path.join("lib");
        let (library, book_id) =
            library_with_one_paragraph_book(library_path.clone(), "No puedo más.").await;
        let paragraph = paragraph_with(
            "Я больше не могу.",
            vec![full_word(
                "puedo",
                "poder",
                "мочь",
                "verb",
                &["могу"],
                false,
            )],
        );
        seed_translation(
            &library,
            book_id,
            0,
            &paragraph,
            Language::from_639_3("rus").unwrap(),
        )
        .await;

        library.backfill_cards_from_translations().await.unwrap();

        let card_path = library_path
            .join("cards")
            .join("spa-rus")
            .join("poder.json");
        let body = std::fs::read_to_string(&card_path).unwrap();
        let card: Card = serde_json::from_str(&body).unwrap();
        assert_eq!(card.lemma, "poder");
        assert_eq!(card.translations_flat(), vec!["мочь"]);
        assert_eq!(card.examples.len(), 1);
        assert_eq!(card.examples[0].book_id, book_id);
        assert_eq!(card.examples[0].chapter, 0);
        assert_eq!(card.examples[0].paragraph, 0);
    }

    #[tokio::test]
    async fn backfill_idempotent() {
        let tmp = TempDir::new("flts_backfill_idemp");
        let library_path = tmp.path.join("lib");
        let (library, book_id) =
            library_with_one_paragraph_book(library_path.clone(), "No puedo más.").await;
        let paragraph = paragraph_with(
            "Я больше не могу.",
            vec![full_word(
                "puedo",
                "poder",
                "мочь",
                "verb",
                &["могу"],
                false,
            )],
        );
        seed_translation(
            &library,
            book_id,
            0,
            &paragraph,
            Language::from_639_3("rus").unwrap(),
        )
        .await;

        library.backfill_cards_from_translations().await.unwrap();
        let deck_dir = library_path.join("cards").join("spa-rus");
        let after_first: Vec<u8> = std::fs::read(deck_dir.join("poder.json")).unwrap();

        library.backfill_cards_from_translations().await.unwrap();
        let after_second: Vec<u8> = std::fs::read(deck_dir.join("poder.json")).unwrap();

        assert_eq!(after_first, after_second);
        let names: Vec<String> = std::fs::read_dir(&deck_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["poder.json".to_string()]);
    }

    #[tokio::test]
    async fn backfill_walks_multiple_books_in_list_order() {
        let tmp = TempDir::new("flts_backfill_multi");
        let library_path = tmp.path.join("lib");
        let library = Library::open(library_path.clone()).await.unwrap();
        let spa = Language::from_639_3("spa").unwrap();
        let rus = Language::from_639_3("rus").unwrap();

        let book_a_arc = library.create_book("Book A", &spa).await.unwrap();
        let book_a_id = {
            let mut b = book_a_arc.lock().await;
            b.book.push_chapter(Some("Intro"));
            b.book.push_paragraph(0, "No puedo más.", None);
            b.save().await.unwrap();
            b.book.id
        };
        let paragraph_a = paragraph_with(
            "Я больше не могу.",
            vec![full_word(
                "puedo",
                "poder",
                "мочь",
                "verb",
                &["могу"],
                false,
            )],
        );
        seed_translation(&library, book_a_id, 0, &paragraph_a, rus).await;

        let book_b_arc = library.create_book("Book B", &spa).await.unwrap();
        let book_b_id = {
            let mut b = book_b_arc.lock().await;
            b.book.push_chapter(Some("Intro"));
            b.book.push_paragraph(0, "Quiero comer.", None);
            b.save().await.unwrap();
            b.book.id
        };
        let paragraph_b = paragraph_with(
            "Я хочу есть.",
            vec![full_word(
                "comer",
                "comer",
                "есть",
                "verb",
                &["есть"],
                false,
            )],
        );
        seed_translation(&library, book_b_id, 0, &paragraph_b, rus).await;

        library.backfill_cards_from_translations().await.unwrap();

        let deck = library_path.join("cards").join("spa-rus");
        let poder: Card =
            serde_json::from_str(&std::fs::read_to_string(deck.join("poder.json")).unwrap())
                .unwrap();
        let comer: Card =
            serde_json::from_str(&std::fs::read_to_string(deck.join("comer.json")).unwrap())
                .unwrap();

        assert_eq!(poder.examples.len(), 1);
        assert_eq!(poder.examples[0].book_id, book_a_id);
        assert_eq!(comer.examples.len(), 1);
        assert_eq!(comer.examples[0].book_id, book_b_id);
    }

    #[tokio::test]
    async fn backfill_walks_multiple_target_languages_on_same_book() {
        let tmp = TempDir::new("flts_backfill_multilang");
        let library_path = tmp.path.join("lib");
        let (library, book_id) =
            library_with_one_paragraph_book(library_path.clone(), "Puedo.").await;

        let p_rus = paragraph_with(
            "Я могу.",
            vec![full_word(
                "puedo",
                "poder",
                "мочь",
                "verb",
                &["могу"],
                false,
            )],
        );
        let p_eng = translation_import::ParagraphTranslation {
            timestamp: 0,
            total_tokens: None,
            sentences: vec![translation_import::Sentence {
                full_translation: "I can.".into(),
                words: vec![full_word("puedo", "poder", "can", "verb", &["can"], false)],
            }],
        };
        seed_translation(
            &library,
            book_id,
            0,
            &p_rus,
            Language::from_639_3("rus").unwrap(),
        )
        .await;
        seed_translation(
            &library,
            book_id,
            0,
            &p_eng,
            Language::from_639_3("eng").unwrap(),
        )
        .await;

        library.backfill_cards_from_translations().await.unwrap();

        let rus_card: Card = serde_json::from_str(
            &std::fs::read_to_string(
                library_path
                    .join("cards")
                    .join("spa-rus")
                    .join("poder.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let eng_card: Card = serde_json::from_str(
            &std::fs::read_to_string(
                library_path
                    .join("cards")
                    .join("spa-eng")
                    .join("poder.json"),
            )
            .unwrap(),
        )
        .unwrap();

        assert_eq!(rus_card.translations_flat(), vec!["мочь"]);
        assert_eq!(eng_card.translations_flat(), vec!["can"]);
    }
}
