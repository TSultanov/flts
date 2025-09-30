use std::{fs, io, time::SystemTime};

use uuid::Uuid;
use vfs::{VfsError, VfsPath};

use crate::book::{
    book::Book, book_metadata::BookMetadata, serialization::Serializable, translation::Translation,
    translation_metadata::TranslationMetadata,
};

pub struct LibraryTranslationMetadata {
    pub source_langugage: String,
    pub target_language: String,
    pub translated_paragraphs_count: usize,
    pub last_modified: SystemTime,
}

pub struct LibraryBookMetadata {
    pub title: String,
    pub last_modified: SystemTime,
    pub paragraphs_count: usize,
    pub translations_metadata: Vec<LibraryTranslationMetadata>,
}

impl LibraryBookMetadata {
    pub fn load(path: &VfsPath) -> Result<Self, VfsError> {
        let book_dat = path.join("book.dat")?;
        let book_last_modified = book_dat.metadata()?.modified.unwrap();

        let mut book_dat_file = book_dat.open_file()?;

        let book_metadata = BookMetadata::read_metadata(&mut book_dat_file)?;

        let mut translations = Vec::new();

        let book_dir_content = path.read_dir()?;
        for file in book_dir_content {
            if file.is_file()?
                && file.filename().starts_with("translation_")
                && file.filename().ends_with(".dat")
            // TODO: handle conflicting versions
            {
                let last_modified = file.metadata()?.modified.unwrap();
                let mut data = file.open_file()?;
                let metadata = TranslationMetadata::read_metadata(&mut data)?;
                translations.push(LibraryTranslationMetadata {
                    source_langugage: metadata.source_language,
                    target_language: metadata.target_language,
                    translated_paragraphs_count: metadata.translated_paragraphs_count,
                    last_modified: last_modified,
                });
            }
        }

        Ok(LibraryBookMetadata {
            title: book_metadata.title,
            last_modified: book_last_modified,
            paragraphs_count: book_metadata.paragraphs_count,
            translations_metadata: translations,
        })
    }
}

pub struct Library {
    library_root: VfsPath,
}

impl Library {
    pub fn open(library_root: VfsPath) -> Result<Self, vfs::error::VfsError> {
        if !library_root.exists()? {
            library_root.create_dir()?
        }

        Ok(Library { library_root })
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
                    println!(
                        "Failed to load book at path {:?}: error {}",
                        path,
                        err.to_string()
                    )
                } // TODO logging
            };
        }

        Ok(books)
    }

    pub fn create_book(&self, title: &str) -> Result<LibraryBook, vfs::error::VfsError> {
        let guid = Uuid::new_v4();
        let book_root = self.library_root.join(guid.to_string())?;

        Ok(LibraryBook {
            path: book_root,
            book: Book::create(title),
            translations: vec![],
        })
    }
}

pub struct LibraryBook {
    path: VfsPath,
    book: Book,
    translations: Vec<Translation>,
}

impl LibraryBook {
    pub fn load(path: &VfsPath) -> Result<Self, vfs::error::VfsError> {
        todo!()
    }

    pub fn save(&self) -> Result<(), vfs::error::VfsError> {
        if !self.path.exists()? {
            self.path.create_dir()?
        }
        let book_path_temp = self.path.join("book.dat~")?;
        let mut file = book_path_temp.create_file()?;
        self.book.serialize(&mut file)?;
        let book_path = self.path.join("book.dat")?;
        book_path_temp.move_file(&book_path)?; // TODO verify modified date

        for translation in &self.translations {
            let translation_file_name = format!(
                "translation_{}_{}.dat",
                translation.source_language, translation.target_language
            );
            let translation_path_temp = self.path.join(format!("{translation_file_name}~"))?;

            let mut translation_file = translation_path_temp.create_file()?;
            translation.serialize(&mut translation_file)?;

            let translation_path = self.path.join(translation_file_name)?;
            translation_path_temp.move_file(&translation_path)?; // TODO verify modified data
        }

        Ok(())
    }
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

    #[test]
    fn list_books_multiple_empty_books() {
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let library = Library::open(library_path.clone()).unwrap();

        let book1 = library.create_book("First Book").unwrap();
        book1.save().unwrap();
        let book2 = library.create_book("Second Book").unwrap();
        book2.save().unwrap();

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
}
