use std::{
    error::Error,
    fmt::Display,
    path::{Path, PathBuf},
    sync::Arc,
};

use isolang::Language;
use itertools::Itertools;
use log::{info, trace};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use crate::{
    book::{book_metadata::BookMetadata, translation_metadata::TranslationMetadata},
    cache::WeakLruCache,
    epub_importer::EpubBook,
    library::{
        file_watcher::LibraryFileChange,
        library_book::{LibraryBook, load_book_user_state},
        library_dictionary::DictionaryCache,
    },
    tla_trace::mutex::TracedMutex,
};

pub mod file_watcher;
pub mod library_book;
pub mod library_dictionary;

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
        })
    }
}

pub struct Library {
    library_root: PathBuf,
    pub(crate) books_cache: WeakLruCache<Uuid, TracedMutex<LibraryBook>>,
    dictionaries_cache: Arc<DictionaryCache>,
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

        let dictionaries_cache = Arc::new(DictionaryCache::new(&library_root));

        Ok(Library {
            library_root,
            books_cache: WeakLruCache::new(cache_capacity),
            dictionaries_cache,
        })
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
            LibraryBook::load_from_metadata(self.dictionaries_cache.clone(), metadata).await?,
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
            LibraryFileChange::DictionaryChanged { modified, from, to } => {
                self.dictionaries_cache
                    .reload_dictionary(*modified, *from, *to)
                    .await?
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
    use crate::test_utils::TempDir;

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
}
