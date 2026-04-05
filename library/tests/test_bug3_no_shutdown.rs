//! Reproduction test for Bug F3: No Shutdown Persistence
//!
//! Demonstrates that in-memory book modifications are lost when the app
//! terminates without explicit save. The Tauri builder in lib.rs has no
//! on_window_event handler, so in-flight translations are discarded.
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

#[tokio::test]
async fn test_bug3_no_shutdown_persistence() {
    let dir = TempDir::new("flts_repro_f3");
    let lib_path = dir.path.join("lib");

    let en = Language::from_str("en").unwrap();
    let ru = Language::from_str("ru").unwrap();

    // Phase 1: Create library, book, add translation to memory, DON'T save
    let book_id = {
        let library = Arc::new(Library::open(lib_path.clone()).await.unwrap());

        let book_id = library
            .create_book_plain("F3 Test Book", "This is a test paragraph.", &en)
            .await.unwrap();

        // Add translation IN MEMORY ONLY — mirrors the window between
        // add_paragraph_translation (line 330) and run_saver processing (line 356)
        {
            let book_handle = library.get_book(&book_id).await.unwrap();
            let mut book = book_handle.lock().await;
            let translation = book.get_or_create_translation(&ru).await;
            let mut t = translation.lock().await;
            t.add_paragraph_translation(
                0,
                &make_translation("Это тестовый абзац."),
                TranslationModel::Gemini25Flash,
            ).await.unwrap();

            let pv = t.paragraph_view(0);
            assert!(pv.is_some(), "Translation should exist in memory before 'shutdown'");
            println!("Before shutdown: translation exists in memory: {}", pv.is_some());

            // DELIBERATELY NOT CALLING save()
            // Simulates: translation completes → save queued → user closes app
            // → no on_window_event handler → data lost
        }

        book_id
    };
    // All Arc<Library> dropped — simulates app termination

    // Phase 2: Re-open the library (simulates restart)
    {
        let library = Arc::new(Library::open(lib_path).await.unwrap());
        let book_handle = library.get_book(&book_id).await.unwrap();
        let mut book = book_handle.lock().await;
        let translation = book.get_or_create_translation(&ru).await;
        let t = translation.lock().await;
        let pv = t.paragraph_view(0);

        // BUG: Translation is GONE — data lost on "shutdown"
        assert!(pv.is_none(),
            "BUG F3 REPRODUCED: Translation lost on shutdown — no on_exit handler to flush");

        println!("BUG F3 REPRODUCED:");
        println!("  Translation added in memory before shutdown: YES");
        println!("  Translation persisted to disk: NO");
        println!("  Translation found after restart: NO — DATA LOST");
        println!("  Root cause: lib.rs has no on_window_event shutdown handler");
    }
}
