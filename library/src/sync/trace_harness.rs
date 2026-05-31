//! Trace-harness scenarios for the roster-mesh TLA+ spec (`spec/roster/`).
//!
//! Drives the REAL roster CRDT, reconcile, and engine glue (`SyncEngine` over a
//! `MockSyncthing`, no live Go engine) across a small simulated mesh, emitting
//! NDJSON trace events via the instrumented `engine.rs` paths. Syncthing's file
//! delivery is stood in by copying one node's `devices.json` into another node's
//! `.flts/` as a `.sync-conflict-*` sibling, exactly the input `RosterStore::load`
//! union-merges in production.
//!
//! The trace sink is a process-global, so this is ONE `#[test]` (sequential), run
//! in isolation by `harness/run.sh` (`--test-threads=1`, filtered) so no other
//! test's `tla_trace` emits land in the file.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::control::MockSyncthing;
use super::engine::SyncEngine;

/// Repo `traces/` dir (sibling of `library/`), where Trace.cfg looks by default.
fn traces_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("traces")
}

fn fresh_root(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("flts-roster-trace-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn node(id: &str, root: &Path) -> SyncEngine {
    SyncEngine::for_test(
        Arc::new(MockSyncthing::new(id)),
        id.to_string(),
        root.to_string_lossy().into_owned(),
    )
}

/// Stand in for Syncthing delivery: drop `src`'s current roster into `dst`'s
/// `.flts/` as a conflict sibling named with `src`'s id as the `modifiedBy` field.
fn deliver_roster(src_root: &Path, dst_root: &Path, src_id: &str) {
    let content = std::fs::read(src_root.join(".flts").join("devices.json"))
        .expect("src has written a roster");
    let dst_dir = dst_root.join(".flts");
    std::fs::create_dir_all(&dst_dir).unwrap();
    std::fs::write(
        dst_dir.join(format!("devices.sync-conflict-20260531-120000-{src_id}.json")),
        content,
    )
    .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn trace_roster_scenarios() {
    let dir = traces_dir();
    std::fs::create_dir_all(&dir).unwrap();

    mesh_forms(&dir.join("roster_mesh_forms.ndjson")).await;
    unpair_propagates(&dir.join("roster_unpair.ndjson")).await;
}

/// Scenario 1: hub (n1) pairs n2 and n3; roster sync + reconcile fan the pairing
/// out to a full 3-node mesh (n2 and n3 never paired directly).
/// Exercises EnsureSelf, PairOn, RosterSync, ReconcileNode.
async fn mesh_forms(trace_file: &Path) {
    crate::tla_trace::set_trace_file(trace_file).unwrap();

    let (r1, r2, r3) = (fresh_root("mf1"), fresh_root("mf2"), fresh_root("mf3"));
    let n1 = node("n1", &r1);
    let n2 = node("n2", &r2);
    let n3 = node("n3", &r3);

    // Each node names itself → seeds itself in its roster (EnsureSelf).
    n1.set_device_name("A").await.unwrap();
    n2.set_device_name("B").await.unwrap();
    n3.set_device_name("C").await.unwrap();

    // Two-sided hub pairings (PairOn; the second side is the pending-approval).
    n1.pair_device("n2", "B").await.unwrap();
    n2.pair_device("n1", "A").await.unwrap();
    n1.pair_device("n3", "C").await.unwrap();
    n3.pair_device("n1", "A").await.unwrap();

    // The hub's roster (now listing all three) reaches n2 and n3; each reconciles
    // and learns the peer it never paired with → full mesh.
    deliver_roster(&r1, &r2, "n1");
    n2.reconcile_once().await.unwrap(); // RosterSync(n1,n2) + ReconcileNode(n2)
    deliver_roster(&r1, &r3, "n1");
    n3.reconcile_once().await.unwrap(); // RosterSync(n1,n3) + ReconcileNode(n3)

    crate::tla_trace::clear_trace_file().unwrap();

    for r in [r1, r2, r3] {
        let _ = std::fs::remove_dir_all(r);
    }
}

/// Scenario 2: from a full mesh, n1 unpairs n2; the tombstone propagates to n3,
/// which reconciles and drops n2. Exercises UnpairOn + tombstone RosterSync +
/// reconcile removal.
async fn unpair_propagates(trace_file: &Path) {
    crate::tla_trace::set_trace_file(trace_file).unwrap();

    let (r1, r2, r3) = (fresh_root("up1"), fresh_root("up2"), fresh_root("up3"));
    let n1 = node("n1", &r1);
    let n2 = node("n2", &r2);
    let n3 = node("n3", &r3);

    n1.set_device_name("A").await.unwrap();
    n2.set_device_name("B").await.unwrap();
    n3.set_device_name("C").await.unwrap();

    n1.pair_device("n2", "B").await.unwrap();
    n2.pair_device("n1", "A").await.unwrap();
    n1.pair_device("n3", "C").await.unwrap();
    n3.pair_device("n1", "A").await.unwrap();

    deliver_roster(&r1, &r2, "n1");
    n2.reconcile_once().await.unwrap();
    deliver_roster(&r1, &r3, "n1");
    n3.reconcile_once().await.unwrap();

    // n1 unpairs n2 (tombstone), and the tombstone propagates to n3.
    n1.unpair_device("n2").await.unwrap(); // UnpairOn(n1,n2)
    deliver_roster(&r1, &r3, "n1");
    n3.reconcile_once().await.unwrap(); // RosterSync(n1,n3) + ReconcileNode(n3) removes n2

    crate::tla_trace::clear_trace_file().unwrap();

    for r in [r1, r2, r3] {
        let _ = std::fs::remove_dir_all(r);
    }
}
