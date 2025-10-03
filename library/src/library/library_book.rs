use std::{collections::HashSet, time::SystemTime};

use uuid::Uuid;
use vfs::VfsPath;

use crate::{
    book::{book::Book, serialization::Serializable, translation::Translation},
    library::{Library, LibraryBookMetadata, LibraryTranslationMetadata},
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

impl LibraryTranslation {
    fn merge(self, other: LibraryTranslation) -> LibraryTranslation {
        let merged_translation = self.translation.merge(other.translation);

        LibraryTranslation {
            translation: merged_translation,
            last_modified: self.last_modified.max(other.last_modified),
        }
    }

    fn load(path: &VfsPath) -> Result<Self, vfs::error::VfsError> {
        let last_modified = path.metadata()?.modified;
        let mut file = path.open_file()?;
        let translation = Translation::deserialize(&mut file)?;

        Ok(Self {
            translation,
            last_modified,
        })
    }

    fn load_from_metadata(
        metadata: LibraryTranslationMetadata,
    ) -> Result<Self, vfs::error::VfsError> {
        todo!("Merge!");

        Self::load(&metadata.main_path)
    }
}

impl LibraryBook {
    fn merge(self, other: LibraryBook) -> LibraryBook {
        let merged_book = self.book.merge(other.book);

        let other_translation_ids = other
            .translations
            .iter()
            .map(|t| t.translation.id)
            .collect::<HashSet<_>>();

        LibraryBook {
            path: self.path,
            last_modified: self.last_modified.max(other.last_modified),
            book: merged_book,
            translations: other
                .translations
                .into_iter()
                .chain(
                    self.translations
                        .into_iter()
                        .filter(|t| !other_translation_ids.contains(&t.translation.id)),
                )
                .collect(),
        }
    }

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

    pub fn load_from_metadata(metadata: LibraryBookMetadata) -> Result<Self, vfs::error::VfsError> {
        todo!("Merge on load!");

        Self::load(&metadata.main_path)
    }

    fn load(path: &VfsPath) -> Result<Self, vfs::error::VfsError> {
        let last_modified = path.metadata()?.modified;
        let mut file = path.open_file()?;
        let book = Book::deserialize(&mut file)?;

        Ok(Self {
            path: path.clone(),
            last_modified,
            book,
            translations: vec![],
        })
    }

    pub fn save(self) -> Result<Self, vfs::error::VfsError> {
        if !self.path.exists()? {
            self.path.create_dir()?
        }

        let mut book = self;

        let mut merged_translations = Vec::new();

        for mut translation in book.translations.drain(0..) {
            let translation_file_name = format!(
                "translation_{}_{}.dat",
                translation.translation.source_language, translation.translation.target_language
            );
            let translation_path = book.path.join(&translation_file_name)?;
            let translation_path_temp = book.path.join(format!("{translation_file_name}~"))?;

            loop {
                let translation_path_modified_pre_save = translation_path.metadata()?.modified;

                if let Some(last_modified) = translation.last_modified {
                    if translation_path.exists()? {
                        let saved_translation_last_modified =
                            translation_path.metadata()?.modified.unwrap();
                        if saved_translation_last_modified > last_modified {
                            let saved_translation = LibraryTranslation::load(&translation_path)?;
                            translation = translation.merge(saved_translation);
                        }
                    }
                } else if translation_path.exists()? {
                    let saved_translation = LibraryTranslation::load(&translation_path)?;
                    translation = translation.merge(saved_translation);
                }

                let mut translation_file = translation_path_temp.create_file()?;
                translation.translation.serialize(&mut translation_file)?;

                if translation_path.metadata()?.modified == translation_path_modified_pre_save {
                    translation_path_temp.move_file(&translation_path)?;
                    merged_translations.push(translation);
                    break;
                }
            }
        }

        let book_path = book.path.join("book.dat")?;
        let book_path_temp = book.path.join("book.dat~")?;
        loop {
            let book_path_modified_pre_save = book_path.metadata()?.modified;

            if let Some(last_modified) = book.last_modified {
                if book_path.exists()? {
                    let saved_book_last_modified = book_path.metadata()?.modified.unwrap();
                    if saved_book_last_modified > last_modified {
                        let saved_book = Self::load(&book_path)?;
                        book = book.merge(saved_book);
                    }
                }
            } else if book_path.exists()? {
                let saved_book = Self::load(&book_path)?;
                book = book.merge(saved_book);
            }

            let mut file = book_path_temp.create_file()?;
            book.book.serialize(&mut file)?;

            if book_path.metadata()?.modified == book_path_modified_pre_save {
                book_path_temp.move_file(&book_path)?;
                break;
            }
            // Attempt to merge and save again otherwise
        }

        let all_book_translations = LibraryBookMetadata::load(&book.path)?;
        let loaded_translations = merged_translations
            .iter()
            .map(|t| t.translation.id)
            .collect::<HashSet<_>>();
        for translation_metadata in all_book_translations.translations_metadata {
            if !loaded_translations.contains(&translation_metadata.id) {
                merged_translations.push(LibraryTranslation::load_from_metadata(
                    translation_metadata,
                )?);
            }
        }

        book.translations = merged_translations;

        Ok(book)
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
        let book1 = book1.save().unwrap();

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
        assert_eq!(
            library_books[0].conflicting_paths[0].filename(),
            conflict_path.filename()
        );
    }

    #[test]
    fn list_books_conflicting_translation_versions() {
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let library = Library::open(library_path.clone()).unwrap();

        let mut book1 = library.create_book("First Book").unwrap();
        let _translation = book1.get_or_create_translation("es", "en");
        let book1 = book1.save().unwrap();

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
        assert_eq!(
            library_books[0].translations_metadata[0]
                .main_path
                .filename(),
            translation_file.filename()
        );
        assert_eq!(
            library_books[0].translations_metadata[0]
                .conflicting_paths
                .len(),
            1
        );
        assert_eq!(
            library_books[0].translations_metadata[0].conflicting_paths[0].filename(),
            conflict_path.filename()
        );
    }
}
