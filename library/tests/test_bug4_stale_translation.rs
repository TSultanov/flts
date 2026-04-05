//! Reproduction test for Bug F4: Translation Lifecycle Atomicity
//!
//! Demonstrates that a book can be reloaded (by file watcher) between
//! the paragraph read and the translation store, causing the translation
//! to be stored against a different version of the paragraph content.
//!
//! TLA+ counterexample (31 states):
//!   WorkerReadParagraph(v=3) → ... → WatcherReload(book v=4)
//!   → WorkerStore (stores against v=4, but content was from v=3)

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
        timestamp: 2000,
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
async fn test_bug4_stale_translation_stored() {
    let dir = TempDir::new("flts_repro_f4");
    let lib_path = dir.path.join("lib");
    let en = Language::from_str("en").unwrap();
    let ru = Language::from_str("ru").unwrap();

    let library = Arc::new(Library::open(lib_path).await.unwrap());

    let book_id = library
        .create_book_plain("F4 Test Book", "The cat sat on the mat.", &en)
        .await.unwrap();

    // Step 1: Worker reads paragraph (translation_queue.rs:227-237)
    // Lock acquired, paragraph read, lock released — this is the critical window.
    let original_text = {
        let book_handle = library.get_book(&book_id).await.unwrap();
        let book = book_handle.lock().await;
        let paragraph = book.book.paragraph_view(0);
        let text = paragraph.original_text.to_string();
        println!("Step 1 - Worker reads paragraph: \"{}\"", text);
        assert_eq!(text, "The cat sat on the mat.");
        text
        // Lock released here
    };

    // Step 2: File watcher reloads book with different content
    // (simulated by modifying the book directly — Level 2 state injection)
    {
        let book_handle = library.get_book(&book_id).await.unwrap();
        let mut book = book_handle.lock().await;
        book.book.push_paragraph(0, "A new paragraph appeared after sync.", None);
        book.save().await.unwrap();
        println!("Step 2 - Book modified (new paragraph added, {} total)", book.book.paragraphs_count());
    }

    // Step 3: Worker stores translation based on STALE read
    // translation_queue.rs:330-334 — NO version check before storing
    {
        let book_handle = library.get_book(&book_id).await.unwrap();
        let mut book = book_handle.lock().await;

        assert_eq!(book.book.paragraphs_count(), 2, "Book should have 2 paragraphs after reload");

        let translation = book.get_or_create_translation(&ru).await;
        let mut t = translation.lock().await;

        // Translation was computed from ORIGINAL text at Step 1.
        // Book has been modified since then, but no version check prevents this.
        let stale_translation = make_translation("Кот сидел на коврике.");
        t.add_paragraph_translation(0, &stale_translation, TranslationModel::Gemini25Flash)
            .await.unwrap();

        let pv = t.paragraph_view(0).unwrap();
        let sentence = pv.sentences().next().unwrap();
        println!("Step 3 - Translation stored: \"{}\"", sentence.full_translation);
    }

    // Verify: translation was accepted despite book modification
    {
        let book_handle = library.get_book(&book_id).await.unwrap();
        let mut book = book_handle.lock().await;
        let translation = book.get_or_create_translation(&ru).await;
        let t = translation.lock().await;
        let pv = t.paragraph_view(0);

        assert!(pv.is_some(),
            "Translation was stored despite book modification — no version check");

        // The translation "Кот сидел на коврике." is for "The cat sat on the mat."
        // but the book may have been restructured between read and store.
        println!("\nBUG F4 REPRODUCED:");
        println!("  Worker read paragraph at Step 1: \"{}\"", original_text);
        println!("  Book modified at Step 2 (new paragraph added)");
        println!("  Translation stored at Step 3 WITHOUT version check: accepted");
        println!("  Root cause: translation_queue.rs releases lock at L237,");
        println!("    re-acquires at L330 with no content/version check.");
    }
}
