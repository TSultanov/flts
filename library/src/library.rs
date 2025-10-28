use std::{collections::HashMap, error::Error, fmt::Display, sync::Arc};

use isolang::Language;
use itertools::Itertools;
use tokio::sync::Mutex;
use uuid::Uuid;
use vfs::{VfsError, VfsPath};

use crate::{
    book::{book_metadata::BookMetadata, translation_metadata::TranslationMetadata},
    epub_importer::EpubBook,
    library::{library_book::LibraryBook, library_dictionary::DictionaryCache},
};

pub mod library_book;
pub mod library_dictionary;

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
    pub main_path: VfsPath,
    pub conflicting_paths: Vec<VfsPath>,
}

pub struct LibraryBookMetadata {
    pub id: Uuid,
    pub title: String,
    pub main_path: VfsPath,
    pub conflicting_paths: Vec<VfsPath>,
    pub chapters_count: usize,
    pub paragraphs_count: usize,
    pub translations_metadata: Vec<LibraryTranslationMetadata>,
}

impl LibraryBookMetadata {
    pub fn load(path: &VfsPath) -> Result<Self, VfsError> {
        let book_dat = path.join("book.dat")?;
        let mut book_dat_file = book_dat.open_file()?;
        let book_metadata = BookMetadata::read_metadata(&mut book_dat_file)?;

        let conflicting_paths = {
            let conflicting_paths = path.read_dir()?.filter(|d| {
                d.filename().starts_with("book")
                    && d.filename().ends_with(".dat")
                    && d.filename() != "book.dat"
            });

            let mut result = Vec::new();

            for path in conflicting_paths {
                let mut file = path.open_file()?;
                let metadata = BookMetadata::read_metadata(&mut file);
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

        let book_dir_content = path.read_dir()?;

        for file in book_dir_content {
            if file.is_file()?
                && file.filename().starts_with("translation_")
                && file.filename().ends_with(".dat")
            {
                let mut data = file.open_file()?;
                let metadata = TranslationMetadata::read_metadata(&mut data)?;
                all_translations.push((file, metadata));
            }
        }

        let grouped_translations = all_translations
            .into_iter()
            .chunk_by(|(_, translation)| translation.id);
        let grouped_translations = grouped_translations
            .into_iter()
            .map(|(id, chunk)| (id, chunk.sorted_by_key(|(p, _)| p.filename().len())));

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

        Ok(LibraryBookMetadata {
            id: book_metadata.id,
            title: book_metadata.title,
            main_path: book_dat,
            conflicting_paths,
            chapters_count: book_metadata.chapters_count,
            paragraphs_count: book_metadata.paragraphs_count,
            translations_metadata,
        })
    }
}

pub struct Library {
    library_root: VfsPath,
    books_cache: HashMap<Uuid, Arc<Mutex<LibraryBook>>>, // TODO: eviction
    dictionaries_cache: Arc<Mutex<DictionaryCache>>,
}

impl Library {
    pub fn open(library_root: VfsPath) -> Result<Self, vfs::error::VfsError> {
        if !library_root.exists()? {
            library_root.create_dir()?
        }

        let dictionaries_cache = Arc::new(Mutex::new(DictionaryCache::new(&library_root)));

        Ok(Library {
            library_root,
            books_cache: HashMap::new(),
            dictionaries_cache,
        })
    }

    pub fn list_books(&self) -> Result<Vec<LibraryBookMetadata>, vfs::error::VfsError> {
        let library_root_content = self.library_root.read_dir()?;

        let mut books = Vec::new();

        for path in library_root_content {
            if !path.is_dir()? {
                continue;
            }

            let book = LibraryBookMetadata::load(&path);
            match book {
                Ok(book) => books.push(book),
                Err(err) => {
                    println!("Failed to load book at path {:?}: error {}", path, err)
                } // TODO logging
            };
        }

        Ok(books)
    }

    pub fn get_book(&mut self, uuid: &Uuid) -> anyhow::Result<Arc<Mutex<LibraryBook>>> {
        if let Some(book) = self.books_cache.get(uuid) {
            return Ok(book.clone());
        }

        let path = &self.library_root.join(uuid.to_string())?;
        let metadata = LibraryBookMetadata::load(path)?;
        let book = Arc::new(Mutex::new(LibraryBook::load_from_metadata(
            self.dictionaries_cache.clone(),
            metadata,
        )?));

        self.books_cache.insert(*uuid, book.clone());
        Ok(book)
    }

    pub async fn create_book_plain(&mut self, title: &str, text: &str, language: &Language) -> anyhow::Result<Uuid> {
        let book = self.create_book(title, language)?;
        let mut book = book.lock().await;
        let chapter_index = book.book.push_chapter(None);
        let paragraphs = split_paragraphs(text);

        for paragraph in paragraphs {
            book.book.push_paragraph(chapter_index, paragraph, None);
        }

        book.save().await?;

        Ok(book.book.id)
    }

    pub async fn create_book_epub(&mut self, epub: &EpubBook, language: &Language) -> anyhow::Result<Uuid> {
        let book = self.create_book(&epub.title, language)?;
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
}

fn split_paragraphs(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|p| !p.is_empty())
}

#[cfg(test)]
mod library_tests {
    use super::*;

    #[test]
    fn library_open_newdirectory() {
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("test").unwrap();
        _ = Library::open(library_path);

        let root_directories = root.read_dir().unwrap().collect::<Vec<_>>();
        assert_eq!(root_directories, vec![root.join("test").unwrap()]);
    }

    #[test]
    fn list_books_empty_library() {
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let library = Library::open(library_path).unwrap();

        let books = library.list_books().unwrap();
        assert!(books.is_empty(), "Expected no books, got {:?}", books.len());
    }

    #[tokio::test]
    async fn list_books_multiple_empty_books() {
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let mut library = Library::open(library_path.clone()).unwrap();

        let book1 = library.create_book("First Book", &Language::from_639_3("eng").unwrap()).unwrap();
        book1.lock().await.save().await.unwrap();
        let book2 = library.create_book("Second Book", &Language::from_639_3("eng").unwrap()).unwrap();
        book2.lock().await.save().await.unwrap();

        let mut books = library.list_books().unwrap();
        assert_eq!(books.len(), 2);
        books.sort_by(|a, b| a.title.cmp(&b.title));
        assert_eq!(books[0].title, "First Book");
        assert_eq!(books[0].paragraphs_count, 0);
        assert!(books[0].translations_metadata.is_empty());
        assert_eq!(books[1].title, "Second Book");
        assert_eq!(books[1].paragraphs_count, 0);
        assert!(books[1].translations_metadata.is_empty());
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
}
