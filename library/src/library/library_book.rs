use std::{
    collections::HashSet,
    io::{BufReader, BufWriter},
    str::FromStr,
    sync::Arc,
    time::SystemTime,
};

use ahash::AHashSet;
use isolang::Language;
use tokio::sync::Mutex;
use uuid::Uuid;
use vfs::VfsPath;

use crate::{
    book::{
        book::Book,
        serialization::{Serializable, create_random_string},
        translation::{ParagraphTranslationView, Translation},
        translation_import,
    },
    library::{
        Library, LibraryBookMetadata, LibraryError, LibraryTranslationMetadata,
        library_dictionary::DictionaryCache,
    },
};

pub struct LibraryBook {
    dict_cache: Arc<Mutex<DictionaryCache>>,
    path: VfsPath,
    last_modified: Option<SystemTime>,
    pub book: Book,
    translations: Vec<Arc<Mutex<LibraryTranslation>>>,
}

pub struct LibraryTranslation {
    dict_cache: Arc<Mutex<DictionaryCache>>,
    translation: Translation,
    source_language: Language,
    target_language: Language,
    last_modified: Option<SystemTime>,
    changed: bool,
}

impl LibraryTranslation {
    fn merge(&mut self, other: LibraryTranslation) {
        let other_t = other.translation;

        let merged_translation = self.translation.merge(&other_t);

        self.translation = merged_translation;
        self.last_modified = self.last_modified.max(other.last_modified);
        self.changed = true;
    }

    fn load(dict_cache: Arc<Mutex<DictionaryCache>>, path: &VfsPath) -> anyhow::Result<Self> {
        let last_modified = path.metadata()?.modified;
        let mut file = BufReader::new(path.open_file()?);
        let translation = Translation::deserialize(&mut file)?;
        let source_language = Language::from_str(&translation.source_language)?;
        let target_language = Language::from_str(&translation.target_language)?;

        Ok(Self {
            dict_cache,
            translation,
            source_language,
            target_language,
            last_modified,
            changed: false,
        })
    }

    fn load_from_metadata(
        dict_cache: Arc<Mutex<DictionaryCache>>,
        metadata: LibraryTranslationMetadata,
    ) -> anyhow::Result<Self> {
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

        Self::load(dict_cache, &metadata.main_path)
    }

    pub async fn add_paragraph_translation(
        &mut self,
        paragraph_index: usize,
        translation: &translation_import::ParagraphTranslation,
    ) -> anyhow::Result<()> {
        let dictionary = self
            .dict_cache
            .lock()
            .await
            .get_dictionary(self.source_language, self.target_language)?;
        self.translation.add_paragraph_translation(
            paragraph_index,
            translation,
            &mut dictionary.lock().await.dictionary,
        );
        self.changed = true;
        Ok(())
    }

    pub fn translated_paragraphs_count(&self) -> usize {
        self.translation.translated_paragraphs_count()
    }

    pub fn paragraph_view(&'_ self, paragraph: usize) -> Option<ParagraphTranslationView<'_>> {
        self.translation.paragraph_view(paragraph)
    }
}

impl LibraryBook {
    pub async fn get_or_create_translation(
        &mut self,
        target_language: &Language,
    ) -> Arc<Mutex<LibraryTranslation>> {
        let source_language = &self.book.language;

        for (t_idx, t) in self.translations.iter().enumerate() {
            if &t.lock().await.translation.source_language == source_language
                && t.lock().await.translation.target_language == target_language.to_639_3()
            {
                return self.translations[t_idx].clone();
            }
        }

        // Not found: create and push
        self.translations
            .push(Arc::new(Mutex::new(LibraryTranslation {
                dict_cache: self.dict_cache.clone(),
                translation: Translation::create(source_language, target_language.to_639_3()),
                source_language: Language::from_639_3(source_language).unwrap(),
                target_language: *target_language,
                last_modified: None,
                changed: true,
            })));

        let last = self.translations.len() - 1;
        self.translations[last].clone()
    }

    pub fn load_from_metadata(
        dict_cache: Arc<Mutex<DictionaryCache>>,
        metadata: LibraryBookMetadata,
    ) -> anyhow::Result<Self> {
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

        let mut book = Self::load(dict_cache.clone(), &metadata.main_path)?;

        for tm in metadata.translations_metadata {
            let translation = Arc::new(Mutex::new(LibraryTranslation::load_from_metadata(
                dict_cache.clone(),
                tm,
            )?));
            book.translations.push(translation);
        }

        Ok(book)
    }

    fn load(
        dict_cache: Arc<Mutex<DictionaryCache>>,
        path: &VfsPath,
    ) -> Result<Self, vfs::error::VfsError> {
        let last_modified = path.metadata()?.modified;
        let mut file = BufReader::new(path.open_file()?);
        let book = Book::deserialize(&mut file)?;

        Ok(Self {
            dict_cache,
            path: path.parent(),
            last_modified,
            book,
            translations: vec![],
        })
    }

    pub async fn reload_book(&mut self, modified: SystemTime) -> anyhow::Result<()> {
        if self.last_modified.map_or(true, |lm| lm < modified) {
            self.save().await?;
        }

        Ok(())
    }

    pub async fn reload_translations(
        &mut self,
        modified: SystemTime,
        from: Language,
        to: Language,
    ) -> anyhow::Result<()> {
        let mut needs_save = false;

        for translation in &self.translations {
            let t = translation.lock().await;
            if t.source_language == from
                && t.target_language == to
                && t.last_modified.map_or(true, |lm| lm < modified)
            {
                needs_save = true;
            }
        }

        if needs_save {
            self.save().await?;
        }

        Ok(())
    }

    pub async fn save(&mut self) -> anyhow::Result<()> {
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

        let mut languages_to_save = AHashSet::new();

        for translation_arc in book.translations.drain(0..) {
            let mut translation = translation_arc.lock().await;
            let source_language = translation.translation.source_language.clone();
            let target_language = translation.translation.target_language.clone();
            let translation_file_name =
                format!("translation_{}_{}.dat", source_language, target_language);
            let translation_path = book.path.join(&translation_file_name)?;
            let translation_path_temp = book.path.join(format!(
                "{translation_file_name}~{}",
                create_random_string(8)
            ))?;

            loop {
                let translation_path_modified_pre_save = get_modified_if_exists(&translation_path)?;

                if let Some(last_modified) = translation.last_modified {
                    if translation_path.exists()? {
                        let saved_translation_last_modified =
                            translation_path.metadata()?.modified.unwrap();
                        if saved_translation_last_modified > last_modified {
                            let saved_translation = LibraryTranslation::load(
                                book.dict_cache.clone(),
                                &translation_path,
                            )?;
                            translation.merge(saved_translation);
                        }
                    }
                } else if translation_path.exists()? {
                    let saved_translation =
                        LibraryTranslation::load(book.dict_cache.clone(), &translation_path)?;
                    translation.merge(saved_translation);
                }

                if translation.changed {
                    let mut translation_file = BufWriter::new(translation_path_temp.create_file()?);
                    translation.translation.serialize(&mut translation_file)?;
                    languages_to_save.insert((source_language.clone(), target_language.clone()));

                    if get_modified_if_exists(&translation_path)?
                        == translation_path_modified_pre_save
                        || translation_path_modified_pre_save.is_none()
                    {
                        if translation_path.exists()? {
                            translation_path.remove_file()?;
                        }
                        translation_path_temp.move_file(&translation_path)?;
                        translation.last_modified = get_modified_if_exists(&translation_path)?;
                        merged_translations.push(translation_arc.clone());
                        break;
                    }
                } else {
                    merged_translations.push(translation_arc.clone());
                    break;
                }
            }
        }

        for (src, tgt) in languages_to_save {
            let src = Language::from_str(&src)?;
            let tgt = Language::from_str(&tgt)?;
            let dict = book.dict_cache.lock().await.get_dictionary(src, tgt)?;
            dict.lock().await.save()?;
        }

        let book_path = book.path.join("book.dat")?;
        let book_path_temp = book
            .path
            .join(format!("book.dat~{}", create_random_string(8)))?;
        loop {
            let book_path_modified_pre_save = get_modified_if_exists(&book_path)?;

            if let Some(last_modified) = book.last_modified {
                if book_path.exists()? {
                    let saved_book_last_modified = book_path.metadata()?.modified.unwrap();
                    if saved_book_last_modified > last_modified {
                        let saved_book = Self::load(book.dict_cache.clone(), &book_path)?;
                        book.book = saved_book.book;
                        book.last_modified = saved_book.last_modified;
                    }
                }
            } else if book_path.exists()? {
                let saved_book = Self::load(book.dict_cache.clone(), &book_path)?;
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

                book.last_modified = get_modified_if_exists(&book_path)?;
                break;
            }
            // Attempt to merge and save again otherwise
        }

        let all_book_translations = LibraryBookMetadata::load(&book.path)?;
        let mut loaded_translations = HashSet::new();
        for t in &merged_translations {
            loaded_translations.insert(t.lock().await.translation.id);
        }

        for translation_metadata in all_book_translations.translations_metadata {
            if !loaded_translations.contains(&translation_metadata.id) {
                merged_translations.push(Arc::new(Mutex::new(
                    LibraryTranslation::load_from_metadata(
                        book.dict_cache.clone(),
                        translation_metadata,
                    )?,
                )));
            }
        }

        book.translations = merged_translations;

        Ok(())
    }
}

impl Library {
    pub fn create_book(
        &mut self,
        title: &str,
        language: &Language,
    ) -> anyhow::Result<Arc<Mutex<LibraryBook>>> {
        let books = self.list_books()?;
        if books.iter().any(|b| b.title == title) {
            Err(LibraryError::DuplicateTitle(title.to_owned()))?
        }

        let guid = Uuid::new_v4();
        let book_root = self.library_root.join(guid.to_string())?;

        let book = Arc::new(Mutex::new(LibraryBook {
            dict_cache: self.dictionaries_cache.clone(),
            path: book_root,
            last_modified: None,
            book: Book::create(guid, title, language),
            translations: vec![],
        }));

        self.books_cache.insert(guid, book.clone());

        Ok(book)
    }
}

#[cfg(test)]
mod library_book_tests {
    use std::{str::FromStr, sync::Arc};

    use isolang::Language;
    use tokio::sync::Mutex;
    use vfs::VfsPath;

    use crate::{
        book::{
            book::Book, serialization::Serializable, translation::Translation, translation_import,
        },
        library::{Library, LibraryTranslationMetadata, library_dictionary::DictionaryCache},
    };

    #[tokio::test]
    async fn list_books_conflicting_versions() {
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let library_path = root.join("lib").unwrap();
        let mut library = Library::open(library_path.clone()).unwrap();

        let book1 = library
            .create_book("First Book", &Language::from_639_3("eng").unwrap())
            .unwrap();
        book1.lock().await.save().await.unwrap();

        let book_file = book1.lock().await.path.join("book.dat").unwrap();

        let conflict_path = book1
            .lock()
            .await
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
        let mut library = Library::open(library_path.clone()).unwrap();

        let book1 = library
            .create_book("First Book", &Language::from_639_3("spa").unwrap())
            .unwrap();
        let _translation = book1
            .lock()
            .await
            .get_or_create_translation(&Language::from_str("en").unwrap())
            .await;
        book1.lock().await.save().await.unwrap();

        let translation_file = book1
            .lock()
            .await
            .path
            .join(format!(
                "translation_{}_{}.dat",
                Language::from_str("es").unwrap().to_639_3(),
                Language::from_str("en").unwrap().to_639_3()
            ))
            .unwrap();

        let conflict_path = book1
            .lock()
            .await
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
        let mut library = Library::open(library_path.clone()).unwrap();

        // Create and save
        let book = library
            .create_book("First Title", &Language::from_639_3("eng").unwrap())
            .unwrap();
        book.lock().await.save().await.unwrap();

        // Simulate "loaded": set last_modified from disk
        let book_file = book.lock().await.path.join("book.dat").unwrap();
        book.lock().await.last_modified = book_file.metadata().unwrap().modified;

        // Change and save again
        book.lock().await.book.title = "Updated Title".into();
        book.lock().await.save().await.unwrap();

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
        let mut library = Library::open(library_path.clone()).unwrap();

        let source_language = Language::from_str("es").unwrap();
        let target_language = Language::from_str("en").unwrap();

        let dict = library
            .dictionaries_cache
            .lock()
            .await
            .get_dictionary(source_language, target_language)
            .unwrap();

        // Create a book and attach a translation with an initial version
        let book_id = {
            let book = library.create_book("First Book", &source_language).unwrap();
            let mut book = book.lock().await;
            let mut tr =
                Translation::create(source_language.to_639_3(), target_language.to_639_3());
            let initial_pt = translation_import::ParagraphTranslation {
                timestamp: 1,
                source_language: source_language.to_639_3().to_owned(),
                target_language: target_language.to_639_3().to_owned(),
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
            tr.add_paragraph_translation(0, &initial_pt, &mut dict.lock().await.dictionary);
            book.translations
                .push(Arc::new(Mutex::new(super::LibraryTranslation {
                    dict_cache: library.dictionaries_cache.clone(),
                    translation: tr,
                    source_language,
                    target_language,
                    last_modified: None,
                    changed: true,
                })));
            book.save().await.unwrap();
            book.book.id
        };

        // Reload book
        let path = {
            let book = library.get_book(&book_id).unwrap();
            let mut book = book.lock().await;

            // Modify both book and translation
            book.book.title = "Second Edition".into();
            let new_pt = translation_import::ParagraphTranslation {
                timestamp: 2,
                source_language: source_language.to_639_3().to_owned(),
                target_language: target_language.to_639_3().to_owned(),
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
                .lock()
                .await
                .translation
                .add_paragraph_translation(0, &new_pt, &mut dict.lock().await.dictionary);

            book.save().await.unwrap();
            book.path.clone()
        };

        let book_file = path.join("book.dat").unwrap();
        let tr_file = path
            .join(format!(
                "translation_{}_{}.dat",
                source_language.to_639_3(),
                target_language.to_639_3()
            ))
            .unwrap();

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
        let mut library = Library::open(library_path.clone()).unwrap();

        let source_language = Language::from_str("en").unwrap();
        let target_language = Language::from_str("ru").unwrap();

        let dict = library
            .dictionaries_cache
            .lock()
            .await
            .get_dictionary(source_language, target_language)
            .unwrap();

        // Create a book with a translation ts=1
        let book = library
            .create_book("Merge Book", &Language::from_639_3("eng").unwrap())
            .unwrap();
        let mut book = book.lock().await;
        let mut tr = Translation::create(source_language.to_639_3(), target_language.to_639_3());
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
        tr.add_paragraph_translation(0, &pt1, &mut dict.lock().await.dictionary);
        book.translations
            .push(Arc::new(Mutex::new(super::LibraryTranslation {
                dict_cache: library.dictionaries_cache.clone(),
                translation: tr,
                source_language,
                target_language,
                last_modified: None,
                changed: true,
            })));
        book.save().await.unwrap();

        // Treat as loaded instance with last_modified
        let book_file = book.path.join("book.dat").unwrap();
        let tr_path = book
            .path
            .join(format!(
                "translation_{}_{}.dat",
                source_language.to_639_3(),
                target_language.to_639_3()
            ))
            .unwrap();
        book.last_modified = book_file.metadata().unwrap().modified;
        book.translations.clear();
        let loaded_tr =
            super::LibraryTranslation::load(library.dictionaries_cache.clone(), &tr_path).unwrap();
        book.translations.push(Arc::new(Mutex::new(loaded_tr)));

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
            .lock()
            .await
            .translation
            .add_paragraph_translation(0, &mem_pt, &mut dict.lock().await.dictionary);

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
            on_disk.add_paragraph_translation(0, &disk_pt, &mut dict.lock().await.dictionary);
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

        let dict_cache = Arc::new(Mutex::new(DictionaryCache::new(&root)));

        let source_language = Language::from_str("en").unwrap();
        let target_language = Language::from_str("ru").unwrap();

        let dict = dict_cache
            .lock()
            .await
            .get_dictionary(source_language, target_language)
            .unwrap();

        let main_path = dir.join("translation_en_ru.dat").unwrap();
        let mut t_main =
            Translation::create(source_language.to_639_3(), target_language.to_639_3());
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
        t_main.add_paragraph_translation(0, &pt2, &mut dict.lock().await.dictionary);
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
        let loaded = super::LibraryTranslation::load_from_metadata(dict_cache, meta).unwrap();

        // Assert: translation loaded and unchanged, latest ts=2
        let latest = loaded.translation.paragraph_view(0).unwrap();
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

        let dict_cache = Arc::new(Mutex::new(DictionaryCache::new(&root)));

        let source_language = Language::from_str("en").unwrap();
        let target_language = Language::from_str("ru").unwrap();

        let dict = dict_cache
            .lock()
            .await
            .get_dictionary(source_language, target_language)
            .unwrap();

        let main_path = dir
            .join(format!(
                "translation_{}_{}.dat",
                source_language.to_639_3(),
                target_language.to_639_3()
            ))
            .unwrap();
        let conflict1 = dir
            .join(format!(
                "translation_{}_{}.conflict1.dat",
                source_language.to_639_3(),
                target_language.to_639_3()
            ))
            .unwrap();
        let conflict2 = dir.join("translation_en_ru.conflict2.dat").unwrap();

        // main: ts=2
        let mut t_main =
            Translation::create(source_language.to_639_3(), target_language.to_639_3());
        let pt2 = translation_import::ParagraphTranslation {
            timestamp: 2,
            source_language: source_language.to_639_3().to_owned(),
            target_language: target_language.to_639_3().to_owned(),
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
        t_main.add_paragraph_translation(0, &pt2, &mut dict.lock().await.dictionary);
        {
            let mut f = main_path.create_file().unwrap();
            t_main.serialize(&mut f).unwrap();
        }

        // conflict1: ts=1
        let mut t_c1 = Translation::create(source_language.to_639_3(), target_language.to_639_3());
        let pt1 = translation_import::ParagraphTranslation {
            timestamp: 1,
            source_language: source_language.to_639_3().to_owned(),
            target_language: target_language.to_639_3().to_owned(),
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
        t_c1.add_paragraph_translation(0, &pt1, &mut dict.lock().await.dictionary);
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
        t_c2.add_paragraph_translation(0, &pt3, &mut dict.lock().await.dictionary);
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
        let loaded = super::LibraryTranslation::load_from_metadata(dict_cache, meta).unwrap();

        // Assert: merged order latest=3, then 2, then 1
        let latest = loaded.translation.paragraph_view(0).unwrap();
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
        let mut library = Library::open(library_path.clone()).unwrap();

        let book = library
            .create_book("Original Title", &Language::from_639_3("eng").unwrap())
            .unwrap();
        let mut book = book.lock().await;
        book.save().await.unwrap();

        // Acquire metadata for the only book
        let mut books = library.list_books().unwrap();
        assert_eq!(books.len(), 1);
        let meta = books.remove(0);
        assert!(meta.conflicting_paths.is_empty());

        // Act
        let loaded =
            super::LibraryBook::load_from_metadata(library.dictionaries_cache, meta).unwrap();

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
        let mut library = Library::open(library_path.clone()).unwrap();

        let book = library
            .create_book("Main V1", &Language::from_639_3("eng").unwrap())
            .unwrap();
        let mut book = book.lock().await;
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
        let loaded =
            super::LibraryBook::load_from_metadata(library.dictionaries_cache, meta).unwrap();

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
        let mut library = Library::open(library_path.clone()).unwrap();

        let book = library
            .create_book("V1", &Language::from_639_3("eng").unwrap())
            .unwrap();
        let mut book = book.lock().await;
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
        let loaded =
            super::LibraryBook::load_from_metadata(library.dictionaries_cache, meta).unwrap();

        // Assert: main is kept, conflict removed
        assert_eq!(loaded.book.title, "V2");
        let mut f = book_file.open_file().unwrap();
        let on_disk = Book::deserialize(&mut f).unwrap();
        assert_eq!(on_disk.title, "V2");
        assert!(!conflict_path.exists().unwrap());
    }
}
