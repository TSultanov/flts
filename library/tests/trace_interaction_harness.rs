//! Trace harness for the **interaction** spec (`spec/interaction/`).
//!
//! Exercises real Library/LibraryBook operations while emitting NDJSON events
//! that match the Trace.tla event schema. Two scenarios:
//!
//! 1. `trace_interaction_baseline` — sequential, covers all 19 event types
//! 2. `trace_interaction_concurrent` — 3 tokio tasks with real temporal overlap
//!
//! Run: `cargo test -p library trace_interaction_ -- --test-threads=1`

use std::{
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use isolang::Language;
use library::{
    book::translation_import,
    library::Library,
    tla_trace_interaction::{InteractionTraceGuard, TraceSpan},
    translator::TranslationModel,
};
use tokio::sync::Barrier;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

fn make_paragraph(
    ts: u64,
    text: &str,
    src: &str,
    tgt: &str,
) -> translation_import::ParagraphTranslation {
    translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: ts,
        source_language: src.to_owned(),
        target_language: tgt.to_owned(),
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

fn en() -> Language {
    Language::from_str("en").unwrap()
}

fn ru() -> Language {
    Language::from_str("ru").unwrap()
}

// ---------------------------------------------------------------------------
// Scenario 1: Baseline — sequential, covers all 19 event types
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trace_interaction_baseline() {
    let _trace = InteractionTraceGuard::start("interaction-baseline.ndjson");
    let temp = TempDir::new("interaction_baseline");
    let lib_root = temp.path.join("lib");

    // === ConfigChange: simulate reconfiguration by opening a new library ===
    let span = TraceSpan::begin("t1", "ConfigChange")
        .field("task", "t1");
    let _library2 = Library::open(lib_root.clone()).await.unwrap();
    span.end();

    // === Setup: create library + book + paragraph ===
    let library = Library::open(lib_root.clone()).await.unwrap();
    let book = library.create_book("Baseline Book", &en()).await.unwrap();
    let book_id = {
        let mut b = book.lock().await;
        b.book.push_chapter(Some("Intro"));
        b.book.push_paragraph(0, "Hello world, this is a test.", None);
        b.save().await.unwrap();
        b.book.id
    };

    // === Worker lifecycle (t1) ===

    // BeginWorker: capture library reference
    let span = TraceSpan::begin("t1", "BeginWorker")
        .field("task", "t1")
        .field("book", "b1")
        .field("lib", 1);
    let book_handle = library.get_book(&book_id).await.unwrap();
    span.end();

    // WorkerReadParagraph: get_or_create_translation + paragraph_view
    let span = TraceSpan::begin("t1", "WorkerReadParagraph")
        .field("task", "t1");
    let translation = {
        let mut b = book_handle.lock().await;
        let tr = b.get_or_create_translation(&ru()).await;
        let _pv = b.book.paragraph_view(0);
        tr
    };
    span.end();

    // WorkerCallAPI: simulated external call
    let span = TraceSpan::begin("t1", "WorkerCallAPI")
        .field("task", "t1");
    tokio::time::sleep(Duration::from_millis(2)).await;
    span.end();

    // WorkerStoreResult: add_paragraph_translation
    let span = TraceSpan::begin("t1", "WorkerStoreResult")
        .field("task", "t1");
    translation
        .lock()
        .await
        .add_paragraph_translation(
            0,
            &make_paragraph(100, "Привет мир", "en", "ru"),
            TranslationModel::Gemini25Flash,
        )
        .await
        .unwrap();
    span.end();

    // WorkerSave: book.save()
    let span = TraceSpan::begin("t1", "WorkerSave")
        .field("task", "t1")
        .field("lib", 1);
    {
        let mut b = book_handle.lock().await;
        b.save().await.unwrap();
    }
    span.end();

    // WorkerComputeSnapshot: list_books
    let span = TraceSpan::begin("t1", "WorkerComputeSnapshot")
        .field("task", "t1");
    let _books = library.list_books().await.unwrap();
    span.end();

    // WorkerEmit: emit library_updated
    TraceSpan::begin("t1", "WorkerEmit")
        .field("task", "t1")
        .end();

    // === Tauri command lifecycle (t2) ===

    // BeginTauri: import a new book
    let span = TraceSpan::begin("t2", "BeginTauri")
        .field("task", "t2")
        .field("book", "b1")
        .field("lib", 1);
    // (just routing — real work in TauriModify)
    span.end();

    // TauriModify: create_book
    let span = TraceSpan::begin("t2", "TauriModify")
        .field("task", "t2");
    let _new_book_id = library
        .create_book_plain("Second Book", "Some text content.", &en())
        .await
        .unwrap();
    span.end();

    // TauriComputeSnapshot: list_books
    let span = TraceSpan::begin("t2", "TauriComputeSnapshot")
        .field("task", "t2");
    let _books = library.list_books().await.unwrap();
    span.end();

    // TauriEmit
    TraceSpan::begin("t2", "TauriEmit")
        .field("task", "t2")
        .end();

    // === File watcher lifecycle (t1 reused) ===

    // BeginWatcher
    let span = TraceSpan::begin("t1", "BeginWatcher")
        .field("task", "t1")
        .field("book", "b1")
        .field("lib", 1);
    let _book_handle = library.get_book(&book_id).await.unwrap();
    span.end();

    // WatcherReload: simulate external file modification + reload
    let span = TraceSpan::begin("t1", "WatcherReload")
        .field("task", "t1");
    {
        // Modify book file on disk to simulate sync conflict
        let book_dir = lib_root.join(book_id.to_string());
        let book_file = book_dir.join("book.dat");
        if book_file.exists() {
            // Touch the file to trigger reload detection
            let content = tokio::fs::read(&book_file).await.unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
            tokio::fs::write(&book_file, &content).await.unwrap();
        }
    }
    span.end();

    // WatcherComputeSnapshot
    let span = TraceSpan::begin("t1", "WatcherComputeSnapshot")
        .field("task", "t1");
    let _books = library.list_books().await.unwrap();
    span.end();

    // WatcherEmit
    TraceSpan::begin("t1", "WatcherEmit")
        .field("task", "t1")
        .end();

    // === DeliverEvent (ui actor, 3 events from worker/tauri/watcher emits) ===
    TraceSpan::begin("ui", "DeliverEvent")
        .field("version", 1)
        .end();
    TraceSpan::begin("ui", "DeliverEvent")
        .field("version", 2)
        .end();
    TraceSpan::begin("ui", "DeliverEvent")
        .field("version", 3)
        .end();

    // === MarkWordVisible ===
    let span = TraceSpan::begin("t2", "MarkWordVisible")
        .field("task", "t2")
        .field("book", "b1");
    {
        let mut b = book_handle.lock().await;
        let tr = b.get_or_create_translation(&ru()).await;
        tr.lock().await.mark_word_visible(0, 0);
        b.save().await.unwrap();
    }
    span.end();

    // === AppClose ===
    TraceSpan::begin("t1", "AppClose")
        .field("task", "t1")
        .end();
}

// ---------------------------------------------------------------------------
// Scenario 2: Concurrent — 3 tasks with real temporal overlap
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trace_interaction_concurrent() {
    let _trace = InteractionTraceGuard::start("interaction-concurrent.ndjson");
    let temp = TempDir::new("interaction_concurrent");
    let lib_root = temp.path.join("lib");

    // === Setup ===
    let library = Arc::new(Library::open(lib_root.clone()).await.unwrap());
    let book = library.create_book("Concurrent Book", &en()).await.unwrap();
    let book_id = {
        let mut b = book.lock().await;
        b.book.push_chapter(Some("Ch1"));
        b.book.push_paragraph(0, "First paragraph.", None);
        b.book.push_paragraph(0, "Second paragraph.", None);
        b.save().await.unwrap();
        b.book.id
    };

    let barrier = Arc::new(Barrier::new(3));

    // --- Task t1: Worker lifecycle (slow API call) ---
    let lib1 = library.clone();
    let bar1 = barrier.clone();
    let bid1 = book_id;
    let t1 = tokio::spawn(async move {
        bar1.wait().await; // synchronize start

        // BeginWorker
        let span = TraceSpan::begin("t1", "BeginWorker")
            .field("task", "t1")
            .field("book", "b1")
            .field("lib", 1);
        let bh = lib1.get_book(&bid1).await.unwrap();
        span.end();

        // WorkerReadParagraph
        let span = TraceSpan::begin("t1", "WorkerReadParagraph")
            .field("task", "t1");
        let tr = {
            let mut b = bh.lock().await;
            let tr = b.get_or_create_translation(&ru()).await;
            let _pv = b.book.paragraph_view(0);
            tr
        };
        span.end();

        // WorkerCallAPI — long async call, other tasks interleave here
        let span = TraceSpan::begin("t1", "WorkerCallAPI")
            .field("task", "t1");
        tokio::time::sleep(Duration::from_millis(20)).await;
        span.end();

        // WorkerStoreResult
        let span = TraceSpan::begin("t1", "WorkerStoreResult")
            .field("task", "t1");
        tr.lock()
            .await
            .add_paragraph_translation(
                0,
                &make_paragraph(200, "Первый абзац", "en", "ru"),
                TranslationModel::Gemini25Flash,
            )
            .await
            .unwrap();
        span.end();

        // WorkerSave
        let span = TraceSpan::begin("t1", "WorkerSave")
            .field("task", "t1")
            .field("lib", 1);
        bh.lock().await.save().await.unwrap();
        span.end();

        // WorkerComputeSnapshot
        let span = TraceSpan::begin("t1", "WorkerComputeSnapshot")
            .field("task", "t1");
        let _books = lib1.list_books().await.unwrap();
        span.end();

        // WorkerEmit
        TraceSpan::begin("t1", "WorkerEmit")
            .field("task", "t1")
            .end();
    });

    // --- Task t2: Tauri command (fast, runs during t1's API call) ---
    let lib2 = library.clone();
    let bar2 = barrier.clone();
    let t2 = tokio::spawn(async move {
        bar2.wait().await; // synchronize start

        // Small delay to ensure t1 has started its API call
        tokio::time::sleep(Duration::from_millis(5)).await;

        // BeginTauri
        let span = TraceSpan::begin("t2", "BeginTauri")
            .field("task", "t2")
            .field("book", "b1")
            .field("lib", 1);
        span.end();

        // TauriModify (import a book)
        let span = TraceSpan::begin("t2", "TauriModify")
            .field("task", "t2");
        let _new = lib2
            .create_book_plain("Imported Book", "Fresh content.", &en())
            .await
            .unwrap();
        span.end();

        // TauriComputeSnapshot
        let span = TraceSpan::begin("t2", "TauriComputeSnapshot")
            .field("task", "t2");
        let _books = lib2.list_books().await.unwrap();
        span.end();

        // TauriEmit
        TraceSpan::begin("t2", "TauriEmit")
            .field("task", "t2")
            .end();
    });

    // --- Task t3: Watcher (also runs during t1's API call) ---
    let lib3 = library.clone();
    let bar3 = barrier.clone();
    let bid3 = book_id;
    let lib_root3 = lib_root.clone();
    let t3 = tokio::spawn(async move {
        bar3.wait().await; // synchronize start

        // Slightly later start
        tokio::time::sleep(Duration::from_millis(8)).await;

        // BeginWatcher
        let span = TraceSpan::begin("t3", "BeginWatcher")
            .field("task", "t3")
            .field("book", "b1")
            .field("lib", 1);
        let _bh = lib3.get_book(&bid3).await.unwrap();
        span.end();

        // WatcherReload (simulate file modification)
        let span = TraceSpan::begin("t3", "WatcherReload")
            .field("task", "t3");
        // Touch the book file to simulate external sync
        let book_dir = lib_root3.join(bid3.to_string());
        let book_file = book_dir.join("book.dat");
        if book_file.exists() {
            let content = tokio::fs::read(&book_file).await.unwrap();
            tokio::time::sleep(Duration::from_millis(2)).await;
            tokio::fs::write(&book_file, &content).await.unwrap();
        }
        span.end();

        // WatcherComputeSnapshot
        let span = TraceSpan::begin("t3", "WatcherComputeSnapshot")
            .field("task", "t3");
        let _books = lib3.list_books().await.unwrap();
        span.end();

        // WatcherEmit
        TraceSpan::begin("t3", "WatcherEmit")
            .field("task", "t3")
            .end();
    });

    // Wait for all tasks to complete
    t1.await.unwrap();
    t2.await.unwrap();
    t3.await.unwrap();

    // --- DeliverEvent: UI receives events in FIFO order ---
    // In the concurrent scenario, t2 and t3 emit BEFORE t1 finishes,
    // so their events are delivered first.
    TraceSpan::begin("ui", "DeliverEvent")
        .field("version", 1)
        .end();
    TraceSpan::begin("ui", "DeliverEvent")
        .field("version", 2)
        .end();
    TraceSpan::begin("ui", "DeliverEvent")
        .field("version", 3)
        .end();
}
