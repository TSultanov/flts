# Bug Report — FLTS Frontend-Backend Interaction

## Summary

- Bug families tested: 4
- Bugs found: 4 (F1, F2, F3, F4 — all confirmed)
- Configs run: MC_hunt_f1.cfg, MC_hunt_f2.cfg, MC_hunt_f3.cfg, MC_hunt_f4.cfg

---

## Bug 1: Stale Library Reference (F1)

- **Bug Family**: F1 — Stale Library Reference
- **Severity**: High
- **Invariant violated**: MCStaleLibrarySafety
- **Config**: MC_hunt_f1.cfg (simulation)
- **Counterexample**: 4 states, output file `output/MC_hunt_f1_sim.out`

### Trace Summary

1. **State 1**: Initial — both tasks idle, `currentLib=1`
2. **State 2**: `BeginWorker(t2, b1)` — worker starts, captures `taskLib[t2]=1`
3. **State 3**: `BeginTauri(t1, b1)` — tauri command starts, captures `taskLib[t1]=1`
4. **State 4**: `ConfigChange` — user opens new library, `currentLib` advances to 2. Both t1 and t2 still hold `taskLib=1` (stale reference)

The invariant fires because `taskLib[t2]=1 ≠ currentLib=2` while t2 is active (worker at `w_read`).

### Root Cause

When the user changes the library configuration (opens a different library), `AppState::update_config()` replaces the `Arc<Library>` in the shared state. However, translation queue worker tasks spawned earlier continue to hold an `Arc` reference to the **old** library. Any translations they produce will be saved to the old library's disk directory, not the new one.

The translation queue spawns long-lived tokio tasks that capture `library.clone()` at spawn time. There is no mechanism to cancel these tasks or notify them that the library has changed.

### Affected Code

- `site/src-tauri/src/app.rs:152-153`: Library replacement — `*self.library.write().await = Some(library.clone())` creates new library without cancelling existing queue tasks
- `site/src-tauri/src/app/translation_queue.rs:103-108`: Worker task captures `library.clone()` at spawn time; the closure runs indefinitely with the captured ref
- `site/src-tauri/src/app/translation_queue.rs:227-237`: `handle_request` uses the captured library ref for all book operations

### Recommendation

Add a cancellation mechanism: when `update_config()` replaces the library, it should either:
1. Send a cancellation token to all translation queue workers, or
2. Have workers re-read the library from `AppState` before each request (instead of using the captured clone)

---

## Bug 2: Stale Snapshot Overwrites (F2)

- **Bug Family**: F2 — Event Ordering / Stale Snapshots
- **Severity**: High
- **Invariant violated**: MCEventMonotonicity
- **Config**: MC.cfg (found during convergence, confirmed with MC_hunt_f2.cfg)
- **Counterexample**: 8 states (convergence), output file `output/MC_r1_bfs_2.out`; also confirmed in `output/MC_hunt_f2_sim.out`

### Trace Summary (convergence counterexample, 8 states)

1. **State 2**: `BeginTauri(t1, b1)` — tauri command starts
2. **State 3**: `TauriModify(t1)` — writes version 2, `truthVersion=2`
3. **State 4**: `TauriComputeSnapshot(t1)` — captures snapshot=2
4. **State 5**: `ConfigChange` — `truthVersion` advances to 3, emits fresh version 3 to event queue
5. **State 6**: `TauriEmit(t1)` — emits stale snapshot=2 to event queue (now queue has `<<3, 2>>`)
6. **State 7**: `DeliverEvent` — UI receives version 3, `uiVersion=3` ✓
7. **State 8**: `DeliverEvent` — UI receives version 2, `uiVersion=2` ✗ **REGRESSES**

The invariant fires because `uiVersion=2 < maxDeliveredVersion=3`.

### Root Cause

The frontend's `eventToReadable` in `tauri.ts` does `setter(event.payload)` — it blindly applies every event payload without any version or timestamp check. When two events arrive out of logical order (which happens when a slow task emits a snapshot computed before a fast concurrent operation), the UI regresses to stale state.

This is fundamental to the FIFO event delivery model: events are delivered in **emit order**, but emit order doesn't match **logical version order** when tasks have different latencies.

### Affected Code

- `site/src/lib/data/tauri.ts:10-13`: `listen<T>(eventName, (event) => { setter(event.payload) })` — no version check
- `site/src-tauri/src/app/translation_queue.rs`: Emits `book_updated` and `library_updated` without version metadata (previously emitted `paragraph_updated` which was removed)
- `site/src-tauri/src/app/library_view.rs`: All emit sites lack version info in payload

### Recommendation

Add a monotonic version counter to all event payloads. The frontend listener should compare the incoming version against the last-applied version and discard stale events:
```typescript
listen<T & {version: number}>(eventName, (event) => {
    if (event.payload.version > lastVersion) {
        lastVersion = event.payload.version;
        setter(event.payload);
    }
});
```

---

## Bug 3: No Shutdown Persistence (F3)

- **Bug Family**: F3 — Data Loss on App Close
- **Severity**: Critical
- **Invariant violated**: MCNoPersistenceLoss
- **Config**: MC_hunt_f3.cfg (BFS, exhaustive)
- **Counterexample**: 4 states, output file `output/MC_hunt_f3_bfs.out`

### Trace Summary

1. **State 1**: Initial — `diskVersion[b1]=0`, `memVersion[b1]=0`
2. **State 2**: `BeginWorker(t1, b1)` — worker starts processing book b1
3. **State 3**: `WorkerReadParagraph(t1)` — reads book into memory, `memVersion[b1]=1`
4. **State 4**: `AppClose` — app terminates, `appAlive=FALSE`. `memVersion[b1]=1` but `diskVersion[b1]=0` — **data lost**

The invariant fires because `memVersion[b1]=1 > diskVersion[b1]=0` when `appAlive=FALSE`.

### Root Cause

The Tauri app has **no shutdown handler**. The `Builder` in `lib.rs` sets up plugins, commands, and a `setup` hook, but never registers an `on_window_event` handler to flush in-memory state on close. When the user closes the window (or the OS kills the process), any in-memory modifications that haven't been explicitly saved are lost.

This is particularly dangerous for translation work: if a user has been translating for a while and the app crashes or they close it before an auto-save, all unsaved translation progress is gone.

### Affected Code

- `site/src-tauri/src/lib.rs:13-74`: Builder setup has no `.on_window_event()` or equivalent shutdown hook
- `library/src/library/library_book.rs`: `LibraryBook` holds in-memory state with `changed` flag but no periodic flush

### Recommendation

Add a shutdown handler that iterates all books and saves any with `changed=true`:
```rust
.on_window_event(|window, event| {
    if let tauri::WindowEvent::CloseRequested { .. } = event {
        let state = window.state::<AppState>();
        tauri::async_runtime::block_on(state.flush_all());
    }
})
```

---

## Bug 4: Translation Lifecycle Atomicity (F4)

- **Bug Family**: F4 — Stale Translation Store
- **Severity**: Medium
- **Invariant violated**: MCNoStaleTranslation
- **Config**: MC_hunt_f4.cfg (simulation)
- **Counterexample**: 31 states, output file `output/MC_hunt_f4_sim.out`

### Trace Summary (key states)

1. **State 2-3**: Watcher (t2) and Tauri (t1) both start on book b1
2. **State 4**: `TauriModify(t1)` — modifies book, `diskVersion=1`, `bookVersion=1`
3. **States 5-16**: Multiple `MarkWordVisible` calls advance `diskVersion` to 10
4. **State 7**: `WatcherReload(t2)` — watcher reloads book from disk, `bookVersion` advances to 2
5. **States 17-24**: Watcher emits, more changes happen, `bookVersion` reaches 3
6. **State 25-26**: `BeginWorker(t1, b1)` → `WorkerReadParagraph(t1)` — worker reads at `bookVersion=3` (`taskReadVersion[t1]=3`)
7. **States 27-29**: Watcher reloads again, `bookVersion` advances to 4
8. **State 31**: `WorkerCallAPI(t1)` — worker is about to store result, but `taskReadVersion[t1]=3 ≠ bookVersion[b1]=4`

The invariant fires because the worker read paragraph content at version 3, but by the time it's ready to store the translation result, the book has been reloaded to version 4 (paragraph content may have changed).

### Root Cause

There is no version check between the paragraph read and translation store in the translation queue's `handle_request` flow. The worker reads a paragraph snapshot at line 231, makes an API call (which takes significant time), then stores the result at line 333. During that API call, a file watcher event can trigger a book reload, changing the paragraph content. The stored translation may then correspond to text that no longer exists in the book.

### Affected Code

- `site/src-tauri/src/app/translation_queue.rs:227-237`: Paragraph read — captures text snapshot without recording version
- `site/src-tauri/src/app/translation_queue.rs:330-334`: Translation store — `add_paragraph_translation()` with no version validation
- `library/src/library/library_book.rs:157-175`: `add_paragraph_translation()` accepts write unconditionally, no conflict detection

### Recommendation

Add optimistic concurrency control: record the book version at read time, and check it at store time:
```rust
// At read time (line 231):
let read_version = book.version();

// At store time (line 330):
let book = library.get_book(&request.book_id).await?;
let book = book.lock().await;
if book.version() != read_version {
    // Book changed — discard stale translation, re-queue the request
    continue;
}
```

---

## Not Reproduced

All 4 bug families were reproduced. No untestable families.

| Bug Family | Config | Mode | States Explored | Result |
|------------|--------|------|-----------------|--------|
| F1 | MC_hunt_f1.cfg | Simulation | 295 | **Violated** (4 states) |
| F2 | MC_hunt_f2.cfg | Simulation | 2,156 | **Violated** (confirmed) |
| F2 | MC.cfg | BFS | 2,308,015,144 | **Violated** during convergence (8 states) |
| F3 | MC_hunt_f3.cfg | BFS | 388 | **Violated** (4 states, exhaustive at depth 10) |
| F4 | MC_hunt_f4.cfg | Simulation | 1,456 | **Violated** (31 states) |

## Structural Invariants

| Invariant | Config | States Explored | Depth | Result |
|-----------|--------|-----------------|-------|--------|
| PCConsistency | MC.cfg | 2,308,015,144 | 126 | ✅ No violation (stopped by user) |
| TaskLibraryValidity | MC.cfg | 2,308,015,144 | 126 | ✅ No violation (stopped by user) |

## Spec Fixes During Hunting

- **UIConsistency (Case A)**: Added `maxDeliveredVersion > 0` guard — invariant was too strong at init (UI starts empty, so `uiVersion=0 ≠ truthVersion=1` is expected). This is an invariant fix, not a spec fix.
- **MCEventMonotonicity / MCUIConsistency**: Moved to hunting-only configs after confirming F2 is a real system bug (not a spec error).
