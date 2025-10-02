use std::time::SystemTime;

use uuid::Uuid;
use vfs::VfsPath;

use crate::{
    book::{book::Book, serialization::Serializable, translation::Translation},
    library::Library,
};

pub struct LibraryBook {
    path: VfsPath,
    last_modified: Option<SystemTime>,
    book: Book,
    translations: Vec<LibraryTranslation>,
}

pub struct LibraryTranslation {
    translation: Translation,
    last_modified: Option<SystemTime>,
}

impl LibraryBook {
    pub fn get_or_create_translation(
        &mut self,
        source_language: &str,
        target_language: &str,
    ) -> &Translation {
        if let Some(idx) = self.translations.iter().position(|t| {
            t.translation.source_language == source_language
                && t.translation.target_language == target_language
        }) {
            return &self.translations[idx].translation;
        }

        // Not found: create and push
        self.translations.push(LibraryTranslation {
            translation: Translation::create(source_language, target_language),
            last_modified: None,
        });

        let last = self.translations.len() - 1;
        &self.translations[last].translation
    }

    pub fn load(path: &VfsPath) -> Result<Self, vfs::error::VfsError> {
        todo!()
    }

    pub fn save(&self) -> Result<(), vfs::error::VfsError> {
        if !self.path.exists()? {
            self.path.create_dir()?
        }

        let book_path = self.path.join("book.dat")?;
        if let Some(last_modified) = self.last_modified {
            if book_path.exists()? {
                let saved_book_last_modified = book_path.metadata()?.modified.unwrap();
                if saved_book_last_modified > last_modified {
                    todo!("Implement book data merging");
                }
            }
        } else if book_path.exists()? {
            todo!("Implement book data merging");
        }

        let book_path_temp = self.path.join("book.dat~")?;
        let mut file = book_path_temp.create_file()?;
        self.book.serialize(&mut file)?;

        book_path_temp.move_file(&book_path)?; // TODO verify modified date

        for translation in &self.translations {
            let translation_file_name = format!(
                "translation_{}_{}.dat",
                translation.translation.source_language, translation.translation.target_language
            );
            let translation_path = self.path.join(&translation_file_name)?;

            if let Some(last_modified) = translation.last_modified {
                if translation_path.exists()? {
                    let saved_translation_last_modified =
                        translation_path.metadata()?.modified.unwrap();
                    if saved_translation_last_modified > last_modified {
                        todo!("Implement translation data merging");
                    }
                }
            } else if translation_path.exists()? {
                todo!("Implement translation data merging");
            }

            let translation_path_temp = self.path.join(format!("{translation_file_name}~"))?;

            let mut translation_file = translation_path_temp.create_file()?;
            translation.translation.serialize(&mut translation_file)?;

            translation_path_temp.move_file(&translation_path)?; // TODO verify modified data
        }

        Ok(())
    }
}

impl Library {
    pub fn create_book(&self, title: &str) -> Result<LibraryBook, vfs::error::VfsError> {
        let guid = Uuid::new_v4();
        let book_root = self.library_root.join(guid.to_string())?;

        Ok(LibraryBook {
            path: book_root,
            last_modified: None,
            book: Book::create(title),
            translations: vec![],
        })
    }
}

#[cfg(test)]
mod library_book_tests {
    use vfs::VfsPath;

    use crate::library::Library;

    #[test]
    fn list_books_conflicting_versions() {
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let library = Library::open(library_path.clone()).unwrap();

        let book1 = library.create_book("First Book").unwrap();
        book1.save().unwrap();

        let book_file = book1.path.join("book.dat").unwrap();

        let conflict_path = book1
            .path
            .join(
                book_file
                    .filename()
                    .replace(".dat", ".syncconflict-foobar.dat"),
            )
            .unwrap();

        book_file.copy_file(&conflict_path).unwrap();

        let library_books = library.list_books().unwrap();

        assert_eq!(library_books.len(), 1);

        assert_eq!(library_books[0].conflicting_paths.len(), 1);
        assert_eq!(library_books[0].conflicting_paths[0].filename(), conflict_path.filename());
    }

    #[test]
    fn list_books_conflicting_translation_versions() {
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let library = Library::open(library_path.clone()).unwrap();

        let mut book1 = library.create_book("First Book").unwrap();
        let _translation = book1.get_or_create_translation("es", "en");
        book1.save().unwrap();

        let translation_file = book1.path.join("translation_es_en.dat").unwrap();

        let conflict_path = book1
            .path
            .join(
                translation_file
                    .filename()
                    .replace(".dat", ".syncconflict-foobar.dat"),
            )
            .unwrap();

        translation_file.copy_file(&conflict_path).unwrap();

        let library_books = library.list_books().unwrap();

        assert_eq!(library_books[0].translations_metadata.len(), 1);
        assert_eq!(library_books[0].translations_metadata[0].main_path.filename(), translation_file.filename());
        assert_eq!(library_books[0].translations_metadata[0].conflicting_paths.len(), 1);
        assert_eq!(library_books[0].translations_metadata[0].conflicting_paths[0].filename(), conflict_path.filename());
    }
}
