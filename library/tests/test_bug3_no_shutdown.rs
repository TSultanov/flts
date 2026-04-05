//! Reproduction test for Bug F3: No Shutdown Persistence
//!
//! Demonstrates that in-memory book modifications are lost when the app
//! terminates without explicit save, AND that `Library::save_all()` fixes it.
//!
//! TLA+ counterexample (4 states):
//!   BeginWorker → WorkerReadParagraph (memVersion=1) → AppClose
//!   → diskVersion still 0, data lost

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use isolang::Language;
use library::book::translation_import;
use library::library::Library;
use library::translator::TranslationModel;

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

fn make_translation(text: &str) -> translation_import::ParagraphTranslation {
    translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: 1000,
        source_language: "eng".to_owned(),
        target_language: "rus".to_owned(),
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

/// Helper: create a library with a book containing an unsaved in-memory translation.
/// Returns (library, book_id).
async fn setup_dirty_library(lib_path: &PathBuf) -> (Arc<Library>, uuid::Uuid) {
    let en = Language::from_str("en").unwrap();
    let ru = Language::from_str("ru").unwrap();

    let library = Arc::new(Library::open(lib_path.clone()).await.unwrap());

    let book_id = library
        .create_book_plain("F3 Test Book", "This is a test paragraph.", &en)
        .await
        .unwrap();

    // Add translation IN MEMORY ONLY — mirrors the window between
    // add_paragraph_translation and run_saver processing
    {
        let book_handle = library.get_book(&book_id).await.unwrap();
        let mut book = book_handle.lock().await;
        let translation = book.get_or_create_translation(&ru).await;
        let mut t = translation.lock().await;
        t.add_paragraph_translation(
            0,
            &make_translation("Это тестовый абзац."),
            TranslationModel::Gemini25Flash,
        )
        .await
        .unwrap();

        let pv = t.paragraph_view(0);
        assert!(
            pv.is_some(),
            "Translation should exist in memory before 'shutdown'"
        );
    }

    (library, book_id)
}

#[tokio::test]
async fn test_bug3_no_shutdown_persistence() {
    let dir = TempDir::new("flts_repro_f3");
    let lib_path = dir.path.join("lib");

    let ru = Language::from_str("ru").unwrap();

    // --- Part 1: Original bug — data lost without save_all() ---
    let book_id = {
        let (library, book_id) = setup_dirty_library(&lib_path).await;
        // Drop library WITHOUT calling save_all() — simulates old shutdown behavior
        drop(library);
        book_id
    };

    {
        let library = Arc::new(Library::open(lib_path.clone()).await.unwrap());
        let book_handle = library.get_book(&book_id).await.unwrap();
        let mut book = book_handle.lock().await;
        let translation = book.get_or_create_translation(&ru).await;
        let t = translation.lock().await;
        let pv = t.paragraph_view(0);

        assert!(
            pv.is_none(),
            "BUG F3: Translation should be lost without save_all()"
        );
        println!("Part 1 — BUG F3 REPRODUCED: translation lost on shutdown without save_all()");
    }

    // --- Part 2: Fix — data preserved with save_all() ---
    let lib_path2 = dir.path.join("lib2");
    let book_id2 = {
        let (library, book_id) = setup_dirty_library(&lib_path2).await;
        // Call save_all() before dropping — simulates RunEvent::Exit handler
        library.save_all().await;
        drop(library);
        book_id
    };

    {
        let library = Arc::new(Library::open(lib_path2).await.unwrap());
        let book_handle = library.get_book(&book_id2).await.unwrap();
        let mut book = book_handle.lock().await;
        let translation = book.get_or_create_translation(&ru).await;
        let t = translation.lock().await;
        let pv = t.paragraph_view(0);

        assert!(
            pv.is_some(),
            "FIX VERIFIED: Translation should survive shutdown with save_all()"
        );
        println!("Part 2 — FIX VERIFIED: translation preserved after save_all() + restart");
    }
}
