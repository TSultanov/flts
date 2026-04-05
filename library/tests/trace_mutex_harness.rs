/// Trace harness for TLA+ mutex / lock safety spec.
///
/// These tests exercise the real lock patterns in the library crate. The
/// `TracedMutex` wrapper automatically emits per-task NDJSON trace events
/// (AcqBook/RelBook, AcqTrans/RelTrans, AcqDict/RelDict) whenever the
/// trace collector is initialized and a `TASK_CTX` is set on the current task.
///
/// Run: `FLTS_MUTEX_TRACE_DIR=traces/mutex cargo test -q -p library trace_mutex_ -- --test-threads=1`
use std::path::PathBuf;
use std::sync::Arc;

use isolang::Language;
use library::{
    library::Library,
    tla_trace_mutex::{self, TaskCtx, TASK_CTX},
};

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

fn trace_dir() -> PathBuf {
    std::env::var_os("FLTS_MUTEX_TRACE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("flts_mutex_traces"))
}

// ---------------------------------------------------------------------------
// Scenario 1: Concurrent save + list (Family 1 — contention)
//
// t1 (Saver): acquires bookLock, calls save() which internally acquires
//             transLock and dictLock (nested). Holds book lock for extended period.
// t2 (List):  waits for bookLock, then calls get_or_create_translation
//             which triggers the double-lock pattern (Family 3).
// t3 (List):  same as t2, concurrent with t2.
//
// All AcqBook/RelBook, AcqTrans/RelTrans, AcqDict/RelDict events are
// emitted automatically by TracedMutex — no manual instrumentation needed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trace_mutex_concurrent_save_and_list() {
    tla_trace_mutex::init();
    tla_trace_mutex::reset();

    let temp = TempDir::new("flts_mutex_save_list");
    let library = Arc::new(Library::open(temp.path.join("lib")).await.unwrap());

    let eng = Language::from_639_3("eng").unwrap();
    let fra = Language::from_639_3("fra").unwrap();

    // Create a book with some content and a translation
    let book_id = library
        .create_book_plain("Test Book", "Hello world.\nGoodbye world.", &eng)
        .await
        .unwrap();

    // Pre-create a translation so the saver has something to iterate
    {
        let book_arc = library.get_book(&book_id).await.unwrap();
        let mut book = book_arc.lock().await;
        let _trans = book.get_or_create_translation(&fra).await;
        book.save().await.unwrap();
    }

    tla_trace_mutex::reset(); // clear setup events

    // --- Task t1: Saver ---
    let lib1 = library.clone();
    let bid1 = book_id;
    let t1 = tokio::spawn(TASK_CTX.scope(
        TaskCtx {
            task_id: "t1".into(),
            role: "saver".into(),
        },
        async move {
            let book_arc = lib1.get_book(&bid1).await.unwrap();
            // TracedMutex emits AcqBook here
            let mut book = book_arc.lock().await;
            // save() internally locks translations + dictionaries
            book.save().await.unwrap();
            drop(book);
            // TracedMutex emits RelBook on guard drop
        },
    ));

    // Small delay so saver grabs book lock first
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;

    // --- Task t2: TauriList ---
    let lib2 = library.clone();
    let bid2 = book_id;
    let t2 = tokio::spawn(TASK_CTX.scope(
        TaskCtx {
            task_id: "t2".into(),
            role: "tauri_list".into(),
        },
        async move {
            let book_arc = lib2.get_book(&bid2).await.unwrap();
            // Will block until t1 releases book lock, then AcqBook emitted
            let mut book = book_arc.lock().await;
            let fra = Language::from_639_3("fra").unwrap();
            // get_or_create_translation does double-lock (AcqTrans/RelTrans twice)
            let trans_arc = book.get_or_create_translation(&fra).await;
            // Paragraph access: another AcqTrans/RelTrans
            let _guard = trans_arc.lock().await;
            drop(_guard);
            drop(book);
        },
    ));

    // --- Task t3: TauriList ---
    let lib3 = library.clone();
    let bid3 = book_id;
    let t3 = tokio::spawn(TASK_CTX.scope(
        TaskCtx {
            task_id: "t3".into(),
            role: "tauri_list".into(),
        },
        async move {
            let book_arc = lib3.get_book(&bid3).await.unwrap();
            let mut book = book_arc.lock().await;
            let fra = Language::from_639_3("fra").unwrap();
            let trans_arc = book.get_or_create_translation(&fra).await;
            let _guard = trans_arc.lock().await;
            drop(_guard);
            drop(book);
        },
    ));

    t1.await.unwrap();
    t2.await.unwrap();
    t3.await.unwrap();

    let out = trace_dir().join("concurrent_save_list");
    tla_trace_mutex::write_per_task_traces(&out).unwrap();
}

// ---------------------------------------------------------------------------
// Scenario 2: Watcher reload + TauriMark (cross-role contention)
//
// t1 (Watcher): acquires bookLock, accesses translation (nested trans lock),
//               then dictionary (via add_paragraph_translation).
// t2 (TauriMark): acquires bookLock, accesses trans lock, marks word, saves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trace_mutex_watcher_and_mark() {
    tla_trace_mutex::init();
    tla_trace_mutex::reset();

    let temp = TempDir::new("flts_mutex_watcher_mark");
    let library = Arc::new(Library::open(temp.path.join("lib")).await.unwrap());

    let eng = Language::from_639_3("eng").unwrap();
    let fra = Language::from_639_3("fra").unwrap();

    let book_id = library
        .create_book_plain("Watcher Test", "Paragraph one.\nParagraph two.", &eng)
        .await
        .unwrap();

    // Pre-create translation
    {
        let book_arc = library.get_book(&book_id).await.unwrap();
        let mut book = book_arc.lock().await;
        let _trans = book.get_or_create_translation(&fra).await;
        book.save().await.unwrap();
    }

    tla_trace_mutex::reset();

    // --- Task t1: Watcher ---
    let lib1 = library.clone();
    let bid1 = book_id;
    let t1 = tokio::spawn(TASK_CTX.scope(
        TaskCtx {
            task_id: "t1".into(),
            role: "watcher".into(),
        },
        async move {
            let book_arc = lib1.get_book(&bid1).await.unwrap();
            let mut book = book_arc.lock().await;

            // Access translation (nested lock under book)
            let fra = Language::from_639_3("fra").unwrap();
            let trans_arc = book.get_or_create_translation(&fra).await;
            let _tguard = trans_arc.lock().await;
            // Simulate processing
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            drop(_tguard);

            drop(book);
        },
    ));

    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    // --- Task t2: TauriMark ---
    let lib2 = library.clone();
    let bid2 = book_id;
    let t2 = tokio::spawn(TASK_CTX.scope(
        TaskCtx {
            task_id: "t2".into(),
            role: "tauri_mark".into(),
        },
        async move {
            let book_arc = lib2.get_book(&bid2).await.unwrap();
            let mut book = book_arc.lock().await;

            let fra = Language::from_639_3("fra").unwrap();
            let trans_arc = book.get_or_create_translation(&fra).await;
            let mut tguard = trans_arc.lock().await;
            let _ = tguard.mark_word_visible(0, 0);
            drop(tguard);

            book.save().await.unwrap();
            drop(book);
        },
    ));

    t1.await.unwrap();
    t2.await.unwrap();

    let out = trace_dir().join("watcher_and_mark");
    tla_trace_mutex::write_per_task_traces(&out).unwrap();
}

// ---------------------------------------------------------------------------
// Scenario 3: Pure get_or_create_translation (Family 3 double-lock isolation)
//
// Single task exercises the double-lock pattern in isolation. TracedMutex
// automatically captures each lock-release-reacquire pair.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trace_mutex_double_lock_pattern() {
    tla_trace_mutex::init();
    tla_trace_mutex::reset();

    let temp = TempDir::new("flts_mutex_double_lock");
    let library = Arc::new(Library::open(temp.path.join("lib")).await.unwrap());

    let eng = Language::from_639_3("eng").unwrap();
    let fra = Language::from_639_3("fra").unwrap();

    let book_id = library
        .create_book_plain("Double Lock Test", "Test paragraph.", &eng)
        .await
        .unwrap();

    // Pre-create translation
    {
        let book_arc = library.get_book(&book_id).await.unwrap();
        let mut book = book_arc.lock().await;
        let _trans = book.get_or_create_translation(&fra).await;
        book.save().await.unwrap();
    }

    tla_trace_mutex::reset();

    let lib1 = library.clone();
    let bid1 = book_id;
    let t1 = tokio::spawn(TASK_CTX.scope(
        TaskCtx {
            task_id: "t1".into(),
            role: "tauri_list".into(),
        },
        async move {
            let book_arc = lib1.get_book(&bid1).await.unwrap();
            let mut book = book_arc.lock().await;

            // get_or_create_translation: double-lock pattern (2× AcqTrans/RelTrans)
            let fra = Language::from_639_3("fra").unwrap();
            let trans_arc = book.get_or_create_translation(&fra).await;

            // Explicit paragraph read (3rd AcqTrans/RelTrans)
            let _guard = trans_arc.lock().await;
            drop(_guard);

            drop(book);
        },
    ));

    t1.await.unwrap();

    let out = trace_dir().join("double_lock");
    tla_trace_mutex::write_per_task_traces(&out).unwrap();
}
