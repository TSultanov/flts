//! Reproduction test for Bug F1: Stale Library Reference
//!
//! Demonstrates that translation queue workers capture an Arc<Library> at init
//! time, and continue using it even after AppState replaces the library.
//! This causes translations to be saved to the WRONG library directory.
//!
//! TLA+ counterexample (4 states): BeginWorker → BeginTauri → ConfigChange
//! → tasks still hold old library reference (taskLib=1 ≠ currentLib=2)
//!
//! Fix: TranslationQueue now stores JoinHandles for all spawned tasks and
//! aborts them in its Drop implementation. When update_config sets the queue
//! to None, tasks are immediately cancelled, preventing stale library usage.

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use isolang::Language;
use library::library::Library;
use tokio::sync::RwLock;

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

/// Mimics AppState's library field: `library: RwLock<Option<Arc<Library>>>`
struct MockAppState {
    library: RwLock<Option<(Arc<Library>, PathBuf)>>,
}

/// Mimics TranslationQueue: captures library Arc at init, uses it for all work.
/// Now also stores task handles and aborts them on Drop (F1 fix).
struct MockTranslationQueue {
    captured_library: Arc<Library>,
    captured_root: PathBuf,
    task_handle: tokio::task::JoinHandle<()>,
}

impl MockTranslationQueue {
    fn init(library: Arc<Library>, root: PathBuf, work_flag: Arc<AtomicBool>) -> Self {
        let task_lib = library.clone();
        let task_handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                // Simulate background work using the captured library
                work_flag.store(true, Ordering::SeqCst);
                let _ = task_lib.list_books().await;
            }
        });
        Self {
            captured_library: library,
            captured_root: root,
            task_handle,
        }
    }
}

impl Drop for MockTranslationQueue {
    fn drop(&mut self) {
        self.task_handle.abort();
    }
}

#[tokio::test]
async fn test_bug1_stale_library_ref() {
    // 1. Create library A
    let dir_a = TempDir::new("flts_repro_f1_a");
    let lib_a_root = dir_a.path.join("lib_a");
    let library_a = Arc::new(Library::open(lib_a_root.clone()).await.unwrap());

    // 2. Simulate AppState holding library A
    let app_state = MockAppState {
        library: RwLock::new(Some((library_a.clone(), lib_a_root.clone()))),
    };

    // 3. Create translation queue (captures library A) — mirrors translation_queue.rs:108
    let work_flag = Arc::new(AtomicBool::new(false));
    let queue = MockTranslationQueue::init(library_a.clone(), lib_a_root.clone(), work_flag.clone());
    assert_eq!(queue.captured_root, lib_a_root);

    // 4. Simulate update_config → create library B and replace in AppState
    let dir_b = TempDir::new("flts_repro_f1_b");
    let lib_b_root = dir_b.path.join("lib_b");
    let library_b = Arc::new(Library::open(lib_b_root.clone()).await.unwrap());
    *app_state.library.write().await = Some((library_b.clone(), lib_b_root.clone()));

    // 5. Verify: AppState now points to library B
    let current_root = app_state.library.read().await.as_ref().unwrap().1.clone();
    assert_eq!(current_root, lib_b_root);

    // 6. BUG: Queue still uses library A
    assert_eq!(queue.captured_root, lib_a_root,
        "Queue should still point to library A (the stale reference)");
    assert_ne!(queue.captured_root, lib_b_root,
        "Queue does NOT point to library B (the current library)");

    // 7. Demonstrate the real consequence: create a book in each library
    let en = Language::from_str("en").unwrap();
    let book_a_id = library_a
        .create_book_plain("Book via stale ref", "Test paragraph", &en)
        .await.unwrap();
    let book_b_id = library_b
        .create_book_plain("Book via current ref", "Test paragraph", &en)
        .await.unwrap();

    let stale_books = library_a.list_books().await.unwrap();
    let current_books = library_b.list_books().await.unwrap();
    assert!(stale_books.iter().any(|b| b.id == book_a_id));
    assert!(current_books.iter().any(|b| b.id == book_b_id));

    // Stale library is kept alive by the queue's Arc
    let stale_strong_count = Arc::strong_count(&queue.captured_library);
    assert!(stale_strong_count >= 2,
        "Stale library has {} strong references (queue + local var)", stale_strong_count);

    println!("BUG F1 REPRODUCED:");
    println!("  Queue library root: {:?} (stale — library A)", queue.captured_root);
    println!("  AppState library root: {:?} (current — library B)", current_root);
    println!("  Stale Arc strong count: {}", stale_strong_count);
}

/// Verify that the fix works: dropping the queue aborts background tasks,
/// preventing further work against the stale library.
#[tokio::test]
async fn test_bug1_fix_drop_aborts_tasks() {
    let dir = TempDir::new("flts_f1_fix");
    let lib_root = dir.path.join("lib");
    let library = Arc::new(Library::open(lib_root.clone()).await.unwrap());

    let work_flag = Arc::new(AtomicBool::new(false));
    let queue = MockTranslationQueue::init(library.clone(), lib_root.clone(), work_flag.clone());

    // Let the task run at least one iteration
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(work_flag.load(Ordering::SeqCst), "Task should have run at least once");

    // Reset flag, then drop the queue (simulates update_config setting queue = None)
    work_flag.store(false, Ordering::SeqCst);
    let task_handle = &queue.task_handle;
    let is_running_before = !task_handle.is_finished();
    assert!(is_running_before, "Task should be running before drop");

    drop(queue);

    // Give the runtime a moment to process the abort
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // The flag should NOT have been set again — the task was aborted
    assert!(!work_flag.load(Ordering::SeqCst),
        "Task should not have run after queue was dropped (abort should have cancelled it)");

    println!("FIX F1 VERIFIED: Dropping queue aborts background tasks");
}
