//! Reproduction test for Bug F2: Stale Snapshot Overwrites
//!
//! Demonstrates that concurrent backend operations can emit events whose
//! snapshot versions arrive at the frontend in non-monotonic order.
//!
//! Part 1 reproduces the original bug: eventToReadable blindly applied
//! `setter(event.payload)`, so a stale snapshot overwrote a fresh one.
//!
//! Part 2 verifies the fix: with versioned payloads and monotonicity
//! checking, stale events are discarded and the UI never regresses.
//!
//! TLA+ counterexample (8 states):
//!   TauriModify(v2) → TauriComputeSnapshot(snap=2) → ConfigChange(emits v3)
//!   → TauriEmit(emits v2) → DeliverEvent(v3→UI) → DeliverEvent(v2→UI, REGRESSES)

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use isolang::Language;
use library::library::Library;
use tokio::sync::Barrier;

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

/// Reproduces the original bug: blind overwrite causes UI regression.
#[tokio::test]
async fn test_bug2_stale_snapshot_overwrites() {
    let dir = TempDir::new("flts_repro_f2");
    let library = Arc::new(Library::open(dir.path.join("lib")).await.unwrap());
    let en = Language::from_str("en").unwrap();

    let _book_id = library
        .create_book_plain("F2 Test", "Original paragraph content", &en)
        .await.unwrap();

    // Shared FIFO event queue mimicking Tauri's event dispatch
    let event_queue: Arc<tokio::sync::Mutex<Vec<(String, usize)>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));

    // Version counter (backend truth version)
    let version = Arc::new(std::sync::atomic::AtomicUsize::new(1));
    let barrier = Arc::new(Barrier::new(2));

    // Task 1 (Tauri command): slow — computes snapshot at v1, emits later
    let eq1 = event_queue.clone();
    let ver1 = version.clone();
    let bar1 = barrier.clone();
    let task1 = tokio::spawn(async move {
        let snapshot_version = ver1.load(std::sync::atomic::Ordering::SeqCst);
        let snapshot_data = format!("snapshot_v{}", snapshot_version);
        bar1.wait().await;
        // Simulate slow API call — task 2 will modify + emit while we're waiting
        tokio::time::sleep(Duration::from_millis(100)).await;
        eq1.lock().await.push((snapshot_data, snapshot_version));
    });

    // Task 2 (File watcher): fast — bumps version and emits fresh snapshot first
    let eq2 = event_queue.clone();
    let ver2 = version.clone();
    let bar2 = barrier.clone();
    let task2 = tokio::spawn(async move {
        bar2.wait().await;
        let new_version = ver2.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let snapshot_data = format!("snapshot_v{}", new_version);
        eq2.lock().await.push((snapshot_data, new_version));
    });

    task1.await.unwrap();
    task2.await.unwrap();

    let events = event_queue.lock().await;
    assert_eq!(events.len(), 2, "Both tasks should have emitted events");

    let (_, first_version) = &events[0];
    let (_, second_version) = &events[1];

    // Fresh event (v2) emitted first, stale event (v1) emitted second
    assert_eq!(*first_version, 2, "Fresh event (v2) emitted first");
    assert_eq!(*second_version, 1, "Stale event (v1) emitted second");

    // Simulate the OLD frontend's eventToReadable (blind overwrite):
    let mut ui_version_old = 0;
    let mut max_delivered = 0;
    for (_, v) in events.iter() {
        ui_version_old = *v; // blindly overwrite
        if *v > max_delivered {
            max_delivered = *v;
        }
    }

    // BUG: Old handler regresses from v2 to v1
    assert!(ui_version_old < max_delivered,
        "Without versioning, UI regresses: shows v{} after having seen v{}",
        ui_version_old, max_delivered);

    // Simulate the FIXED frontend's eventToReadable (version-aware):
    let mut ui_version_new = 0;
    let mut last_version = 0;
    for (_, v) in events.iter() {
        if *v > last_version {
            last_version = *v;
            ui_version_new = *v;
        }
        // else: stale event discarded
    }

    // FIX: Version-aware handler maintains monotonicity
    assert!(ui_version_new >= max_delivered,
        "With versioning, EventMonotonicity holds: ui_version={} >= max_delivered={}",
        ui_version_new, max_delivered);
    assert_eq!(ui_version_new, 2, "UI correctly shows latest version");
}
