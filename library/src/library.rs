use std::{
    error::Error,
    fmt::Display,
    path::{Path, PathBuf},
};

use isolang::Language;
use log::trace;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::{
    book::book::Book,
    card::{Card, CardWordFields, card_update_from_word},
    epub_importer::EpubBook,
    library::{
        file_watcher::LibraryFileChange, library_book::LibraryBook, library_card::LibraryCardStore,
    },
    store::LibraryStore,
};

pub use library_book::{BookHandle, BookRef, BookSnapshot, BookStatus};

pub mod file_watcher;
pub mod library_book;
pub mod library_card;
mod registry;
mod session;

/// Books pinned in the warm LRU. Beyond it a book stays reachable through the
/// weak index until its last holder drops.
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

/// The chapter that owns `paragraph_id`, if any. Shared by the card paths so a
/// card's provenance matches the chapter the reader clicked in.
fn chapter_index_for_paragraph(
    book: &crate::book::book::Book,
    paragraph_id: usize,
) -> Option<usize> {
    book.chapter_views()
        .find(|ch| (0..ch.paragraph_count()).any(|i| ch.paragraph_view(i).id == paragraph_id))
        .map(|ch| ch.idx)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardCreationOutcome {
    Created,
    AlreadyExists,
}

pub struct LibraryTranslationMetadata {
    pub id: Uuid,
    pub source_langugage: String,
    pub target_language: String,
    pub translated_paragraphs_count: usize,
}

pub struct LibraryBookMetadata {
    pub id: Uuid,
    pub title: String,
    pub chapters_count: usize,
    pub paragraphs_count: usize,
    pub translations_metadata: Vec<LibraryTranslationMetadata>,
    pub folder_path: Vec<String>,
}

/// What adopting other devices' writes changed here.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PeerChanges {
    /// The shelf may differ: books, their folders or their translation counts.
    pub library: bool,
    /// Loaded books whose summaries, position or folder changed, or whose
    /// changed paragraphs are not known one by one.
    pub books: Vec<Uuid>,
    /// Paragraphs of loaded books that gained a translation.
    pub paragraphs: Vec<(Uuid, usize)>,
    /// Books deleted on another device.
    pub deleted: Vec<Uuid>,
    pub cards: bool,
}

impl PeerChanges {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Housekeeping waits until startup is well over, then recurs rarely: it
/// rewrites whole files and nothing reads better for it.
const MAINTAIN_AFTER: std::time::Duration = std::time::Duration::from_secs(120);
const MAINTAIN_EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

type Closed = tokio::sync::watch::Receiver<(u64, Option<Result<(), String>>)>;
type Retired = std::sync::Arc<std::sync::Mutex<Vec<u64>>>;

/// True once the library has closed, or was dropped.
async fn closed_within(closed: &mut Closed, wait: std::time::Duration) -> bool {
    let done = |closed: &Closed| closed.borrow().1.as_ref().is_some_and(Result::is_ok);
    if done(closed) {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(wait) => false,
        changed = closed.changed() => changed.is_err() || done(closed),
    }
}

async fn maintain_periodically(store: LibraryStore, retired: Retired, mut closed: Closed) {
    let mut wait = MAINTAIN_AFTER;
    while !closed_within(&mut closed, wait).await {
        wait = MAINTAIN_EVERY;
        let retired = retired.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match store.run(move |db| db.maintain(&retired)).await {
            Ok(done) => log::debug!("library store maintained: {done:?}"),
            Err(err) => log::warn!("library store maintenance skipped: {err:#}"),
        }
    }
}

/// Compaction is looked into this often, and done only once nothing was
/// written for a while: writers wait for it while it runs.
const COMPACT_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
const COMPACT_IDLE: std::time::Duration = std::time::Duration::from_secs(30);

async fn compact_when_idle(store: LibraryStore, mut closed: Closed) {
    while !closed_within(&mut closed, COMPACT_EVERY).await {
        let compacting = store.clone();
        match tokio::task::spawn_blocking(move || compacting.compact_if_idle(COMPACT_IDLE)).await {
            Ok(Err(err)) => log::debug!("library store compaction skipped: {err:#}"),
            Ok(Ok(_)) | Err(_) => {}
        }
    }
}

/// How often a commit that failed is tried again on its own.
const REPUBLISH_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// Lands what a failed commit left staged, for as long as the library is open.
async fn republish(store: LibraryStore, mut closed: Closed) {
    while !closed_within(&mut closed, REPUBLISH_EVERY).await {
        if store
            .read(|db| Ok(db.has_unpublished()))
            .await
            .unwrap_or(false)
            && let Err(err) = store.run(|db| db.publish()).await
        {
            log::debug!("library store still cannot commit: {err:#}");
        }
    }
}

use registry::{BookRequest, serve_books};
#[cfg(test)]
use std::sync::Arc;

#[derive(Clone)]
pub struct Library {
    library_root: PathBuf,
    store: LibraryStore,
    card_store: LibraryCardStore,
    books: mpsc::Sender<BookRequest>,
    close_control: tokio::sync::watch::Sender<u64>,
    closed: Closed,
    retired: Retired,
}

impl Library {
    pub async fn open(library_root: PathBuf) -> anyhow::Result<Self> {
        Self::open_with_capacity(library_root, DEFAULT_BOOKS_CACHE_CAPACITY).await
    }

    /// `open`, telling `progress` how far the import of old-format files is.
    /// On a large library that import is most of the first start.
    pub async fn open_reporting(
        library_root: PathBuf,
        progress: crate::legacy::Progress,
    ) -> anyhow::Result<Self> {
        Self::open_inner(library_root, DEFAULT_BOOKS_CACHE_CAPACITY, Some(progress)).await
    }

    pub async fn open_with_capacity(
        library_root: PathBuf,
        cache_capacity: usize,
    ) -> anyhow::Result<Self> {
        Self::open_inner(library_root, cache_capacity, None).await
    }

    async fn open_inner(
        library_root: PathBuf,
        cache_capacity: usize,
        progress: Option<crate::legacy::Progress>,
    ) -> anyhow::Result<Self> {
        if !tokio::fs::try_exists(&library_root).await? {
            tokio::fs::create_dir_all(&library_root).await?;
        }

        let lease = session::LibraryLease::acquire(&library_root)?;
        let library_root = lease.root.clone();
        let store = {
            let root = library_root.clone();
            tokio::task::spawn_blocking(move || LibraryStore::open(&root)).await??
        };
        import_legacy(&library_root, &store, progress).await;
        let card_store = LibraryCardStore::new(store.clone());
        let (books, requests) = mpsc::channel(256);
        let (close_control, close_requests) = tokio::sync::watch::channel(0);
        let (completion, closed) = tokio::sync::watch::channel((0, None));
        tokio::spawn(serve_books(
            lease,
            store.clone(),
            cache_capacity,
            requests,
            close_requests,
            completion,
        ));
        tokio::spawn(republish(store.clone(), closed.clone()));
        tokio::spawn(compact_when_idle(store.clone(), closed.clone()));
        let retired = Retired::default();
        tokio::spawn(maintain_periodically(
            store.clone(),
            retired.clone(),
            closed.clone(),
        ));

        Ok(Library {
            library_root,
            store,
            card_store,
            books,
            close_control,
            closed,
            retired,
        })
    }

    pub fn card_store(&self) -> &LibraryCardStore {
        &self.card_store
    }

    async fn request<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> BookRequest,
    ) -> anyhow::Result<T> {
        let (reply, response) = oneshot::channel();
        self.books
            .send(build(reply))
            .await
            .map_err(|_| anyhow::anyhow!("book registry closed"))?;
        response
            .await
            .map_err(|_| anyhow::anyhow!("book registry dropped reply"))
    }

    pub async fn get_book(&self, uuid: &Uuid) -> anyhow::Result<BookHandle> {
        let id = *uuid;
        self.request(|reply| BookRequest::Get { id, reply }).await?
    }

    /// The book if it is already loaded; never touches disk.
    #[cfg(test)]
    async fn cached_book(&self, id: Uuid) -> Option<BookHandle> {
        self.request(|reply| BookRequest::Cached { id, reply })
            .await
            .ok()
            .flatten()
    }

    /// Resolves once no load for `id` is in flight and the book is deleted.
    pub(crate) async fn remove_book(&self, id: Uuid) -> anyhow::Result<()> {
        self.request(|reply| BookRequest::Remove { id, reply })
            .await?
    }

    #[cfg(test)]
    pub(crate) async fn cached_books(&self) -> Vec<BookHandle> {
        self.request(|reply| BookRequest::Live { reply })
            .await
            .unwrap_or_default()
    }

    #[cfg(test)]
    async fn inflight_loads(&self) -> usize {
        self.request(|reply| BookRequest::Inflight { reply })
            .await
            .unwrap()
    }

    /// Create the card for one word in a translated paragraph, or report that
    /// it already exists.
    pub async fn create_card_for_word(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
        sentence_id: usize,
        word_id: usize,
        target_language: Language,
    ) -> anyhow::Result<CardCreationOutcome> {
        let book = self.get_book(&book_id).await?.snapshot();

        let source_language = Language::from_639_3(&book.book.language).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown source language code on book {book_id}: {}",
                book.book.language
            )
        })?;
        let chapter_index =
            chapter_index_for_paragraph(&book.book, paragraph_id).ok_or_else(|| {
                anyhow::anyhow!("paragraph {paragraph_id} is not attached to any chapter")
            })?;

        let translation = book.translation(&target_language).ok_or_else(|| {
            anyhow::anyhow!("book {book_id} has no translation to {target_language}")
        })?;

        let update = {
            let paragraph = translation
                .paragraph_view(paragraph_id)
                .ok_or_else(|| anyhow::anyhow!("paragraph {paragraph_id} is not translated"))?;
            // The frontend can race a re-translation that shrinks the view;
            // indexing raw would abort the process (panic = abort).
            if sentence_id >= paragraph.sentence_count() {
                anyhow::bail!("sentence {sentence_id} is out of range");
            }
            let sentence = paragraph.sentence_view(sentence_id);
            if word_id >= sentence.word_count() {
                anyhow::bail!("word {word_id} is out of range");
            }
            let word = sentence.word_view(word_id);
            card_update_from_word(
                CardWordFields {
                    is_punctuation: word.is_punctuation,
                    original_initial_form: word.grammar.original_initial_form.as_ref(),
                    target_initial_form: word.grammar.target_initial_form.as_ref(),
                    part_of_speech: word.grammar.part_of_speech.as_ref(),
                },
                &sentence.source_text(),
                sentence.full_translation.as_ref(),
                source_language,
                target_language,
                book_id,
                chapter_index,
                paragraph_id,
            )
            .map_err(|reason| anyhow::anyhow!("cannot add word to Anki: {reason}"))?
        };

        drop(book);

        let created = self
            .card_store
            .create_if_absent(
                &Card::new_from_update(&update),
                &update.key.source_language,
                &update.key.target_language,
            )
            .await?;
        Ok(if created {
            CardCreationOutcome::Created
        } else {
            CardCreationOutcome::AlreadyExists
        })
    }

    pub async fn list_books(&self) -> anyhow::Result<Vec<LibraryBookMetadata>> {
        let books = self.store.read(|db| db.list_books()).await?;
        Ok(books
            .into_iter()
            .map(|book| LibraryBookMetadata {
                id: book.id,
                title: book.title,
                chapters_count: book.chapters_count,
                paragraphs_count: book.paragraphs_count,
                translations_metadata: book
                    .translations
                    .into_iter()
                    .map(|t| LibraryTranslationMetadata {
                        id: t.id,
                        source_langugage: t.source_language,
                        target_language: t.target_language,
                        translated_paragraphs_count: t.translated_paragraphs_count,
                    })
                    .collect(),
                folder_path: book.folder_path,
            })
            .collect())
    }

    pub fn store(&self) -> &LibraryStore {
        &self.store
    }

    /// Adopts what other devices' shards (or their leftover legacy files) now
    /// hold, and brings every loaded book that is behind up to date.
    pub async fn refresh_from_peers(&self) -> anyhow::Result<PeerChanges> {
        let imported = import_legacy(&self.library_root, &self.store, None).await;
        let adopted = match self.store.run(|db| db.refresh()).await {
            Ok(adopted) => adopted,
            Err(err) => {
                // Whatever got in before the failure may include cards.
                self.card_store.invalidate_all();
                return Err(err);
            }
        };
        let mut changes = PeerChanges {
            library: imported.books > 0 || adopted.shelf,
            cards: imported.cards || adopted.cards,
            deleted: adopted.purged.clone(),
            ..PeerChanges::default()
        };
        if changes.cards {
            self.card_store.invalidate_all();
        }
        if !adopted.purged.is_empty() {
            // The set-aside old-format copy holds the same text.
            let (local, purged) = (self.store.local_dir().to_owned(), adopted.purged.clone());
            let swept = tokio::task::spawn_blocking(move || {
                purged
                    .into_iter()
                    .try_for_each(|id| crate::legacy::remove_backups_of(&local, id))
            });
            if let Ok(Err(err)) = swept.await {
                log::warn!("could not remove the old copy of a deleted book: {err}");
            }
        }
        let loaded = self.request(|reply| BookRequest::All { reply }).await?;
        let ids: Vec<Uuid> = loaded.iter().map(|b| b.snapshot().book.id).collect();
        let states = self
            .store
            .read(move |db| {
                ids.iter()
                    .map(|id| Ok((db.is_purged(*id)?, db.book_generation(*id))))
                    .collect::<anyhow::Result<Vec<_>>>()
            })
            .await?;
        for (book, (purged, generation)) in loaded.into_iter().zip(states) {
            let snapshot = book.snapshot();
            let id = snapshot.book.id;
            if purged {
                // Deleted elsewhere: only the owner and our leftovers remain.
                let _ = self.remove_book(id).await;
                if !changes.deleted.contains(&id) {
                    changes.deleted.push(id);
                }
                continue;
            }
            // Behind for any reason, an earlier failed reload included.
            if snapshot.store_generation >= generation {
                continue;
            }
            match book.reload().await {
                Ok(false) => {}
                Ok(true) => match adopted.books.get(&id) {
                    Some(change) if !change.summaries && !change.user_state => {
                        if change.paragraphs.is_empty() {
                            changes.books.push(id);
                        }
                        changes
                            .paragraphs
                            .extend(change.paragraphs.iter().map(|paragraph| (id, *paragraph)));
                    }
                    Some(change) => {
                        changes.books.push(id);
                        changes
                            .paragraphs
                            .extend(change.paragraphs.iter().map(|paragraph| (id, *paragraph)));
                    }
                    None => changes.books.push(id),
                },
                Err(err) => log::debug!("book {id} is not up to date yet: {err:#}"),
            }
        }
        Ok(changes)
    }

    /// Names the store writers of devices that left for good, so that
    /// housekeeping stops waiting for them.
    pub fn set_retired_writers(&self, retired: Vec<u64>) {
        *self.retired.lock().unwrap_or_else(|e| e.into_inner()) = retired;
    }

    /// What `maintain_periodically` does on its own schedule.
    pub async fn maintain(&self) -> anyhow::Result<crate::store::Maintained> {
        let retired = self
            .retired
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        self.store.run(move |db| db.maintain(&retired)).await
    }

    /// See [`crate::store::Inner::note_shared_identity`].
    pub async fn note_shared_identity(&self) -> anyhow::Result<()> {
        self.store.run(|db| db.note_shared_identity()).await
    }

    /// The user keeps this installation's sync identity as it is.
    pub async fn settle_shared_identity(&self) -> anyhow::Result<()> {
        self.store.run(|db| db.settle_shared_identity()).await
    }

    /// The user gives this installation an identity of its own: a new store
    /// writer here, and a new sync identity by the caller, who must have
    /// stopped the sync engine first.
    pub async fn take_new_identity(&self) -> anyhow::Result<()> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.start_over_as_new_writer()).await??;
        self.card_store.invalidate_all();
        self.settle_shared_identity().await
    }

    pub async fn store_health(&self) -> anyhow::Result<crate::store::StoreHealth> {
        self.store.read(|db| Ok(db.health())).await
    }

    pub async fn create_book_plain(
        &self,
        title: &str,
        text: &str,
        language: &Language,
    ) -> anyhow::Result<Uuid> {
        let mut book = Book::create(Uuid::new_v4(), title, language);
        let chapter_index = book.push_chapter(None);
        for paragraph in split_paragraphs(text) {
            book.push_paragraph(chapter_index, paragraph, None);
        }

        Ok(self.create_book(book).await?.snapshot().book.id)
    }

    pub async fn create_book_epub(
        &self,
        epub: &EpubBook,
        language: &Language,
    ) -> anyhow::Result<Uuid> {
        let mut book = Book::create(Uuid::new_v4(), &epub.title, language);
        for ch in &epub.chapters {
            let ch_idx = book.push_chapter(Some(&ch.title));
            for p in &ch.paragraphs {
                book.push_paragraph(ch_idx, &p.text, Some(&p.html));
            }
        }

        Ok(self.create_book(book).await?.snapshot().book.id)
    }

    /// Ok once nothing of this device's is waiting for a commit.
    pub async fn flush(&self) -> anyhow::Result<()> {
        let books = self.request(|reply| BookRequest::All { reply }).await?;
        let results = futures_util::future::join_all(books.iter().map(|book| async move {
            match book.flush().await {
                // A book on its way out stages what it holds as it goes.
                Err(_) if *book.admission.borrow() != BookStatus::Active => Ok(()),
                result => result,
            }
        }))
        .await;
        for result in results {
            result?;
        }
        self.store.run(|db| db.publish()).await
    }
    pub async fn close(&self) -> anyhow::Result<()> {
        if self.closed.borrow().1.as_ref().is_some_and(Result::is_ok) {
            return Ok(());
        }
        let mut done = self.closed.clone();
        self.close_control
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        let epoch = *self.close_control.borrow();
        loop {
            let (completed, result) = done.borrow_and_update().clone();
            if completed >= epoch {
                if let Some(result) = result {
                    return result.map_err(anyhow::Error::msg);
                }
            }
            done.changed()
                .await
                .map_err(|_| anyhow::anyhow!("library closed without completion"))?;
        }
    }
    pub fn root(&self) -> &Path {
        &self.library_root
    }

    pub async fn handle_file_change_event(
        &self,
        event: &LibraryFileChange,
    ) -> anyhow::Result<PeerChanges> {
        trace!("Starting file change event handling: {:?}...", event);
        let LibraryFileChange::StoreChanged { paths } = event;
        // Our own commits echo back through the watcher.
        if paths.iter().all(|path| self.store.is_own_file(path)) {
            return Ok(PeerChanges::default());
        }
        let result = self.refresh_from_peers().await;
        trace!("Finish file change event handling");
        result
    }
}

/// Imports what old-format files are in the root. Never fatal: the store
/// works without them, and they are looked at again on the next refresh.
async fn import_legacy(
    root: &Path,
    store: &LibraryStore,
    progress: Option<crate::legacy::Progress>,
) -> crate::legacy::Imported {
    crate::legacy::migrate_reporting(root, store, progress)
        .await
        .unwrap_or_else(|err| {
            log::warn!("legacy import skipped: {err:#}");
            crate::legacy::Imported::default()
        })
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
        let book_id = create_saved_book(&library, "Test Book", "spa", &[paragraph_text]).await;
        (library, book_id)
    }

    async fn create_saved_book(
        library: &Library,
        title: &str,
        language: &str,
        paragraphs: &[&str],
    ) -> Uuid {
        let mut book = Book::create(
            Uuid::new_v4(),
            title,
            &Language::from_639_3(language).unwrap(),
        );
        if !paragraphs.is_empty() {
            let chapter = book.push_chapter(Some("Intro"));
            for paragraph in paragraphs {
                book.push_paragraph(chapter, paragraph, None);
            }
        }
        let id = book.id;
        library.create_book(book).await.unwrap();
        id
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

        create_saved_book(&library, "First Book", "eng", &[]).await;
        create_saved_book(&library, "Second Book", "eng", &[]).await;

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

        let id = create_saved_book(&library, "Categorized", "eng", &[]).await;
        library
            .get_book(&id)
            .await
            .unwrap()
            .update_folder_path(vec!["Shelf".into(), "Modern".into()])
            .await
            .unwrap();

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
        create_saved_book(library, title, "eng", &[]).await
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

    #[tokio::test(start_paused = true)]
    async fn evicted_uuid_reloads_after_holder_drops() {
        let temp_dir = TempDir::new("flts_test");
        let library = Library::open_with_capacity(temp_dir.path.join("lib"), 2)
            .await
            .unwrap();

        let id = make_saved_book(&library, "Dropped").await;
        let first = library.get_book(&id).await.unwrap();
        let first_owner = first.incarnation;

        for i in 0..4 {
            let _ = make_saved_book(&library, &format!("Filler {i}")).await;
        }

        drop(first);
        // The sweep is what notices a dropped handle.
        for _ in 0..100 {
            if library.cached_book(id).await.is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        let reloaded = library.get_book(&id).await.unwrap();
        assert!(
            reloaded.incarnation > first_owner,
            "expected a fresh owner after the last holder dropped",
        );
    }

    #[tokio::test]
    async fn weak_entry_cleared_when_book_dropped() {
        let temp_dir = TempDir::new("flts_test");
        let library = Library::open_with_capacity(temp_dir.path.join("lib"), 1)
            .await
            .unwrap();

        let id = make_saved_book(&library, "Ephemeral").await;
        let first_owner = library.get_book(&id).await.unwrap().incarnation;

        for i in 0..3 {
            let _ = make_saved_book(&library, &format!("Other {i}")).await;
        }

        let reloaded = library.get_book(&id).await.unwrap();
        assert!(
            reloaded.incarnation > first_owner,
            "evicted book without holder must reload as a fresh instance",
        );
        assert_eq!(
            Arc::strong_count(&reloaded),
            2,
            "expected exactly two strong refs: caller + LRU pin",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_get_book_single_flight_after_eviction() {
        let temp_dir = TempDir::new("flts_test");
        let library = Library::open_with_capacity(temp_dir.path.join("lib"), 1)
            .await
            .unwrap();
        let id = evicted_book(&library).await;

        let mut handles = Vec::new();
        for _ in 0..8 {
            let library = library.clone();
            handles.push(tokio::spawn(async move { library.get_book(&id).await }));
        }
        let mut books = Vec::new();
        for handle in handles {
            books.push(
                handle
                    .await
                    .unwrap()
                    .expect("concurrent get_book must not fail"),
            );
        }

        let first = &books[0];
        for book in &books[1..] {
            assert!(
                Arc::ptr_eq(first, book),
                "all concurrent callers must resolve to the same instance",
            );
        }
        assert_eq!(
            library.inflight_loads().await,
            0,
            "flights must be pruned once all callers are answered",
        );
    }

    /// A saved book that capacity 1 has pushed out, so the next get loads it.
    async fn evicted_book(library: &Library) -> Uuid {
        let id = make_saved_book(library, "Raced").await;
        let _ = make_saved_book(library, "Filler").await;
        id
    }

    /// Occupies the store until the returned sender fires, parking every load.
    async fn hold_store(library: &Library) -> std::sync::mpsc::Sender<()> {
        let store = library.store().clone();
        let (entered, held) = oneshot::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        tokio::task::spawn_blocking(move || {
            store
                .run_blocking(|_| {
                    let _ = entered.send(());
                    let _ = released.recv();
                    Ok(())
                })
                .unwrap()
        });
        held.await.unwrap();
        release
    }

    async fn wait_for_inflight_load(library: &Library) {
        for _ in 0..1000 {
            if library.inflight_loads().await > 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("load never became visible as in flight");
    }

    #[tokio::test]
    async fn book_registry_exits_when_library_drops() {
        let temp_dir = TempDir::new("flts_test");
        let baseline = tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks();
        let library = Library::open(temp_dir.path.join("lib")).await.unwrap();
        let id = make_saved_book(&library, "Short-lived").await;
        library.get_book(&id).await.unwrap();

        drop(library);

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while tokio::runtime::Handle::current()
                .metrics()
                .num_alive_tasks()
                > baseline
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("registry task must exit once the library drops");
    }

    #[tokio::test]
    async fn get_book_error_reaches_every_waiter() {
        let temp_dir = TempDir::new("flts_test");
        let library = Library::open(temp_dir.path.join("lib")).await.unwrap();
        let missing = Uuid::new_v4();

        let gets: Vec<_> = (0..4)
            .map(|_| {
                let library = library.clone();
                tokio::spawn(async move { library.get_book(&missing).await })
            })
            .collect();
        for get in gets {
            assert!(get.await.unwrap().is_err());
        }
        assert_eq!(library.inflight_loads().await, 0);
    }

    #[tokio::test]
    async fn remove_during_load_caches_nothing() {
        let temp_dir = TempDir::new("flts_test");
        let root = temp_dir.path.join("lib");
        let library = Library::open_with_capacity(root.clone(), 1).await.unwrap();
        let id = evicted_book(&library).await;

        let release = hold_store(&library).await;
        let get = {
            let library = library.clone();
            tokio::spawn(async move { library.get_book(&id).await })
        };
        wait_for_inflight_load(&library).await;
        let remove = {
            let library = library.clone();
            tokio::spawn(async move { library.remove_book(id).await })
        };

        assert!(
            get.await.unwrap().is_err(),
            "a get racing a remove must not hand out a book that is being deleted",
        );
        assert!(
            !remove.is_finished(),
            "remove must resolve only after the in-flight load finished",
        );
        release.send(()).unwrap();
        remove.await.unwrap().unwrap();

        assert!(library.cached_book(id).await.is_none());
        assert_eq!(library.inflight_loads().await, 0);
        assert!(library.get_book(&id).await.is_err());
        let listed = library.list_books().await.unwrap();
        assert!(listed.iter().all(|book| book.id != id));
    }

    #[tokio::test]
    async fn cancelled_get_book_does_not_abort_load() {
        let temp_dir = TempDir::new("flts_test");
        let root = temp_dir.path.join("lib");
        let library = Library::open_with_capacity(root.clone(), 1).await.unwrap();
        let id = evicted_book(&library).await;

        let release = hold_store(&library).await;
        let get = {
            let library = library.clone();
            tokio::spawn(async move { library.get_book(&id).await })
        };
        wait_for_inflight_load(&library).await;
        get.abort();
        let _ = get.await;
        assert_eq!(library.inflight_loads().await, 1);
        release.send(()).unwrap();

        // The abandoned load still lands; this get finds its book.
        for _ in 0..1000 {
            if library.inflight_loads().await == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        let cached = library.cached_book(id).await.expect("the load completed");
        let fetched = library.get_book(&id).await.unwrap();
        assert!(Arc::ptr_eq(&cached, &fetched));
        assert_eq!(library.inflight_loads().await, 0);
    }

    fn spanish_paragraph() -> translation_import::ParagraphTranslation {
        paragraph_with(
            "Я могу войти в дом.",
            vec![
                full_word("Puedo", "poder", "мочь", "verb", &["могу"], false),
                full_word("entrar", "entrar", "входить", "verb", &["войти"], false),
                full_word("en", "en", "в", "prep", &["в"], false),
                full_word("casa", "casa", "дом", "noun", &["дом"], false),
                full_word(".", ".", ".", "punct", &[], true),
            ],
        )
    }

    #[tokio::test]
    async fn create_card_for_word_writes_one_card() {
        let tmp = TempDir::new("flts_card_manual");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "Puedo entrar en casa.").await;
        let tgt = Language::from_639_3("rus").unwrap();
        let paragraph = spanish_paragraph();
        seed_translation(&library, book_id, 0, &paragraph, tgt).await;

        // Clicking the inflected "Puedo" creates the citation-form card.
        let outcome = library
            .create_card_for_word(book_id, 0, 0, 0, tgt)
            .await
            .unwrap();
        assert_eq!(outcome, CardCreationOutcome::Created);

        let card = stored_card(&library, "poder").await.unwrap();
        assert_eq!(card.lemma, "poder");
        assert_eq!(card.translations_flat(), vec!["мочь"]);
        let example = &card.examples[0];
        assert_eq!(example.source, "Puedo entrar en casa.");
        assert_eq!(example.translation, "Я могу войти в дом.");
        assert_eq!(example.book_id, book_id);
        assert_eq!(example.chapter, 0);
        assert_eq!(example.paragraph, 0);
        assert!(card.anki_data.is_none(), "new cards wait for the sync pass");

        // A second click is a no-op, not a second example.
        let outcome = library
            .create_card_for_word(book_id, 0, 0, 0, tgt)
            .await
            .unwrap();
        assert_eq!(outcome, CardCreationOutcome::AlreadyExists);
        let card = stored_card(&library, "poder").await.unwrap();
        assert_eq!(card.examples.len(), 1);

        // The other words were not touched by this one click.
        assert_eq!(deck(&library).await, vec!["poder".to_string()]);
    }

    async fn stored_card(library: &Library, slug: &str) -> Option<Card> {
        library.card_store().load("spa", "rus", slug).await.unwrap()
    }

    async fn deck(library: &Library) -> Vec<String> {
        library
            .card_store()
            .list_cards_in_pair("spa", "rus")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn create_card_for_word_rejects_ineligible_word() {
        let tmp = TempDir::new("flts_card_manual_skip");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "Puedo entrar en casa.").await;
        let tgt = Language::from_639_3("rus").unwrap();
        let paragraph = spanish_paragraph();
        seed_translation(&library, book_id, 0, &paragraph, tgt).await;

        let err = library
            .create_card_for_word(book_id, 0, 0, 4, tgt)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("punctuation"),
            "unexpected error: {err}"
        );
        assert!(
            deck(&library).await.is_empty(),
            "ineligible words must not write a card"
        );
    }

    #[tokio::test]
    async fn create_card_for_word_requires_a_translation() {
        let tmp = TempDir::new("flts_card_manual_notrans");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "Puedo entrar.").await;

        let err = library
            .create_card_for_word(book_id, 0, 0, 0, Language::from_639_3("rus").unwrap())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("no translation"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn create_card_for_word_leaves_an_existing_card_unchanged() {
        use crate::card::Example;
        let tmp = TempDir::new("flts_card_existing");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "No puedo más.").await;

        let other_book = Uuid::new_v4();
        let existing = Card {
            version: 2,
            id: "flts_spa_rus_poder".into(),
            lemma: "poder".into(),
            translations: std::collections::BTreeMap::from([(
                "verb".to_string(),
                vec!["мочь".to_string(), "уметь".to_string()],
            )]),
            examples: vec![Example {
                source: "puedo".into(),
                translation: "могу".into(),
                book_id: other_book,
                chapter: 0,
                paragraph: 0,
            }],
            anki_data: None,
        };
        library
            .card_store()
            .create_if_absent(&existing, "spa", "rus")
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
        let tgt = Language::from_639_3("rus").unwrap();
        seed_translation(&library, book_id, 0, &paragraph, tgt).await;

        let outcome = library
            .create_card_for_word(book_id, 0, 0, 0, tgt)
            .await
            .unwrap();
        assert_eq!(outcome, CardCreationOutcome::AlreadyExists);

        assert_eq!(deck(&library).await, vec!["poder".to_string()]);
        assert_eq!(
            stored_card(&library, "poder").await,
            Some(existing),
            "the existing card is not given a new example"
        );
    }

    #[tokio::test]
    async fn create_card_for_word_persists_noisy_pos() {
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

        let tgt = Language::from_639_3("rus").unwrap();
        seed_translation(&library, book_id, 0, &paragraph, tgt).await;
        library
            .create_card_for_word(book_id, 0, 0, 0, tgt)
            .await
            .unwrap();
        library
            .create_card_for_word(book_id, 0, 0, 1, tgt)
            .await
            .unwrap();

        assert_eq!(
            deck(&library).await,
            vec!["good".to_string(), "judge".to_string()],
            "cards are keyed by lemma alone"
        );

        // The noisy PoS strings survive as translations-map keys.
        let good = stored_card(&library, "good").await.unwrap();
        assert!(
            good.translations
                .keys()
                .any(|k| k.contains("существительное")),
            "expected noisy PoS as a key inside translations, got {:?}",
            good.translations.keys().collect::<Vec<_>>()
        );
        let judge = stored_card(&library, "judge").await.unwrap();
        assert!(
            judge.translations.keys().any(|k| k.contains("глагол")),
            "expected noisy PoS as a key inside translations, got {:?}",
            judge.translations.keys().collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn create_card_for_word_files_the_card_under_its_language_pair() {
        let tmp = TempDir::new("flts_card_pair");
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
        let tgt = Language::from_639_3("rus").unwrap();
        seed_translation(&library, book_id, 0, &paragraph, tgt).await;
        library
            .create_card_for_word(book_id, 0, 0, 0, tgt)
            .await
            .unwrap();
        assert_eq!(
            library.card_store().list_pairs().await.unwrap(),
            [("spa".to_string(), "rus".to_string())]
        );
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
            library.cached_books().await.len(),
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
        let book = library.get_book(&book_id).await.unwrap();
        let paragraph = paragraph.clone();
        book.modify(move |book| {
            book.get_or_create_translation(&target_language)
                .add_paragraph_translation(paragraph_id, &paragraph, "models/gemini-2.5-flash");
        })
        .await
        .unwrap();
        book.flush().await.unwrap();
    }
}
