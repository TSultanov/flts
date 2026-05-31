use std::{
    io::Write,
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime},
};

use crate::tla_trace::mutex::TracedMutex;
use isolang::Language;

use crate::{
    book::{book::Book, serialization::Serializable, translation::Translation, translation_import},
    library::{Library, LibraryTranslationMetadata, library_book::BookReadingState},
    test_utils::TempDir,
    translator::TranslationModel,
};

#[tokio::test]
async fn list_books_conflicting_versions() {
    let temp_dir = TempDir::new("flts_test_book");
    let library_path = temp_dir.path.join("lib");
    let library = Library::open(library_path.clone()).await.unwrap();

    let book1 = library
        .create_book("First Book", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    book1.lock().await.save().await.unwrap();

    let book_file = book1.lock().await.path.join("book.dat");

    let conflict_path = book1.lock().await.path.join(
        book_file
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .replace(".dat", ".syncconflict-foobar.dat"),
    );

    std::fs::copy(&book_file, &conflict_path).unwrap();

    let library_books = library.list_books().await.unwrap();

    assert_eq!(library_books.len(), 1);

    assert_eq!(library_books[0].conflicting_paths.len(), 1);
    assert_eq!(
        library_books[0].conflicting_paths[0].file_name(),
        conflict_path.file_name()
    );
}

#[tokio::test]
async fn list_books_conflicting_translation_versions() {
    let temp_dir = TempDir::new("flts_test_book");
    let library_path = temp_dir.path.join("lib");
    let library = Library::open(library_path.clone()).await.unwrap();

    let book1 = library
        .create_book("First Book", &Language::from_639_3("spa").unwrap())
        .await
        .unwrap();
    let _translation = book1
        .lock()
        .await
        .get_or_create_translation(&Language::from_str("en").unwrap())
        .await;
    book1.lock().await.save().await.unwrap();

    let translation_file = book1.lock().await.path.join(format!(
        "translation_{}_{}.dat",
        Language::from_str("es").unwrap().to_639_3(),
        Language::from_str("en").unwrap().to_639_3()
    ));

    let conflict_path = book1.lock().await.path.join(
        translation_file
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .replace(".dat", ".syncconflict-foobar.dat"),
    );

    std::fs::copy(&translation_file, &conflict_path).unwrap();

    let library_books = library.list_books().await.unwrap();

    assert_eq!(library_books[0].translations_metadata.len(), 1);
    assert_eq!(
        library_books[0].translations_metadata[0]
            .main_path
            .file_name(),
        translation_file.file_name()
    );
    assert_eq!(
        library_books[0].translations_metadata[0]
            .conflicting_paths
            .len(),
        1
    );
    assert_eq!(
        library_books[0].translations_metadata[0].conflicting_paths[0].file_name(),
        conflict_path.file_name()
    );
}

#[tokio::test]
async fn save_after_load_trivial_book_change() {
    let temp_dir = TempDir::new("flts_test_book");
    let library_path = temp_dir.path.join("lib");
    let library = Library::open(library_path.clone()).await.unwrap();

    // Create and save
    let book = library
        .create_book("First Title", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    book.lock().await.save().await.unwrap();

    // Simulate "loaded": set last_modified from disk
    let book_file = book.lock().await.path.join("book.dat");
    book.lock().await.last_modified = std::fs::metadata(&book_file).unwrap().modified().ok();

    // Change and save again
    book.lock().await.book.title = "Updated Title".into();
    book.lock().await.save().await.unwrap();

    // Verify on-disk
    let f = std::fs::File::open(&book_file).unwrap();
    let mut reader = std::io::BufReader::new(f);
    let loaded_book = Book::deserialize(&mut reader).unwrap();
    assert_eq!(loaded_book.title, "Updated Title");
}

#[tokio::test]
async fn save_after_load_book_and_translation_changed() {
    let temp_dir = TempDir::new("flts_test_book");
    let library_path = temp_dir.path.join("lib");
    let library = Library::open(library_path.clone()).await.unwrap();

    let source_language = Language::from_str("es").unwrap();
    let target_language = Language::from_str("en").unwrap();

    // Create a book and attach a translation with an initial version
    let book_id = {
        let book = library
            .create_book("First Book", &source_language)
            .await
            .unwrap();
        let mut book = book.lock().await;
        let mut tr = Translation::create(source_language.to_639_3(), target_language.to_639_3());
        let initial_pt = translation_import::ParagraphTranslation {
            total_tokens: None,
            timestamp: 1,
            sentences: vec![translation_import::Sentence {
                full_translation: "Hola".into(),
                words: vec![translation_import::Word {
                    original: "Hola".into(),
                    contextual_translations: vec!["Hello".into()],
                    note: Some(String::new()),
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
        tr.add_paragraph_translation(0, &initial_pt, TranslationModel::Gemini25Flash);
        book.translations
            .push(Arc::new(TracedMutex::new(super::LibraryTranslation {
                translation: tr,
                source_language,
                target_language,
                last_modified: None,
                last_saved_hash: None,
                changed: true,
            })));
        book.save().await.unwrap();
        book.book.id
    };

    // Reload book
    let path = {
        let book = library.get_book(&book_id).await.unwrap();
        let mut book = book.lock().await;

        // Modify both book and translation
        book.book.title = "Second Edition".into();
        let new_pt = translation_import::ParagraphTranslation {
            total_tokens: None,
            timestamp: 2,
            sentences: vec![translation_import::Sentence {
                full_translation: "Hola mundo".into(),
                words: vec![translation_import::Word {
                    original: "Hola".into(),
                    contextual_translations: vec!["Hello".into()],
                    note: Some(String::new()),
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
        // Go through the wrapper so the dirty flag is set, matching production
        // (the inner Translation has no dirty tracking of its own).
        book.translations[0]
            .lock()
            .await
            .add_paragraph_translation(0, &new_pt, TranslationModel::Gemini25Flash);

        book.save().await.unwrap();
        book.path.clone()
    };

    let book_file = path.join("book.dat");
    let tr_file = path.join(format!(
        "translation_{}_{}.dat",
        source_language.to_639_3(),
        target_language.to_639_3()
    ));

    // Verify book updated
    let bf = std::fs::File::open(&book_file).unwrap();
    let mut reader = std::io::BufReader::new(bf);
    let loaded_book = Book::deserialize(&mut reader).unwrap();
    assert_eq!(loaded_book.title, "Second Edition");

    // Verify translation latest version
    let tf = std::fs::File::open(&tr_file).unwrap();
    let mut reader = std::io::BufReader::new(tf);
    let tr2 = Translation::deserialize(&mut reader).unwrap();
    let latest = tr2.paragraph_view(0).unwrap();
    assert_eq!(latest.timestamp, 2);
    assert_eq!(latest.sentence_view(0).full_translation, "Hola mundo");
}

#[tokio::test]
async fn save_merges_translation_with_concurrent_on_disk_change() {
    let temp_dir = TempDir::new("flts_test_book");
    let library_path = temp_dir.path.join("lib");
    let library = Library::open(library_path.clone()).await.unwrap();

    let source_language = Language::from_str("en").unwrap();
    let target_language = Language::from_str("ru").unwrap();

    // Create a book with a translation ts=1
    let book = library
        .create_book("Merge Book", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    let mut book = book.lock().await;
    let mut tr = Translation::create(source_language.to_639_3(), target_language.to_639_3());
    let pt1 = translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: 1,
        sentences: vec![translation_import::Sentence {
            full_translation: "v1".into(),
            words: vec![translation_import::Word {
                original: "v1".into(),
                contextual_translations: vec!["v1".into()],
                note: Some(String::new()),
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
    tr.add_paragraph_translation(0, &pt1, TranslationModel::Gemini25Flash);
    book.translations
        .push(Arc::new(TracedMutex::new(super::LibraryTranslation {
            translation: tr,
            source_language,
            target_language,
            last_modified: None,
            last_saved_hash: None,
            changed: true,
        })));
    book.save().await.unwrap();

    // Treat as loaded instance with last_modified
    let book_file = book.path.join("book.dat");
    let tr_path = book.path.join(format!(
        "translation_{}_{}.dat",
        source_language.to_639_3(),
        target_language.to_639_3()
    ));
    book.last_modified = std::fs::metadata(&book_file).unwrap().modified().ok();
    book.translations.clear();
    let loaded_tr = super::LibraryTranslation::load(&tr_path).await.unwrap();
    book.translations
        .push(Arc::new(TracedMutex::new(loaded_tr)));

    // In-memory change ts=2
    let mem_pt = translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: 2,
        sentences: vec![translation_import::Sentence {
            full_translation: "mem".into(),
            words: vec![translation_import::Word {
                original: "mem".into(),
                contextual_translations: vec!["mem".into()],
                note: Some(String::new()),
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
        .add_paragraph_translation(0, &mem_pt, TranslationModel::Gemini25Flash);

    // Concurrent on-disk change ts=3
    {
        let mut on_disk = {
            let f = std::fs::File::open(&tr_path).unwrap();
            let mut reader = std::io::BufReader::new(f);
            Translation::deserialize(&mut reader).unwrap()
        };
        let disk_pt = translation_import::ParagraphTranslation {
            total_tokens: None,
            timestamp: 3,
            sentences: vec![translation_import::Sentence {
                full_translation: "disk".into(),
                words: vec![translation_import::Word {
                    original: "disk".into(),
                    contextual_translations: vec!["disk".into()],
                    note: Some(String::new()),
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
        on_disk.add_paragraph_translation(0, &disk_pt, TranslationModel::Gemini25Flash);
        let wf = std::fs::File::create(&tr_path).unwrap();
        let mut writer = std::io::BufWriter::new(wf);
        on_disk.serialize(&mut writer).unwrap();
    }

    // Save should merge: latest ts=3 -> ts=2 -> ts=1
    book.save().await.unwrap();
    let tf = std::fs::File::open(&tr_path).unwrap();
    let mut reader = std::io::BufReader::new(tf);
    let merged_tr = Translation::deserialize(&mut reader).unwrap();
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
async fn reading_state_roundtrip() {
    let temp_dir = TempDir::new("flts_test_book");
    let library_path = temp_dir.path.join("lib");
    let library = Library::open(library_path.clone()).await.unwrap();

    let book = library
        .create_book("Stateful", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    let book_id = {
        let mut book = book.lock().await;
        book.save().await.unwrap();
        book.update_reading_state(BookReadingState {
            chapter_id: 2,
            paragraph_id: 15,
            page_offset: 0,
        })
        .await
        .unwrap();
        book.book.id
    };

    let book = library.get_book(&book_id).await.unwrap();
    let mut book = book.lock().await;
    let state = book.reading_state().await.unwrap();
    assert_eq!(state.as_ref().map(|s| s.chapter_id), Some(2));
    assert_eq!(state.as_ref().map(|s| s.paragraph_id), Some(15));
}

#[tokio::test]
async fn folder_path_roundtrip() {
    let temp_dir = TempDir::new("flts_test_book");
    let library_path = temp_dir.path.join("lib");
    let library = Library::open(library_path.clone()).await.unwrap();

    let book = library
        .create_book("Shelved", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    let book_id = {
        let mut book = book.lock().await;
        book.save().await.unwrap();
        book.update_folder_path(vec!["Shelf".into(), "Favorites".into()])
            .await
            .unwrap();
        book.book.id
    };

    let book = library.get_book(&book_id).await.unwrap();
    let mut book = book.lock().await;
    let folder_path = book.folder_path().await.unwrap();
    assert_eq!(
        folder_path,
        vec!["Shelf".to_string(), "Favorites".to_string()]
    );
}

#[tokio::test]
async fn reading_state_prefers_latest_conflict() {
    let temp_dir = TempDir::new("flts_test_book");
    let library_root = temp_dir.path.join("lib");
    let library = Library::open(library_root.clone()).await.unwrap();

    let book = library
        .create_book("Conflicted", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    let book_id = {
        let mut book = book.lock().await;
        book.save().await.unwrap();
        book.update_reading_state(BookReadingState {
            chapter_id: 1,
            paragraph_id: 1,
            page_offset: 0,
        })
        .await
        .unwrap();
        book.book.id
    };

    {
        let book = library.get_book(&book_id).await.unwrap();
        let book = book.lock().await;
        let conflict_path = book.path.join("state (conflict copy).json");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let serialized = serde_json::to_vec(&BookReadingState {
            chapter_id: 4,
            paragraph_id: 8,
            page_offset: 0,
        })
        .unwrap();
        let mut file = std::fs::File::create(&conflict_path).unwrap();
        file.write_all(&serialized).unwrap();
    }

    drop(library);

    let library = Library::open(library_root).await.unwrap();
    let book = library.get_book(&book_id).await.unwrap();
    let mut book = book.lock().await;
    let state = book.reading_state().await.unwrap();
    assert_eq!(state.as_ref().map(|s| s.chapter_id), Some(4));
    assert_eq!(state.as_ref().map(|s| s.paragraph_id), Some(8));
}

#[tokio::test]
async fn load_user_state_from_legacy_file() {
    let temp_dir = TempDir::new("flts_test_book");
    let book_dir = temp_dir.path.join("legacy");
    std::fs::create_dir_all(&book_dir).unwrap();

    let state_path = book_dir.join("state.json");
    {
        let mut file = std::fs::File::create(&state_path).unwrap();
        file.write_all(br#"{"chapterId":3,"paragraphId":9}"#)
            .unwrap();
    }

    let state = super::load_book_user_state(&book_dir).await.unwrap();
    assert_eq!(state.folder_path, Vec::<String>::new());
    assert_eq!(state.reading_state.as_ref().map(|s| s.chapter_id), Some(3));
    assert_eq!(
        state.reading_state.as_ref().map(|s| s.paragraph_id),
        Some(9)
    );
}

#[tokio::test]
async fn load_from_metadata_no_conflicts() {
    // Arrange: create a single main translation file with a simple history
    let temp_dir = TempDir::new("flts_test_book");
    let dir = temp_dir.path.join("book");
    std::fs::create_dir_all(&dir).unwrap();

    let source_language = Language::from_str("en").unwrap();
    let target_language = Language::from_str("ru").unwrap();

    let main_path = dir.join("translation_en_ru.dat");
    let mut t_main = Translation::create(source_language.to_639_3(), target_language.to_639_3());
    let pt2 = translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: 2,
        sentences: vec![translation_import::Sentence {
            full_translation: "m2".into(),
            words: vec![translation_import::Word {
                original: "m2".into(),
                contextual_translations: vec!["m2".into()],
                note: Some(String::new()),
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
    t_main.add_paragraph_translation(0, &pt2, TranslationModel::Gemini25Flash);
    {
        let f = std::fs::File::create(&main_path).unwrap();
        let mut writer = std::io::BufWriter::new(f);
        t_main.serialize(&mut writer).unwrap();
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
    let loaded = super::LibraryTranslation::load_from_metadata(meta)
        .await
        .unwrap();

    // Assert: translation loaded and unchanged, latest ts=2
    let latest = loaded.translation.paragraph_view(0).unwrap();
    assert_eq!(latest.timestamp, 2);
    assert_eq!(latest.sentence_view(0).full_translation, "m2");
}

#[tokio::test]
async fn load_from_metadata_merges_conflicts_and_persists() {
    // Arrange: create main + two conflict files with different timestamps
    let temp_dir = TempDir::new("flts_test_book");
    let dir = temp_dir.path.join("book2");
    std::fs::create_dir_all(&dir).unwrap();

    let source_language = Language::from_str("en").unwrap();
    let target_language = Language::from_str("ru").unwrap();

    let main_path = dir.join(format!(
        "translation_{}_{}.dat",
        source_language.to_639_3(),
        target_language.to_639_3()
    ));
    let conflict1 = dir.join(format!(
        "translation_{}_{}.conflict1.dat",
        source_language.to_639_3(),
        target_language.to_639_3()
    ));
    let conflict2 = dir.join("translation_en_ru.conflict2.dat");

    // main: ts=2
    let mut t_main = Translation::create(source_language.to_639_3(), target_language.to_639_3());
    let pt2 = translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: 2,
        sentences: vec![translation_import::Sentence {
            full_translation: "m2".into(),
            words: vec![translation_import::Word {
                original: "m2".into(),
                contextual_translations: vec!["m2".into()],
                note: Some(String::new()),
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
    t_main.add_paragraph_translation(0, &pt2, TranslationModel::Gemini25Flash);
    {
        let f = std::fs::File::create(&main_path).unwrap();
        let mut writer = std::io::BufWriter::new(f);
        t_main.serialize(&mut writer).unwrap();
    }

    // conflict1: ts=1
    let mut t_c1 = Translation::create(source_language.to_639_3(), target_language.to_639_3());
    let pt1 = translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: 1,
        sentences: vec![translation_import::Sentence {
            full_translation: "c1".into(),
            words: vec![translation_import::Word {
                original: "c1".into(),
                contextual_translations: vec!["c1".into()],
                note: Some(String::new()),
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
    t_c1.add_paragraph_translation(0, &pt1, TranslationModel::Gemini25Flash);
    {
        let f = std::fs::File::create(&conflict1).unwrap();
        let mut writer = std::io::BufWriter::new(f);
        t_c1.serialize(&mut writer).unwrap();
    }

    // conflict2: ts=3
    let mut t_c2 = Translation::create("en", "ru");
    let pt3 = translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: 3,
        sentences: vec![translation_import::Sentence {
            full_translation: "c3".into(),
            words: vec![translation_import::Word {
                original: "c3".into(),
                contextual_translations: vec!["c3".into()],
                note: Some(String::new()),
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
    t_c2.add_paragraph_translation(0, &pt3, TranslationModel::Gemini25Flash);
    {
        let f = std::fs::File::create(&conflict2).unwrap();
        let mut writer = std::io::BufWriter::new(f);
        t_c2.serialize(&mut writer).unwrap();
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
    let loaded = super::LibraryTranslation::load_from_metadata(meta)
        .await
        .unwrap();

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
    let f = std::fs::File::open(&main_path).unwrap();
    let mut reader = std::io::BufReader::new(f);
    let on_disk = Translation::deserialize(&mut reader).unwrap();
    let on_disk_latest = on_disk.paragraph_view(0).unwrap();
    assert_eq!(on_disk_latest.timestamp, 3);
    assert_eq!(on_disk_latest.sentence_view(0).full_translation, "c3");
}

#[tokio::test]
async fn library_book_load_from_metadata_no_conflicts() {
    // Arrange
    let temp_dir = TempDir::new("flts_test_book");
    let library_path = temp_dir.path.join("lib");
    let library = Library::open(library_path.clone()).await.unwrap();

    let book = library
        .create_book("Original Title", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    let mut book = book.lock().await;
    book.save().await.unwrap();

    // Acquire metadata for the only book
    let mut books = library.list_books().await.unwrap();
    assert_eq!(books.len(), 1);
    let meta = books.remove(0);
    assert!(meta.conflicting_paths.is_empty());

    // Act
    let loaded = super::LibraryBook::load_from_metadata(meta).await.unwrap();

    // Assert
    assert_eq!(loaded.book.title, "Original Title");
}

#[tokio::test]
async fn library_book_load_from_metadata_selects_newest_conflict_and_cleans() {
    use std::{thread::sleep, time::Duration};

    // Arrange
    let temp_dir = TempDir::new("flts_test_book");
    let library_path = temp_dir.path.join("lib");
    let library = Library::open(library_path.clone()).await.unwrap();

    let book = library
        .create_book("Main V1", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    let mut book = book.lock().await;
    book.save().await.unwrap();

    let book_file = book.path.join("book.dat");
    let conflict_path = book.path.join(
        book_file
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .replace(".dat", ".syncconflict-newer.dat"),
    );

    // Create conflict as a copy first (same id)
    std::fs::copy(&book_file, &conflict_path).unwrap();

    // Ensure timestamp difference and update conflict content to be "newer"
    sleep(Duration::from_millis(5));
    let rf = std::fs::File::open(&conflict_path).unwrap();
    let mut reader = std::io::BufReader::new(rf);
    let mut conflict_book = Book::deserialize(&mut reader).unwrap();
    conflict_book.title = "From Conflict".into();
    let wf = std::fs::File::create(&conflict_path).unwrap();
    let mut writer = std::io::BufWriter::new(wf);
    conflict_book.serialize(&mut writer).unwrap();

    // Acquire metadata (should include the conflict)
    let mut books = library.list_books().await.unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].conflicting_paths.len(), 1);
    let meta = books.remove(0);

    // Act: load should select the newest (conflict), move it to main, and delete conflicts
    let loaded = super::LibraryBook::load_from_metadata(meta).await.unwrap();

    // Assert: loaded content is from conflict (newest)
    assert_eq!(loaded.book.title, "From Conflict");
    // On-disk main should now contain the conflict content and conflict file should be gone
    let f = std::fs::File::open(&book_file).unwrap();
    let mut reader = std::io::BufReader::new(f);
    let on_disk = Book::deserialize(&mut reader).unwrap();
    assert_eq!(on_disk.title, "From Conflict");
    assert!(!conflict_path.exists());
}

#[tokio::test]
async fn library_book_load_from_metadata_keeps_main_if_newest_and_cleans() {
    use std::{thread::sleep, time::Duration};

    // Arrange
    let temp_dir = TempDir::new("flts_test_book");
    let library_path = temp_dir.path.join("lib");
    let library = Library::open(library_path.clone()).await.unwrap();

    let book = library
        .create_book("V1", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    let mut book = book.lock().await;
    book.save().await.unwrap();

    let book_file = book.path.join("book.dat");
    let conflict_path = book.path.join(
        book_file
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .replace(".dat", ".syncconflict-older.dat"),
    );

    // Create conflict as a copy (same id)
    std::fs::copy(&book_file, &conflict_path).unwrap();

    // Now update the MAIN file to be newer with a different title
    sleep(Duration::from_millis(5));
    let rf = std::fs::File::open(&book_file).unwrap();
    let mut reader = std::io::BufReader::new(rf);
    let mut main_book = Book::deserialize(&mut reader).unwrap();
    main_book.title = "V2".into();
    let wf = std::fs::File::create(&book_file).unwrap();
    let mut writer = std::io::BufWriter::new(wf);
    main_book.serialize(&mut writer).unwrap();

    // Acquire metadata (should include conflict)
    let mut books = library.list_books().await.unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].conflicting_paths.len(), 1);
    let meta = books.remove(0);

    // Act
    let loaded = super::LibraryBook::load_from_metadata(meta).await.unwrap();

    // Assert: main is kept, conflict removed
    assert_eq!(loaded.book.title, "V2");
    let f = std::fs::File::open(&book_file).unwrap();
    let mut reader = std::io::BufReader::new(f);
    let on_disk = Book::deserialize(&mut reader).unwrap();
    assert_eq!(on_disk.title, "V2");
    assert!(!conflict_path.exists());
}

#[tokio::test]
async fn delete_book_removes_directory() {
    let temp_dir = TempDir::new("flts_test_book");
    let library_path = temp_dir.path.join("lib");
    let library = Library::open(library_path.clone()).await.unwrap();

    let book = library
        .create_book("Disposable", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    let book_id = {
        let mut book = book.lock().await;
        book.save().await.unwrap();
        book.book.id
    };

    let book_dir = library_path.join(book_id.to_string());
    assert!(book_dir.exists());

    library.delete_book(&book_id).await.unwrap();

    assert!(!book_dir.exists());
    assert!(library.list_books().await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// File-watcher echo suppression (save/reload loop fix)
// ---------------------------------------------------------------------------

fn simple_paragraph(text: &str, timestamp: u64) -> translation_import::ParagraphTranslation {
    translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp,
        sentences: vec![translation_import::Sentence {
            full_translation: text.into(),
            words: vec![translation_import::Word {
                original: text.into(),
                contextual_translations: vec![text.into()],
                note: Some(String::new()),
                is_punctuation: false,
                grammar: translation_import::Grammar {
                    original_initial_form: text.into(),
                    target_initial_form: text.into(),
                    part_of_speech: "n".into(),
                    plurality: None,
                    person: None,
                    tense: None,
                    case: None,
                    other: None,
                },
            }],
        }],
    }
}

/// Pushes the file's mtime into the future without touching its bytes,
/// mirroring how our own atomic save (or a Syncthing re-touch) bumps mtime.
/// Returns the time it set.
fn bump_mtime_future(path: &std::path::Path) -> SystemTime {
    let future = SystemTime::now() + Duration::from_secs(3600);
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(future).unwrap();
    future
}

/// Creates a book with one en->ru translation, saves it, and returns the
/// locked book plus the translation file path. After save the in-memory
/// translation/book carry their `last_saved_hash`.
async fn book_with_saved_translation(
    library: &Library,
    title: &str,
) -> (Arc<TracedMutex<super::LibraryBook>>, std::path::PathBuf) {
    let source_language = Language::from_str("en").unwrap();
    let target_language = Language::from_str("ru").unwrap();
    let book = library
        .create_book(title, &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    {
        let mut book = book.lock().await;
        let mut tr = Translation::create(source_language.to_639_3(), target_language.to_639_3());
        tr.add_paragraph_translation(
            0,
            &simple_paragraph("v1", 1),
            TranslationModel::Gemini25Flash,
        );
        book.translations
            .push(Arc::new(TracedMutex::new(super::LibraryTranslation {
                translation: tr,
                source_language,
                target_language,
                last_modified: None,
                last_saved_hash: None,
                changed: true,
            })));
        book.save().await.unwrap();
    }
    let tr_path = {
        let book = book.lock().await;
        book.path.join(format!(
            "translation_{}_{}.dat",
            source_language.to_639_3(),
            target_language.to_639_3()
        ))
    };
    (book, tr_path)
}

#[tokio::test]
async fn serialize_is_deterministic() {
    // The echo gate assumes re-serializing identical state yields identical
    // bytes (and thus an identical trailing hash). Guard that assumption.
    let source_language = Language::from_str("en").unwrap();
    let target_language = Language::from_str("ru").unwrap();
    let mut tr = Translation::create(source_language.to_639_3(), target_language.to_639_3());
    tr.add_paragraph_translation(0, &simple_paragraph("hello", 1), TranslationModel::Gemini25Flash);
    tr.add_paragraph_translation(1, &simple_paragraph("world", 2), TranslationModel::Gemini25Flash);

    let mut a = Vec::new();
    let mut b = Vec::new();
    tr.serialize(&mut a).unwrap();
    tr.serialize(&mut b).unwrap();
    assert_eq!(a, b, "translation serialization must be byte-deterministic");

    let book = Book::create(
        uuid::Uuid::new_v4(),
        "Det Book",
        &Language::from_639_3("eng").unwrap(),
    );
    let mut ba = Vec::new();
    let mut bb = Vec::new();
    book.serialize(&mut ba).unwrap();
    book.serialize(&mut bb).unwrap();
    assert_eq!(ba, bb, "book serialization must be byte-deterministic");
}

#[tokio::test]
async fn save_clears_changed_flag() {
    let temp_dir = TempDir::new("flts_test_book");
    let library = Library::open(temp_dir.path.join("lib")).await.unwrap();
    let (book, _tr_path) = book_with_saved_translation(&library, "Clean").await;

    let book = book.lock().await;
    assert!(
        !book.has_unsaved_changes().await,
        "translation should be clean after a successful save"
    );
}

#[tokio::test]
async fn reload_translations_skips_same_content_echo() {
    let temp_dir = TempDir::new("flts_test_book");
    let library = Library::open(temp_dir.path.join("lib")).await.unwrap();
    let (book, tr_path) = book_with_saved_translation(&library, "Echo Tr").await;

    let before = std::fs::read(&tr_path).unwrap();
    // Bump mtime only — content is byte-identical (our own write echo).
    let future = bump_mtime_future(&tr_path);

    let from = Language::from_str("en").unwrap();
    let to = Language::from_str("ru").unwrap();
    let saved = book
        .lock()
        .await
        .reload_translations(future, from, to)
        .await
        .unwrap();

    assert!(!saved, "same-content echo must not trigger a re-save");
    assert_eq!(
        std::fs::read(&tr_path).unwrap(),
        before,
        "file must be left untouched"
    );
}

#[tokio::test]
async fn reload_translations_saves_on_external_change() {
    let temp_dir = TempDir::new("flts_test_book");
    let library = Library::open(temp_dir.path.join("lib")).await.unwrap();
    let (book, tr_path) = book_with_saved_translation(&library, "Ext Tr").await;

    // Externally rewrite the translation with genuinely different content.
    {
        let f = std::fs::File::open(&tr_path).unwrap();
        let mut reader = std::io::BufReader::new(f);
        let mut on_disk = Translation::deserialize(&mut reader).unwrap();
        on_disk.add_paragraph_translation(
            0,
            &simple_paragraph("external", 5),
            TranslationModel::Gemini25Flash,
        );
        let wf = std::fs::File::create(&tr_path).unwrap();
        let mut writer = std::io::BufWriter::new(wf);
        on_disk.serialize(&mut writer).unwrap();
    }
    let future = bump_mtime_future(&tr_path);

    let from = Language::from_str("en").unwrap();
    let to = Language::from_str("ru").unwrap();
    let saved = book
        .lock()
        .await
        .reload_translations(future, from, to)
        .await
        .unwrap();

    assert!(saved, "a genuine external content change must trigger a save");
}

#[tokio::test]
async fn reload_book_skips_same_content_echo() {
    let temp_dir = TempDir::new("flts_test_book");
    let library = Library::open(temp_dir.path.join("lib")).await.unwrap();
    let (book, _tr_path) = book_with_saved_translation(&library, "Echo Bk").await;

    let book_file = book.lock().await.path.join("book.dat");
    let before = std::fs::read(&book_file).unwrap();
    let future = bump_mtime_future(&book_file);

    let saved = book.lock().await.reload_book(future).await.unwrap();

    assert!(!saved, "same-content book echo must not trigger a re-save");
    assert_eq!(
        std::fs::read(&book_file).unwrap(),
        before,
        "book.dat must be left untouched"
    );
}

#[tokio::test]
async fn reload_book_saves_on_external_change() {
    let temp_dir = TempDir::new("flts_test_book");
    let library = Library::open(temp_dir.path.join("lib")).await.unwrap();
    let (book_a, _tr_a) = book_with_saved_translation(&library, "Bk A").await;
    let (book_b, _tr_b) = book_with_saved_translation(&library, "Bk B").await;

    // Overwrite A's book.dat with B's (different content => different hash).
    let a_file = book_a.lock().await.path.join("book.dat");
    let b_file = book_b.lock().await.path.join("book.dat");
    std::fs::copy(&b_file, &a_file).unwrap();
    let future = bump_mtime_future(&a_file);

    let saved = book_a.lock().await.reload_book(future).await.unwrap();
    assert!(saved, "a genuine external book change must trigger a save");
}
