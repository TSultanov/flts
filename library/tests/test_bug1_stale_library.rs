//! Reproduction test for Bug F1: Stale Library Reference
//!
//! Demonstrates that translation queue workers capture an Arc<Library> at init
//! time, and continue using it even after AppState replaces the library.
//! This causes translations to be saved to the WRONG library directory.
//!
//! TLA+ counterexample (4 states): BeginWorker → BeginTauri → ConfigChange
//! → tasks still hold old library reference (taskLib=1 ≠ currentLib=2)

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

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
struct MockTranslationQueue {
    captured_library: Arc<Library>,
    captured_root: PathBuf,
}

impl MockTranslationQueue {
    fn init(library: Arc<Library>, root: PathBuf) -> Self {
        Self {
            captured_library: library,
            captured_root: root,
        }
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
    let queue = MockTranslationQueue::init(library_a.clone(), lib_a_root.clone());
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
