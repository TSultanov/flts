use std::{
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use isolang::Language;
use library::{
    book::{book::Book, serialization::Serializable, translation::Translation, translation_import},
    dictionary::Dictionary,
    library::{
        Library,
        library_book::BookReadingState,
        library_dictionary::{LibraryDictionary, LibraryDictionaryMetadata},
    },
    tla_trace,
    translator::TranslationModel,
};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{}_{}", prefix, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct TraceGuard {
    cleanup_dir: Option<PathBuf>,
}

impl TraceGuard {
    fn start(name: &str) -> Self {
        let cleanup_dir = if std::env::var_os("FLTS_TRACE_DIR").is_some() {
            None
        } else {
            Some(std::env::temp_dir().join(format!(
                "flts_trace_harness_{}",
                uuid::Uuid::new_v4()
            )))
        };
        let root = std::env::var_os("FLTS_TRACE_DIR")
            .map(PathBuf::from)
            .or_else(|| cleanup_dir.clone())
            .unwrap();
        std::fs::create_dir_all(&root).unwrap();
        tla_trace::set_trace_file(&root.join(name)).unwrap();
        Self { cleanup_dir }
    }
}

impl Drop for TraceGuard {
    fn drop(&mut self) {
        tla_trace::clear_trace_file().unwrap();
        if let Some(dir) = &self.cleanup_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn sleep_for_mtime_tick() {
    std::thread::sleep(Duration::from_millis(5));
}

fn make_paragraph(ts: u64, text: &str, source_language: &str, target_language: &str) -> translation_import::ParagraphTranslation {
    translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: ts,
        source_language: source_language.to_owned(),
        target_language: target_language.to_owned(),
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

fn write_book(path: &Path, title: &str) {
    let file = std::fs::File::open(path).unwrap();
    let mut reader = BufReader::new(file);
    let mut book = Book::deserialize(&mut reader).unwrap();
    book.title = title.into();
    let file = std::fs::File::create(path).unwrap();
    let mut writer = BufWriter::new(file);
    book.serialize(&mut writer).unwrap();
    writer.flush().unwrap();
}

#[tokio::test]
async fn trace_book_conflict_and_save() {
    let temp_dir = TempDir::new("flts_trace_book");
    let library_root = temp_dir.path.join("lib");
    let library = Library::open(library_root.clone()).await.unwrap();

    let book = library
        .create_book("Trace Book", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    let book_id = {
        let mut book = book.lock().await;
        book.book.push_chapter(Some("Intro"));
        book.book.push_paragraph(0, "hello", None);
        book.save().await.unwrap();
        book.book.id
    };

    let book_dir = library_root.join(book_id.to_string());
    let book_file = book_dir.join("book.dat");
    let conflict_path = book_dir.join("book.syncconflict-trace.dat");
    std::fs::copy(&book_file, &conflict_path).unwrap();
    sleep_for_mtime_tick();
    write_book(&conflict_path, "Conflict Winner");

    drop(library);

    let _trace = TraceGuard::start("book-conflict-and-save.ndjson");
    let library = Library::open(library_root.clone()).await.unwrap();
    let book = library.get_book(&book_id).await.unwrap();
    let mut book = book.lock().await;

    book.book.title = "Memory Edit".into();
    sleep_for_mtime_tick();
    write_book(&book_file, "Disk Edit");
    book.save().await.unwrap();
}

#[tokio::test]
async fn trace_state_updates() {
    let temp_dir = TempDir::new("flts_trace_state");
    let library_root = temp_dir.path.join("lib");
    let library = Library::open(library_root.clone()).await.unwrap();

    let book = library
        .create_book("Stateful", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    let book_id = {
        let mut book = book.lock().await;
        book.save().await.unwrap();
        book.book.id
    };

    let _trace = TraceGuard::start("state-updates.ndjson");
    let book = library.get_book(&book_id).await.unwrap();
    let mut book = book.lock().await;
    book.update_reading_state(BookReadingState {
        chapter_id: 1,
        paragraph_id: 1,
    })
    .await
    .unwrap();
    book.update_folder_path(vec!["f1".into()]).await.unwrap();

    let conflict_path = library_root
        .join(book_id.to_string())
        .join("state (conflict copy).json");
    sleep_for_mtime_tick();
    let mut file = std::fs::File::create(&conflict_path).unwrap();
    file.write_all(br#"{"readingState":{"chapterId":2,"paragraphId":3},"folderPath":["f2"]}"#)
        .unwrap();
    drop(file);

    let _ = book.reading_state().await.unwrap();
}

#[tokio::test]
async fn trace_translation_merge_and_save() {
    let temp_dir = TempDir::new("flts_trace_translation");
    let library_root = temp_dir.path.join("lib");
    let source_language = Language::from_str("en").unwrap();
    let target_language = Language::from_str("ru").unwrap();

    let library = Library::open(library_root.clone()).await.unwrap();
    let book = library
        .create_book("Translate Me", &source_language)
        .await
        .unwrap();
    let book_id = {
        let mut book = book.lock().await;
        book.book.push_chapter(Some("Intro"));
        book.book.push_paragraph(0, "hello", None);
        let translation = book.get_or_create_translation(&target_language).await;
        translation
            .lock()
            .await
            .add_paragraph_translation(
                0,
                &make_paragraph(1, "v1", "en", "ru"),
                TranslationModel::Gemini25Flash,
            )
            .await
            .unwrap();
        book.save().await.unwrap();
        book.book.id
    };

    let book_dir = library_root.join(book_id.to_string());
    let translation_file = book_dir.join("translation_eng_rus.dat");
    let conflict_path = book_dir.join("translation_eng_rus.syncconflict.dat");
    std::fs::copy(&translation_file, &conflict_path).unwrap();

    drop(library);

    let _trace = TraceGuard::start("translation-merge-and-save.ndjson");
    let library = Library::open(library_root.clone()).await.unwrap();
    let book = library.get_book(&book_id).await.unwrap();
    let mut book = book.lock().await;
    let translation = book.get_or_create_translation(&target_language).await;
    translation
        .lock()
        .await
        .add_paragraph_translation(
            0,
            &make_paragraph(2, "mem", "en", "ru"),
            TranslationModel::Gemini25Flash,
        )
        .await
        .unwrap();

    sleep_for_mtime_tick();
    {
        let file = std::fs::File::open(&translation_file).unwrap();
        let mut reader = BufReader::new(file);
        let mut on_disk = Translation::deserialize(&mut reader).unwrap();
        let mut temp_dict = Dictionary::create("en".into(), "ru".into());
        on_disk.add_paragraph_translation(
            0,
            &make_paragraph(3, "disk", "en", "ru"),
            TranslationModel::Gemini25Flash,
            &mut temp_dict,
        );
        let file = std::fs::File::create(&translation_file).unwrap();
        let mut writer = BufWriter::new(file);
        on_disk.serialize(&mut writer).unwrap();
        writer.flush().unwrap();
    }

    book.save().await.unwrap();
}

#[tokio::test]
async fn trace_dictionary_load() {
    let temp_dir = TempDir::new("flts_trace_dictionary");
    let library_root = temp_dir.path.join("lib");
    std::fs::create_dir_all(&library_root).unwrap();

    let mut main_dict = Dictionary::create("en".into(), "ru".into());
    main_dict.add_translation("hello", "privet");
    let mut conflict_dict = Dictionary::create("en".into(), "ru".into());
    conflict_dict.add_translation("world", "mir");

    let main_path = library_root.join("dictionary_eng_rus.dat");
    let conflict_path = library_root.join("dictionary_eng_rus.syncconflict.dat");
    {
        let file = std::fs::File::create(&main_path).unwrap();
        let mut writer = BufWriter::new(file);
        main_dict.serialize(&mut writer).unwrap();
        writer.flush().unwrap();
    }
    {
        let file = std::fs::File::create(&conflict_path).unwrap();
        let mut writer = BufWriter::new(file);
        conflict_dict.serialize(&mut writer).unwrap();
        writer.flush().unwrap();
    }

    let _trace = TraceGuard::start("dictionary-load.ndjson");
    let metadata = LibraryDictionaryMetadata::load(&main_path).await.unwrap();
    let _loaded = LibraryDictionary::load_from_metadata(metadata).await.unwrap();
}
