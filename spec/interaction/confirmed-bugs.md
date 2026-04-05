# Confirmed Bugs — Interaction Protocol

All 4 bugs found by TLA+ model checking have been confirmed through
code audit (Phase 1), developer intent analysis (Phase 1.5), and
reproduction tests (Phase 2).

## Summary

| Bug | Name | Reproduction Level | Test Result |
|-----|------|-------------------|-------------|
| F1 | Stale Library Reference | Level 0 (black-box) | ✅ PASS |
| F2 | Stale Snapshot Overwrites | Level 0 (black-box) | ✅ PASS |
| F3 | No Shutdown Persistence | Level 0 (black-box) | ✅ PASS |
| F4 | Translation Lifecycle Atomicity | Level 2 (state injection) | ✅ PASS |

---

## F1: Stale Library Reference

**Verdict: CONFIRMED — True positive**

**Reproduction test:** `repro/tests/test_bug1_stale_library.rs`

**What it proves:** When AppState replaces its library reference (via
`update_config`), the TranslationQueue's spawned tasks retain an `Arc<Library>`
pointing to the old library. The test creates two libraries, replaces one with
the other in a mock AppState, and shows the queue still uses the old one. The
stale `Arc` has strong_count ≥ 2, proving the old library stays alive.

**Developer awareness:** Comment at `app.rs:110-111` acknowledges config
capture, but no cancellation mechanism exists. Commit `ea80c0c` fixed deadlocks
but not stale references.

**Test output:**
```
BUG F1 REPRODUCED:
  Queue library root: ".../lib_a" (stale — library A)
  AppState library root: ".../lib_b" (current — library B)
  Stale Arc strong count: 2
```

---

## F2: Stale Snapshot Overwrites

**Verdict: CONFIRMED — True positive**

**Reproduction test:** `repro/tests/test_bug2_stale_snapshot.rs`

**What it proves:** Two concurrent backend operations can emit events where
the fresh event (v2) is enqueued before the stale event (v1). Since
`eventToReadable` in `tauri.ts:10-13` blindly applies `setter(event.payload)`,
the UI regresses from v2 back to v1. The test uses a barrier + sleep to force
the timing, then simulates the frontend's blind application to show the
`EventMonotonicity` invariant is violated.

**Developer awareness:** No version/ordering awareness exists in any frontend
event handler. No TODO/FIXME about stale events.

**Test output:**
```
BUG F2 REPRODUCED:
  Event delivery order: [2, 1]
  UI version after delivery: 1 (should be >= 2)
  EventMonotonicity invariant holds: false
```

---

## F3: No Shutdown Persistence

**Verdict: CONFIRMED — True positive**

**Reproduction test:** `repro/tests/test_bug3_no_shutdown.rs`

**What it proves:** A translation added in memory (via
`add_paragraph_translation`) is lost when the library is dropped without
calling `save()`. This mirrors what happens when the Tauri app closes — there
is no `on_window_event` handler in `lib.rs` to flush pending changes. The test
creates a library, adds a translation, drops the library (simulating shutdown),
re-opens it, and confirms the translation is gone.

**Developer awareness:** No shutdown handler code, no TODO about it. The
`run_saver` task has a delay loop, creating a window where data can be lost.

**Test output:**
```
BUG F3 REPRODUCED:
  Translation added in memory before shutdown: YES
  Translation persisted to disk: NO
  Translation found after restart: NO — DATA LOST
  Root cause: lib.rs has no on_window_event shutdown handler
```

---

## F4: Translation Lifecycle Atomicity (TOCTOU)

**Verdict: CONFIRMED — True positive**

**Reproduction test:** `repro/tests/test_bug4_stale_translation.rs`

**What it proves:** Between the paragraph read (lock acquire → release at
`translation_queue.rs:237`) and the translation store (lock re-acquire at
line 330), the book can be modified by a concurrent file watcher reload. The
translation is stored unconditionally — no version check, no content hash
comparison. The test reads a paragraph, modifies the book (adding a paragraph
to simulate sync), then stores a translation and confirms it's accepted
despite the book having changed.

**Developer awareness:** Commit `44300d8` fixed a TOCTOU in dedup logic by
holding the lock, showing awareness of race conditions. But the read-modify-
write window in `handle_request` was not addressed.

**Test output:**
```
BUG F4 REPRODUCED:
  Worker read paragraph at Step 1: "The cat sat on the mat."
  Book modified at Step 2 (new paragraph added)
  Translation stored at Step 3 WITHOUT version check: accepted
  Root cause: translation_queue.rs releases lock at L237,
    re-acquires at L330 with no content/version check.
```

---

## Severity Assessment

| Bug | Impact | Trigger Probability | Severity |
|-----|--------|-------------------|----------|
| F1 | Data written to wrong library | Low (requires config change during active translation) | Medium |
| F2 | UI shows stale data until refresh | Medium (any concurrent operation + event) | Medium |
| F3 | Translation work lost on close | High (normal user closes app) | **High** |
| F4 | Translation mismatched to paragraph | Low (requires sync during translation) | Low-Medium |

**F3 is the most impactful** — it can be triggered by normal user behavior
(closing the app while translations are in progress) and results in silent
data loss.

## Recommended Fixes

1. **F1:** Replace captured `Arc<Library>` with `Arc<RwLock<Option<Arc<Library>>>>` 
   (shared reference to AppState's field), or cancel/restart tasks on config change.

2. **F2:** Add version stamping to emitted events; frontend discards events
   with version ≤ current displayed version.

3. **F3:** Add `on_window_event(WindowEvent::CloseRequested)` handler to
   `lib.rs` that calls `save()` on all dirty books/translations.

4. **F4:** Either hold the book lock through the entire read→API→store cycle
   (costly), or add a content hash / version check before storing.
