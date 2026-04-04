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
    library::Library,
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

fn sleep_for_mtime_tick() {
    std::thread::sleep(Duration::from_millis(5));
}

fn write_book(path: &Path, book: &Book) {
    let file = std::fs::File::create(path).unwrap();
    let mut writer = BufWriter::new(file);
    book.serialize(&mut writer).unwrap();
    writer.flush().unwrap();
}

fn read_book(path: &Path) -> Book {
    let file = std::fs::File::open(path).unwrap();
    let mut reader = BufReader::new(file);
    Book::deserialize(&mut reader).unwrap()
}

fn read_book_paragraphs(book: &Book) -> Vec<String> {
    let mut paragraphs = Vec::new();
    for chapter in book.chapter_views() {
        for paragraph in chapter.paragraphs() {
            paragraphs.push(paragraph.original_text.into_owned());
        }
    }
    paragraphs
}

fn append_book_paragraph(path: &Path, text: &str) {
    let mut book = read_book(path);
    if book.chapter_count() == 0 {
        book.push_chapter(Some("Intro"));
    }
    book.push_paragraph(0, text, None);
    write_book(path, &book);
}

fn overwrite_book_title(path: &Path, title: &str) {
    let mut book = read_book(path);
    book.title = title.to_owned();
    write_book(path, &book);
}

fn make_paragraph(
    ts: u64,
    text: &str,
    source_language: &str,
    target_language: &str,
) -> translation_import::ParagraphTranslation {
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

fn read_translation(path: &Path) -> Translation {
    let file = std::fs::File::open(path).unwrap();
    let mut reader = BufReader::new(file);
    Translation::deserialize(&mut reader).unwrap()
}

fn write_translation(path: &Path, translation: &Translation) {
    let file = std::fs::File::create(path).unwrap();
    let mut writer = BufWriter::new(file);
    translation.serialize(&mut writer).unwrap();
    writer.flush().unwrap();
}

#[tokio::test]
async fn repro_book_save_overwrite_discards_unsaved_memory_edit() {
    let temp_dir = TempDir::new("flts_bug_confirm_save");
    let library_root = temp_dir.path.join("lib");
    let library = Library::open(library_root.clone()).await.unwrap();

    let book = library
        .create_book("Original", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();

    let book_id = {
        let mut book = book.lock().await;
        book.book.push_chapter(Some("Intro"));
        book.book.push_paragraph(0, "base paragraph", None);
        book.save().await.unwrap();
        book.book.title = "Memory Edit".into();
        book.book.id
    };

    let book_path = library_root.join(book_id.to_string()).join("book.dat");
    sleep_for_mtime_tick();
    overwrite_book_title(&book_path, "Disk Edit");

    let in_memory_after_save = {
        let mut book = book.lock().await;
        book.save().await.unwrap();
        book.book.title.clone()
    };
    let on_disk_after_save = read_book(&book_path).title;

    println!("BUG FIXED: in-memory book edit survives save when disk is newer");
    println!("in_memory_after_save={in_memory_after_save:?}");
    println!("on_disk_after_save={on_disk_after_save:?}");

    assert_eq!(in_memory_after_save, "Memory Edit");
    assert_eq!(on_disk_after_save, "Memory Edit");
}

#[tokio::test]
async fn repro_book_conflict_newest_wins_loses_independent_edit() {
    let temp_dir = TempDir::new("flts_bug_confirm_book_conflict");
    let library_root = temp_dir.path.join("lib");
    let library = Library::open(library_root.clone()).await.unwrap();

    let book = library
        .create_book("Conflict Base", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();

    let book_id = {
        let mut book = book.lock().await;
        book.book.push_chapter(Some("Intro"));
        book.book.push_paragraph(0, "base paragraph", None);
        book.save().await.unwrap();
        book.book.id
    };

    let book_dir = library_root.join(book_id.to_string());
    let book_path = book_dir.join("book.dat");
    let conflict_path = book_dir.join("book.syncconflict-copy.dat");

    std::fs::copy(&book_path, &conflict_path).unwrap();
    append_book_paragraph(&book_path, "main-only edit");
    sleep_for_mtime_tick();
    append_book_paragraph(&conflict_path, "conflict-only edit");

    drop(library);

    let library = Library::open(library_root).await.unwrap();
    let loaded = library.get_book(&book_id).await.unwrap();
    let loaded = loaded.lock().await;
    let paragraphs = read_book_paragraphs(&loaded.book);

    println!("BUG REPRODUCED: newest conflict sibling replaced the main book instead of merging");
    println!("expected both edits to survive, loaded_paragraphs={paragraphs:?}");

    assert!(paragraphs.iter().any(|p| p == "base paragraph"));
    assert!(paragraphs.iter().any(|p| p == "conflict-only edit"));
    assert!(!paragraphs.iter().any(|p| p == "main-only edit"));
}

#[tokio::test]
async fn repro_translation_same_timestamp_conflict_collapses_distinct_version() {
    let temp_dir = TempDir::new("flts_bug_confirm_translation_conflict");
    let library_root = temp_dir.path.join("lib");
    let library = Library::open(library_root.clone()).await.unwrap();

    let source_language = Language::from_str("en").unwrap();
    let target_language = Language::from_str("ru").unwrap();

    let book = library
        .create_book("Translation Base", &source_language)
        .await
        .unwrap();

    let book_id = {
        let mut book = book.lock().await;
        book.book.push_chapter(Some("Intro"));
        book.book.push_paragraph(0, "source paragraph", None);

        let translation = book.get_or_create_translation(&target_language).await;
        translation
            .lock()
            .await
            .add_paragraph_translation(
                0,
                &make_paragraph(
                    1,
                    "main version",
                    source_language.to_639_3(),
                    target_language.to_639_3(),
                ),
                TranslationModel::Gemini25Flash,
            )
            .await
            .unwrap();
        book.save().await.unwrap();
        book.book.id
    };

    let book_dir = library_root.join(book_id.to_string());
    let main_path = book_dir.join(format!(
        "translation_{}_{}.dat",
        source_language.to_639_3(),
        target_language.to_639_3()
    ));
    let main_translation = read_translation(&main_path);
    let conflict_path = book_dir.join(format!(
        "translation_{}_{}.syncconflict-copy.dat",
        source_language.to_639_3(),
        target_language.to_639_3()
    ));

    let mut conflict_translation = Translation::create(
        source_language.to_639_3(),
        target_language.to_639_3(),
    );
    conflict_translation.id = main_translation.id;
    conflict_translation.add_paragraph_translation(
        0,
        &make_paragraph(
            1,
            "conflict version",
            source_language.to_639_3(),
            target_language.to_639_3(),
        ),
        TranslationModel::Gemini25Flash,
        &mut Dictionary::create(
            source_language.to_639_3().to_owned(),
            target_language.to_639_3().to_owned(),
        ),
    );
    write_translation(&conflict_path, &conflict_translation);

    drop(library);

    let library = Library::open(library_root).await.unwrap();
    let loaded = library.get_book(&book_id).await.unwrap();
    let mut loaded = loaded.lock().await;
    let translation = loaded.get_or_create_translation(&target_language).await;
    let translation = translation.lock().await;
    let latest = translation.paragraph_view(0).unwrap();
    let latest_text = latest.sentence_view(0).full_translation.to_string();
    let previous = latest.get_previous_version();

    println!("BUG FIXED: both translation versions survive despite same timestamp");
    println!(
        "latest={latest_text:?}, has_previous_version={}",
        previous.is_some()
    );

    // Both versions must survive: one as latest, one as previous.
    assert!(previous.is_some());
    let prev = previous.unwrap();
    let prev_text = prev.sentence_view(0).full_translation.to_string();
    let texts: Vec<&str> = vec![&latest_text, &prev_text];
    assert!(texts.contains(&"main version"));
    assert!(texts.contains(&"conflict version"));
    // The collision must have been resolved by bumping one timestamp.
    assert_ne!(latest.timestamp, prev.timestamp);
}
