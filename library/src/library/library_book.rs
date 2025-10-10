use std::{
    collections::HashSet,
    io::{BufReader, BufWriter},
    sync::Arc,
    time::SystemTime,
};

use tokio::sync::Mutex;
use uuid::Uuid;
use vfs::VfsPath;

use crate::{
    book::{book::Book, serialization::Serializable, translation::Translation},
    library::{Library, LibraryBookMetadata, LibraryError, LibraryTranslationMetadata},
};

pub struct LibraryBook {
    path: VfsPath,
    last_modified: Option<SystemTime>,
    pub book: Book,
    translations: Vec<LibraryTranslation>,
}

pub struct LibraryTranslation {
    translation: Arc<Mutex<Translation>>,
    last_modified: Option<SystemTime>,
}

impl LibraryTranslation {
    async fn merge(self, other: LibraryTranslation) -> LibraryTranslation {
        let other_t = other.translation.lock().await;

        let merged_translation =
            Arc::new(Mutex::new(self.translation.lock().await.merge(&other_t)));

        LibraryTranslation {
            translation: merged_translation,
            last_modified: self.last_modified.max(other.last_modified),
        }
    }

    fn load(path: &VfsPath) -> Result<Self, vfs::error::VfsError> {
        let last_modified = path.metadata()?.modified;
        let mut file = BufReader::new(path.open_file()?);
        let translation = Arc::new(Mutex::new(Translation::deserialize(&mut file)?));

        Ok(Self {
            translation,
            last_modified,
        })
    }

    fn load_from_metadata(
        metadata: LibraryTranslationMetadata,
    ) -> Result<Self, vfs::error::VfsError> {
        if !metadata.conflicting_paths.is_empty() {
            let mut translation = {
                let mut main_file = BufReader::new(metadata.main_path.open_file()?);
                Translation::deserialize(&mut main_file)?
            };

            for conflict in metadata.conflicting_paths {
                let mut conflict_file = BufReader::new(conflict.open_file()?);
                let conflict_translation = Translation::deserialize(&mut conflict_file)?;
                translation = translation.merge(&conflict_translation);
            }

            let mut main_file = metadata.main_path.create_file()?;
            translation.serialize(&mut main_file)?;
        }

        Self::load(&metadata.main_path)
    }
}

impl LibraryBook {
    pub async fn get_or_create_translation(
        &mut self,
        source_language: &str,
        target_language: &str,
    ) -> Arc<Mutex<Translation>> {
        for (t_idx, t) in self.translations.iter().enumerate() {
            if t.translation.lock().await.source_language == source_language
                && t.translation.lock().await.target_language == target_language
            {
                return self.translations[t_idx].translation.clone();
            }
        }

        // Not found: create and push
        self.translations.push(LibraryTranslation {
            translation: Arc::new(Mutex::new(Translation::create(
                source_language,
                target_language,
            ))),
            last_modified: None,
        });

        let last = self.translations.len() - 1;
        self.translations[last].translation.clone()
    }

    pub fn load_from_metadata(metadata: LibraryBookMetadata) -> Result<Self, vfs::error::VfsError> {
        let mut candidates: Vec<(&VfsPath, Option<SystemTime>)> = Vec::new();
        candidates.push((&metadata.main_path, metadata.main_path.metadata()?.modified));
        for p in &metadata.conflicting_paths {
            candidates.push((p, p.metadata()?.modified));
        }

        let mut newest_idx = 0usize;
        let mut newest_time = candidates[0].1.unwrap_or(SystemTime::UNIX_EPOCH);
        for (i, (_, m)) in candidates.iter().enumerate().skip(1) {
            if m.unwrap_or(SystemTime::UNIX_EPOCH) > newest_time {
                newest_idx = i;
                newest_time = m.unwrap_or(SystemTime::UNIX_EPOCH);
            }
        }

        if newest_idx != 0 {
            if metadata.main_path.exists()? {
                metadata.main_path.remove_file()?;
            }
            let source = &candidates[newest_idx].0;
            if source.exists()? {
                source.move_file(&metadata.main_path)?;
            }
        }

        for p in metadata.conflicting_paths {
            if p.exists()? {
                // It's possible we've just moved the newest conflict into main, so ignore missing
                let _ = p.remove_file();
            }
        }

        let mut book = Self::load(&metadata.main_path)?;

        for tm in metadata.translations_metadata {
            let translation = LibraryTranslation::load_from_metadata(tm)?;
            book.translations.push(translation);
        }

        Ok(book)
    }

    fn load(path: &VfsPath) -> Result<Self, vfs::error::VfsError> {
        let last_modified = path.metadata()?.modified;
        let mut file = BufReader::new(path.open_file()?);
        let book = Book::deserialize(&mut file)?;

        Ok(Self {
            path: path.parent(),
            last_modified,
            book,
            translations: vec![],
        })
    }

    pub async fn save(&mut self) -> Result<(), vfs::error::VfsError> {
        if !self.path.exists()? {
            self.path.create_dir()?
        }

        let get_modified_if_exists = |path: &VfsPath| {
            if path.exists()? {
                Ok::<_, vfs::error::VfsError>(path.metadata()?.modified)
            } else {
                Ok(None)
            }
        };

        let book = self;

        let mut merged_translations = Vec::new();

        for mut translation in book.translations.drain(0..) {
            let source_language = translation.translation.lock().await.source_language.clone();
            let target_language = translation.translation.lock().await.target_language.clone();
            let translation_file_name =
                format!("translation_{}_{}.dat", source_language, target_language);
            let translation_path = book.path.join(&translation_file_name)?;
            let translation_path_temp = book.path.join(format!("{translation_file_name}~"))?;

            loop {
                let translation_path_modified_pre_save = get_modified_if_exists(&translation_path)?;

                if let Some(last_modified) = translation.last_modified {
                    if translation_path.exists()? {
                        let saved_translation_last_modified =
                            translation_path.metadata()?.modified.unwrap();
                        if saved_translation_last_modified > last_modified {
                            let saved_translation = LibraryTranslation::load(&translation_path)?;
                            translation = translation.merge(saved_translation).await;
                        }
                    }
                } else if translation_path.exists()? {
                    let saved_translation = LibraryTranslation::load(&translation_path)?;
                    translation = translation.merge(saved_translation).await;
                }

                let mut translation_file = BufWriter::new(translation_path_temp.create_file()?);
                translation
                    .translation
                    .lock()
                    .await
                    .serialize(&mut translation_file)?;

                if get_modified_if_exists(&translation_path)? == translation_path_modified_pre_save
                    || translation_path_modified_pre_save.is_none()
                {
                    if translation_path.exists()? {
                        translation_path.remove_file()?;
                    }
                    translation_path_temp.move_file(&translation_path)?;
                    merged_translations.push(translation);
                    break;
                }
            }
        }

        let book_path = book.path.join("book.dat")?;
        let book_path_temp = book.path.join("book.dat~")?;
        loop {
            let book_path_modified_pre_save = get_modified_if_exists(&book_path)?;

            if let Some(last_modified) = book.last_modified {
                if book_path.exists()? {
                    let saved_book_last_modified = book_path.metadata()?.modified.unwrap();
                    if saved_book_last_modified > last_modified {
                        let saved_book = Self::load(&book_path)?;
                        book.book = saved_book.book;
                        book.last_modified = saved_book.last_modified;
                    }
                }
            } else if book_path.exists()? {
                let saved_book = Self::load(&book_path)?;
                book.book = saved_book.book;
                book.last_modified = saved_book.last_modified;
            }

            let mut file = BufWriter::new(book_path_temp.create_file()?);
            book.book.serialize(&mut file)?;

            if get_modified_if_exists(&book_path)? == book_path_modified_pre_save
                || book_path_modified_pre_save.is_none()
            {
                if book_path.exists()? {
                    book_path.remove_file()?;
                }
                book_path_temp.move_file(&book_path)?;
                break;
            }
            // Attempt to merge and save again otherwise
        }

        let all_book_translations = LibraryBookMetadata::load(&book.path)?;
        let mut loaded_translations = HashSet::new();
        for t in &merged_translations {
            loaded_translations.insert(t.translation.lock().await.id);
        }

        for translation_metadata in all_book_translations.translations_metadata {
            if !loaded_translations.contains(&translation_metadata.id) {
                merged_translations.push(LibraryTranslation::load_from_metadata(
                    translation_metadata,
                )?);
            }
        }

        book.translations = merged_translations;

        Ok(())
    }
}

impl Library {
    pub fn create_book(&self, title: &str) -> anyhow::Result<LibraryBook> {
        let books = self.list_books()?;
        if books.iter().any(|b| b.title == title) {
            Err(LibraryError::DuplicateTitle(title.to_owned()))?
        }

        let guid = Uuid::new_v4();
        let book_root = self.library_root.join(guid.to_string())?;

        Ok(LibraryBook {
            path: book_root,
            last_modified: None,
            book: Book::create(guid, title),
            translations: vec![],
        })
    }
}

#[cfg(test)]
mod library_book_tests {
    use std::{cell::RefCell, rc::Rc, sync::Arc};

    use tokio::sync::Mutex;
    use vfs::VfsPath;

    use crate::{
        book::{
            book::Book, serialization::Serializable, translation::Translation, translation_import,
        },
        library::{Library, LibraryTranslationMetadata},
    };

    #[tokio::test]
    async fn list_books_conflicting_versions() {
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let library = Library::open(library_path.clone()).unwrap();

        let mut book1 = library.create_book("First Book").unwrap();
        book1.save().await.unwrap();

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

    #[tokio::test]
    async fn list_books_conflicting_translation_versions() {
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let library = Library::open(library_path.clone()).unwrap();

        let mut book1 = library.create_book("First Book").unwrap();
        let _translation = book1.get_or_create_translation("es", "en").await;
        book1.save().await.unwrap();

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

    #[tokio::test]
    async fn save_after_load_trivial_book_change() {
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let library = Library::open(library_path.clone()).unwrap();

        // Create and save
        let mut book = library.create_book("First Title").unwrap();
        book.save().await.unwrap();

        // Simulate "loaded": set last_modified from disk
        let book_file = book.path.join("book.dat").unwrap();
        book.last_modified = book_file.metadata().unwrap().modified;

        // Change and save again
        book.book.title = "Updated Title".into();
        book.save().await.unwrap();

        // Verify on-disk
        let mut f = book_file.open_file().unwrap();
        let loaded_book = Book::deserialize(&mut f).unwrap();
        assert_eq!(loaded_book.title, "Updated Title");
    }

    #[tokio::test]
    async fn save_after_load_book_and_translation_changed() {
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let library = Library::open(library_path.clone()).unwrap();

        // Create a book and attach a translation with an initial version
        let mut book = library.create_book("First Book").unwrap();
        let mut tr = Translation::create("es", "en");
        let initial_pt = translation_import::ParagraphTranslation {
            timestamp: 1,
            source_language: "es".to_owned(),
            target_language: "en".to_owned(),
            sentences: vec![translation_import::Sentence {
                full_translation: "Hola".into(),
                words: vec![translation_import::Word {
                    original: "Hola".into(),
                    contextual_translations: vec!["Hello".into()],
                    note: String::new(),
                    is_punctuation: false,
                    grammar: translation_import::Grammar {
                        original_initial_form: "hola".into(),
                        target_initial_form: "hello".into(),
                        part_of_speech: "interj".into(),
                        plurality: None,
                        person: None,
                        tense: None,
                        case: None,
                        other: None,
                    },
                }],
            }],
        };
        tr.add_paragraph_translation(0, &initial_pt);
        book.translations.push(super::LibraryTranslation {
            translation: Arc::new(Mutex::new(tr)),
            last_modified: None,
        });
        book.save().await.unwrap();

        // Treat as loaded: refresh last_modified and translations from disk
        let book_file = book.path.join("book.dat").unwrap();
        let tr_file = book.path.join("translation_es_en.dat").unwrap();
        book.last_modified = book_file.metadata().unwrap().modified;
        book.translations.clear();
        let loaded_tr = super::LibraryTranslation::load(&tr_file).unwrap();
        book.translations.push(loaded_tr);

        // Modify both book and translation
        book.book.title = "Second Edition".into();
        let new_pt = translation_import::ParagraphTranslation {
            timestamp: 2,
            source_language: "es".to_owned(),
            target_language: "en".to_owned(),
            sentences: vec![translation_import::Sentence {
                full_translation: "Hola mundo".into(),
                words: vec![translation_import::Word {
                    original: "Hola".into(),
                    contextual_translations: vec!["Hello".into()],
                    note: String::new(),
                    is_punctuation: false,
                    grammar: translation_import::Grammar {
                        original_initial_form: "hola".into(),
                        target_initial_form: "hello".into(),
                        part_of_speech: "interj".into(),
                        plurality: None,
                        person: None,
                        tense: None,
                        case: None,
                        other: None,
                    },
                }],
            }],
        };
        book.translations[0]
            .translation
            .lock()
            .await
            .add_paragraph_translation(0, &new_pt);

        let _saved = book.save().await.unwrap();

        // Verify book updated
        let mut bf = book_file.open_file().unwrap();
        let loaded_book = Book::deserialize(&mut bf).unwrap();
        assert_eq!(loaded_book.title, "Second Edition");

        // Verify translation latest version
        let mut tf = tr_file.open_file().unwrap();
        let tr2 = Translation::deserialize(&mut tf).unwrap();
        let latest = tr2.paragraph_view(0).unwrap();
        assert_eq!(latest.timestamp, 2);
        assert_eq!(latest.sentence_view(0).full_translation, "Hola mundo");
    }

    #[tokio::test]
    async fn save_merges_translation_with_concurrent_on_disk_change() {
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let library = Library::open(library_path.clone()).unwrap();

        // Create a book with a translation ts=1
        let mut book = library.create_book("Merge Book").unwrap();
        let mut tr = Translation::create("en", "ru");
        let pt1 = translation_import::ParagraphTranslation {
            timestamp: 1,
            source_language: "en".to_owned(),
            target_language: "ru".to_owned(),
            sentences: vec![translation_import::Sentence {
                full_translation: "v1".into(),
                words: vec![translation_import::Word {
                    original: "v1".into(),
                    contextual_translations: vec!["v1".into()],
                    note: String::new(),
                    is_punctuation: false,
                    grammar: translation_import::Grammar {
                        original_initial_form: "v1".into(),
                        target_initial_form: "v1".into(),
                        part_of_speech: "n".into(),
                        plurality: None,
                        person: None,
                        tense: None,
                        case: None,
                        other: None,
                    },
                }],
            }],
        };
        tr.add_paragraph_translation(0, &pt1);
        book.translations.push(super::LibraryTranslation {
            translation: Arc::new(Mutex::new(tr)),
            last_modified: None,
        });
        book.save().await.unwrap();

        // Treat as loaded instance with last_modified
        let book_file = book.path.join("book.dat").unwrap();
        let tr_path = book.path.join("translation_en_ru.dat").unwrap();
        book.last_modified = book_file.metadata().unwrap().modified;
        book.translations.clear();
        let loaded_tr = super::LibraryTranslation::load(&tr_path).unwrap();
        book.translations.push(loaded_tr);

        // In-memory change ts=2
        let mem_pt = translation_import::ParagraphTranslation {
            timestamp: 2,
            source_language: "en".to_owned(),
            target_language: "ru".to_owned(),
            sentences: vec![translation_import::Sentence {
                full_translation: "mem".into(),
                words: vec![translation_import::Word {
                    original: "mem".into(),
                    contextual_translations: vec!["mem".into()],
                    note: String::new(),
                    is_punctuation: false,
                    grammar: translation_import::Grammar {
                        original_initial_form: "mem".into(),
                        target_initial_form: "mem".into(),
                        part_of_speech: "n".into(),
                        plurality: None,
                        person: None,
                        tense: None,
                        case: None,
                        other: None,
                    },
                }],
            }],
        };
        book.translations[0]
            .translation
            .lock()
            .await
            .add_paragraph_translation(0, &mem_pt);

        // Concurrent on-disk change ts=3
        {
            let mut on_disk = {
                let mut f = tr_path.open_file().unwrap();
                Translation::deserialize(&mut f).unwrap()
            };
            let disk_pt = translation_import::ParagraphTranslation {
                timestamp: 3,
                source_language: "en".to_owned(),
                target_language: "ru".to_owned(),
                sentences: vec![translation_import::Sentence {
                    full_translation: "disk".into(),
                    words: vec![translation_import::Word {
                        original: "disk".into(),
                        contextual_translations: vec!["disk".into()],
                        note: String::new(),
                        is_punctuation: false,
                        grammar: translation_import::Grammar {
                            original_initial_form: "disk".into(),
                            target_initial_form: "disk".into(),
                            part_of_speech: "n".into(),
                            plurality: None,
                            person: None,
                            tense: None,
                            case: None,
                            other: None,
                        },
                    }],
                }],
            };
            on_disk.add_paragraph_translation(0, &disk_pt);
            let mut wf = tr_path.create_file().unwrap();
            on_disk.serialize(&mut wf).unwrap();
        }

        // Save should merge: latest ts=3 -> ts=2 -> ts=1
        let _merged = book.save().await.unwrap();
        let mut tf = tr_path.open_file().unwrap();
        let merged_tr = Translation::deserialize(&mut tf).unwrap();
        let latest = merged_tr.paragraph_view(0).unwrap();
        assert_eq!(latest.timestamp, 3);
        assert_eq!(latest.sentence_view(0).full_translation, "disk");
        let prev = latest.get_previous_version().unwrap();
        assert_eq!(prev.timestamp, 2);
        assert_eq!(prev.sentence_view(0).full_translation, "mem");
        let prev2 = prev.get_previous_version().unwrap();
        assert_eq!(prev2.timestamp, 1);
        assert_eq!(prev2.sentence_view(0).full_translation, "v1");
        assert!(prev2.get_previous_version().is_none());
    }

    #[tokio::test]
    async fn load_from_metadata_no_conflicts() {
        // Arrange: create a single main translation file with a simple history
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let dir = root.join("book").unwrap();
        dir.create_dir().unwrap();

        let main_path = dir.join("translation_en_ru.dat").unwrap();
        let mut t_main = Translation::create("en", "ru");
        let pt2 = translation_import::ParagraphTranslation {
            timestamp: 2,
            source_language: "en".to_owned(),
            target_language: "ru".to_owned(),
            sentences: vec![translation_import::Sentence {
                full_translation: "m2".into(),
                words: vec![translation_import::Word {
                    original: "m2".into(),
                    contextual_translations: vec!["m2".into()],
                    note: String::new(),
                    is_punctuation: false,
                    grammar: translation_import::Grammar {
                        original_initial_form: "m2".into(),
                        target_initial_form: "m2".into(),
                        part_of_speech: "n".into(),
                        plurality: None,
                        person: None,
                        tense: None,
                        case: None,
                        other: None,
                    },
                }],
            }],
        };
        t_main.add_paragraph_translation(0, &pt2);
        {
            let mut f = main_path.create_file().unwrap();
            t_main.serialize(&mut f).unwrap();
        }

        let meta = LibraryTranslationMetadata {
            id: t_main.id,
            source_langugage: "en".into(),
            target_language: "ru".into(),
            translated_paragraphs_count: 1,
            main_path: main_path.clone(),
            conflicting_paths: vec![],
        };

        // Act
        let loaded = super::LibraryTranslation::load_from_metadata(meta).unwrap();

        // Assert: translation loaded and unchanged, latest ts=2
        let translation_ref = loaded.translation.lock().await;
        let latest = translation_ref.paragraph_view(0).unwrap();
        assert_eq!(latest.timestamp, 2);
        assert_eq!(latest.sentence_view(0).full_translation, "m2");
    }

    #[tokio::test]
    async fn load_from_metadata_merges_conflicts_and_persists() {
        // Arrange: create main + two conflict files with different timestamps
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let dir = root.join("book2").unwrap();
        dir.create_dir().unwrap();

        let main_path = dir.join("translation_en_ru.dat").unwrap();
        let conflict1 = dir.join("translation_en_ru.conflict1.dat").unwrap();
        let conflict2 = dir.join("translation_en_ru.conflict2.dat").unwrap();

        // main: ts=2
        let mut t_main = Translation::create("en", "ru");
        let pt2 = translation_import::ParagraphTranslation {
            timestamp: 2,
            source_language: "en".to_owned(),
            target_language: "ru".to_owned(),
            sentences: vec![translation_import::Sentence {
                full_translation: "m2".into(),
                words: vec![translation_import::Word {
                    original: "m2".into(),
                    contextual_translations: vec!["m2".into()],
                    note: String::new(),
                    is_punctuation: false,
                    grammar: translation_import::Grammar {
                        original_initial_form: "m2".into(),
                        target_initial_form: "m2".into(),
                        part_of_speech: "n".into(),
                        plurality: None,
                        person: None,
                        tense: None,
                        case: None,
                        other: None,
                    },
                }],
            }],
        };
        t_main.add_paragraph_translation(0, &pt2);
        {
            let mut f = main_path.create_file().unwrap();
            t_main.serialize(&mut f).unwrap();
        }

        // conflict1: ts=1
        let mut t_c1 = Translation::create("en", "ru");
        let pt1 = translation_import::ParagraphTranslation {
            timestamp: 1,
            source_language: "en".to_owned(),
            target_language: "ru".to_owned(),
            sentences: vec![translation_import::Sentence {
                full_translation: "c1".into(),
                words: vec![translation_import::Word {
                    original: "c1".into(),
                    contextual_translations: vec!["c1".into()],
                    note: String::new(),
                    is_punctuation: false,
                    grammar: translation_import::Grammar {
                        original_initial_form: "c1".into(),
                        target_initial_form: "c1".into(),
                        part_of_speech: "n".into(),
                        plurality: None,
                        person: None,
                        tense: None,
                        case: None,
                        other: None,
                    },
                }],
            }],
        };
        t_c1.add_paragraph_translation(0, &pt1);
        {
            let mut f = conflict1.create_file().unwrap();
            t_c1.serialize(&mut f).unwrap();
        }

        // conflict2: ts=3
        let mut t_c2 = Translation::create("en", "ru");
        let pt3 = translation_import::ParagraphTranslation {
            timestamp: 3,
            source_language: "en".to_owned(),
            target_language: "ru".to_owned(),
            sentences: vec![translation_import::Sentence {
                full_translation: "c3".into(),
                words: vec![translation_import::Word {
                    original: "c3".into(),
                    contextual_translations: vec!["c3".into()],
                    note: String::new(),
                    is_punctuation: false,
                    grammar: translation_import::Grammar {
                        original_initial_form: "c3".into(),
                        target_initial_form: "c3".into(),
                        part_of_speech: "n".into(),
                        plurality: None,
                        person: None,
                        tense: None,
                        case: None,
                        other: None,
                    },
                }],
            }],
        };
        t_c2.add_paragraph_translation(0, &pt3);
        {
            let mut f = conflict2.create_file().unwrap();
            t_c2.serialize(&mut f).unwrap();
        }

        let meta = LibraryTranslationMetadata {
            id: t_main.id,
            source_langugage: "en".into(),
            target_language: "ru".into(),
            translated_paragraphs_count: 1,
            main_path: main_path.clone(),
            conflicting_paths: vec![conflict1.clone(), conflict2.clone()],
        };

        // Act
        let loaded = super::LibraryTranslation::load_from_metadata(meta).unwrap();

        // Assert: merged order latest=3, then 2, then 1
        let translation_ref = loaded.translation.lock().await;
        let latest = translation_ref.paragraph_view(0).unwrap();
        assert_eq!(latest.timestamp, 3);
        assert_eq!(latest.sentence_view(0).full_translation, "c3");
        let prev = latest.get_previous_version().unwrap();
        assert_eq!(prev.timestamp, 2);
        assert_eq!(prev.sentence_view(0).full_translation, "m2");
        let prev2 = prev.get_previous_version().unwrap();
        assert_eq!(prev2.timestamp, 1);
        assert_eq!(prev2.sentence_view(0).full_translation, "c1");
        assert!(prev2.get_previous_version().is_none());

        // Also verify that the main file now contains the merged result (latest ts=3)
        let mut f = main_path.open_file().unwrap();
        let on_disk = Translation::deserialize(&mut f).unwrap();
        let on_disk_latest = on_disk.paragraph_view(0).unwrap();
        assert_eq!(on_disk_latest.timestamp, 3);
        assert_eq!(on_disk_latest.sentence_view(0).full_translation, "c3");
    }

    #[tokio::test]
    async fn library_book_load_from_metadata_no_conflicts() {
        // Arrange
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let library = Library::open(library_path.clone()).unwrap();

        let mut book = library.create_book("Original Title").unwrap();
        book.save().await.unwrap();

        // Acquire metadata for the only book
        let mut books = library.list_books().unwrap();
        assert_eq!(books.len(), 1);
        let meta = books.remove(0);
        assert!(meta.conflicting_paths.is_empty());

        // Act
        let loaded = super::LibraryBook::load_from_metadata(meta).unwrap();

        // Assert
        assert_eq!(loaded.book.title, "Original Title");
    }

    #[tokio::test]
    async fn library_book_load_from_metadata_selects_newest_conflict_and_cleans() {
        use std::{thread::sleep, time::Duration};

        // Arrange
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let library = Library::open(library_path.clone()).unwrap();

        let mut book = library.create_book("Main V1").unwrap();
        book.save().await.unwrap();

        let book_file = book.path.join("book.dat").unwrap();
        let conflict_path = book
            .path
            .join(
                book_file
                    .filename()
                    .replace(".dat", ".syncconflict-newer.dat"),
            )
            .unwrap();

        // Create conflict as a copy first (same id)
        book_file.copy_file(&conflict_path).unwrap();

        // Ensure timestamp difference and update conflict content to be "newer"
        sleep(Duration::from_millis(5));
        let mut rf = conflict_path.open_file().unwrap();
        let mut conflict_book = Book::deserialize(&mut rf).unwrap();
        conflict_book.title = "From Conflict".into();
        let mut wf = conflict_path.create_file().unwrap();
        conflict_book.serialize(&mut wf).unwrap();

        // Acquire metadata (should include the conflict)
        let mut books = library.list_books().unwrap();
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].conflicting_paths.len(), 1);
        let meta = books.remove(0);

        // Act: load should select the newest (conflict), move it to main, and delete conflicts
        let loaded = super::LibraryBook::load_from_metadata(meta).unwrap();

        // Assert: loaded content is from conflict (newest)
        assert_eq!(loaded.book.title, "From Conflict");
        // On-disk main should now contain the conflict content and conflict file should be gone
        let mut f = book_file.open_file().unwrap();
        let on_disk = Book::deserialize(&mut f).unwrap();
        assert_eq!(on_disk.title, "From Conflict");
        assert!(!conflict_path.exists().unwrap());
    }

    #[tokio::test]
    async fn library_book_load_from_metadata_keeps_main_if_newest_and_cleans() {
        use std::{thread::sleep, time::Duration};

        // Arrange
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let library = Library::open(library_path.clone()).unwrap();

        let mut book = library.create_book("V1").unwrap();
        book.save().await.unwrap();

        let book_file = book.path.join("book.dat").unwrap();
        let conflict_path = book
            .path
            .join(
                book_file
                    .filename()
                    .replace(".dat", ".syncconflict-older.dat"),
            )
            .unwrap();

        // Create conflict as a copy (same id)
        book_file.copy_file(&conflict_path).unwrap();

        // Now update the MAIN file to be newer with a different title
        sleep(Duration::from_millis(5));
        let mut rf = book_file.open_file().unwrap();
        let mut main_book = Book::deserialize(&mut rf).unwrap();
        main_book.title = "V2".into();
        let mut wf = book_file.create_file().unwrap();
        main_book.serialize(&mut wf).unwrap();

        // Acquire metadata (should include conflict)
        let mut books = library.list_books().unwrap();
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].conflicting_paths.len(), 1);
        let meta = books.remove(0);

        // Act
        let loaded = super::LibraryBook::load_from_metadata(meta).unwrap();

        // Assert: main is kept, conflict removed
        assert_eq!(loaded.book.title, "V2");
        let mut f = book_file.open_file().unwrap();
        let on_disk = Book::deserialize(&mut f).unwrap();
        assert_eq!(on_disk.title, "V2");
        assert!(!conflict_path.exists().unwrap());
    }
}
