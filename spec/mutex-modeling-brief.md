# Modeling Brief: FLTS Mutex / Lock Safety

## 1. System Overview

- **System**: FLTS — Rust workspace with library crate, Tauri desktop backend, and CLI
- **Language**: Rust, ~5100 LOC core logic across 8 files
- **Category**: **Category B (Concurrent / Lock-Free / Runtime)** — the interesting correctness boundaries are tokio::sync::Mutex and RwLock acquisition patterns across concurrent Tauri commands, file watcher events, and background translation/saver tasks.
- **Concurrency model**: Tokio async runtime. Multiple concurrent Tauri command handlers (spawned per UI request), one file watcher event loop, one translation queue processor, one saver task, and one status updater task. All share state through `Arc<Mutex<..>>` and `Arc<RwLock<..>>` wrappers.
- **Key architectural choice**: Evolved from a single `Arc<Mutex<App>>` to fine-grained per-field locks (commit `d15e3aa`). Library itself is lock-free at the top level (`Arc<Library>`), with internal `RwLock<HashMap<Uuid, Arc<Mutex<LibraryBook>>>>` for the book cache.

## 2. Lock Inventory

| ID | Lock | Type | Location | Protects |
|----|------|------|----------|----------|
| L1 | `AppState.config` | `RwLock<Config>` | app.rs:65 | App configuration |
| L2 | `AppState.library` | `RwLock<Option<Arc<Library>>>` | app.rs:66 | Library reference |
| L3 | `AppState.translation_queue` | `RwLock<Option<Arc<TranslationQueue>>>` | app.rs:67 | Translation queue reference |
| L4 | `AppState.watcher` | `Arc<Mutex<LibraryWatcher>>` | app.rs:68 | File watcher |
| L5 | `Library.books_cache` | `RwLock<HashMap<Uuid, Arc<Mutex<LibraryBook>>>>` | library.rs:201 | Book cache |
| L6 | Per-book | `Arc<Mutex<LibraryBook>>` | library.rs:201 (values) | Individual book state |
| L7 | Per-translation | `Arc<Mutex<LibraryTranslation>>` | library_book.rs:57 | Individual translation state |
| L8 | `DictionaryCache.cache` | `RwLock<HashMap<..., Arc<Mutex<LibraryDictionary>>>>` | library_dictionary.rs:234 | Dictionary cache |
| L9 | Per-dictionary | `Arc<Mutex<LibraryDictionary>>` | library_dictionary.rs:234 (values) | Individual dictionary state |
| L10 | `TranslationQueue.state` | `Arc<Mutex<TranslationQueueState>>` | translation_queue.rs:63 | Queue request/status maps |
| L11 | `run_saver.savers` | `Arc<Mutex<HashMap>>` | translation_queue.rs:344 | Saver task dedup map |
| L12 | `emit_state` | `Arc<std::sync::Mutex<EmitState>>` | translation_queue.rs:274 | Progress callback throttle (std sync, not tokio) |

## 3. Bug Families

### Family 1: Lock Starvation via Long-Held Book Mutex (MEDIUM)

**Mechanism**: The `LibraryBook` Mutex (L6) is held during the entire `save()` operation, which performs multiple disk I/O operations (write each translation file, write each dictionary, write book.dat), each with a compare-and-swap retry loop. Concurrent Tauri commands, saver tasks, and file watcher events all contend on this single per-book Mutex.

**Evidence**:
- Historical: `68dc022` "Fix deadlocks" — changed bounded channels to unbounded and added bounded retry to avoid re-queuing (which caused workers to hold locks while waiting on full channels).
- Historical: `d15e3aa` "Replace global App mutex with internal locks" — broke coarse-grained `Arc<Mutex<App>>` into per-field locks to reduce contention.
- Code analysis: `library_book.rs:522-734` — `save()` holds book Mutex, iterates all translations (each with its own Mutex acquisition + disk I/O), then saves book.dat with retry loop.
- Code analysis: `library_view.rs:184-218` — `list_book_chapter_paragraphs` holds book Mutex while locking each translation per paragraph (O(paragraphs) lock acquisitions while book is locked).

**Affected code paths**:
- `LibraryBook::save()` (library_book.rs:522-734)
- `list_book_chapter_paragraphs` (library_view.rs:184-218)
- `handle_file_change_event` → `reload_book`/`reload_translations` → `save()` (library.rs:302-340)
- `save_book` (translation_queue.rs:418-422)

**Suggested modeling approach**:
- Variables: per-process program counter, `bookLockHolder`, `translationLockHolder[t]`, `dictLockHolder[d]`
- Actions: model lock acquisition as separate atomic steps; model `save()` as a multi-step action holding L6 throughout
- Granularity: split into AcquireBookLock, SaveTranslation (per translation), SaveBook, ReleaseBookLock

**Priority**: Medium
**Rationale**: No deadlock, but extended lock holding causes UI responsiveness issues. The pattern is consistent and intentional — reducing scope would require architectural change. TLA+ can quantify maximum contention depth.

---

### Family 2: TOCTOU in Translation Queue Request Deduplication (LOW)

**Mechanism**: The `translate()` function checks for an existing request ID, then sends a new request, then inserts the new request ID — all with separate lock acquisitions on `TranslationQueue.state` (L10). Two concurrent calls for the same paragraph can both pass the check and both send translation requests.

**Evidence**:
- Code analysis: `translation_queue.rs:155-184` — `get_request_id()` locks state, returns None, drops lock; `fetch_add` increments atomically; `send_async` enqueues; separate `lock().await` inserts mapping. Window between check and insert.
- Code analysis: `translation_queue.rs:139-143` — worker removes paragraph from map after completing, creating another window where a duplicate request could be submitted.

**Affected code paths**:
- `TranslationQueue::translate()` (translation_queue.rs:155-184)
- Worker completion handler (translation_queue.rs:139-143)

**Suggested modeling approach**:
- Variables: `requestMap`, `pendingRequests`, `workerState`
- Actions: `CheckExisting`, `SendRequest`, `InsertMapping`, `WorkerComplete`, `WorkerRemoveMapping`
- Granularity: each step is a separate action to expose the interleaving window

**Priority**: Low
**Rationale**: Duplicate translation requests waste API calls but don't corrupt data. The translation itself is idempotent. Impact is cost/performance, not correctness.

---

### Family 3: Fragile Double-Lock Pattern in get_or_create_translation (LOW)

**Mechanism**: `get_or_create_translation` acquires the same translation Mutex twice in a single `if &&` condition. Currently safe due to async temporary drop semantics (MutexGuard dropped before second `.await`), but fragile — any refactoring that binds the guard to a name or changes the expression structure would deadlock.

**Evidence**:
- Code analysis: `library_book.rs:375-377` — `t.lock().await.translation.source_language == ...  && t.lock().await.translation.target_language == ...` acquires same Mutex twice.
- Empirical: tested with edition 2024 AND 2021; no deadlock due to async desugaring dropping guard before second await point.
- Code analysis: The two separate lock acquisitions create a TOCTOU window (translation fields could theoretically change between checks, though practically impossible since book Mutex is held).

**Affected code paths**:
- `LibraryBook::get_or_create_translation()` (library_book.rs:369-396)
- Called from: `handle_request` (translation_queue.rs:220), `list_book_chapter_paragraphs` (library_view.rs:193), `get_paragraph_view` (library_view.rs:114), `get_word_info` (library_view.rs:232), `mark_word_visible` (library_view.rs:364)

**Suggested modeling approach**:
- Variables: `translationLocked[t]`, `bookLocked`, `pcState` (program counter for the function)
- Actions: model the two lock acquisitions as separate steps; inject a "refactoring scenario" where the guard is held across both
- Invariant: NoSelfDeadlock — no process holds a Mutex and awaits the same Mutex

**Priority**: Low
**Rationale**: Currently safe. The TOCTOU has no practical impact because the book Mutex prevents concurrent modification. However, the pattern is a maintenance hazard.

---

### Family 4: RwLock Read-Write Upgrade Windows (LOW)

**Mechanism**: Several cache lookup patterns follow read-then-write: acquire RwLock.read(), check if entry exists, drop read, create entry, acquire RwLock.write(), double-check, insert. The window between read-drop and write-acquire allows duplicate creation.

**Evidence**:
- Code analysis: `library.rs:243-260` — `get_book()` reads cache, drops read guard, loads from disk, acquires write guard, double-checks. Two concurrent calls for the same uncached book both load from disk; only one insert wins, but both do disk I/O.
- Code analysis: `library_dictionary.rs:304-332` — `get_dictionary()` same pattern.
- Code analysis: `app.rs:248-268` — `get_or_init_translation_queue()` same pattern with `RwLock<Option<Arc<TranslationQueue>>>`.

**Affected code paths**:
- `Library::get_book()` (library.rs:243-260)
- `DictionaryCache::get_dictionary()` (library_dictionary.rs:304-332)
- `AppState::get_or_init_translation_queue()` (app.rs:248-268)

**Suggested modeling approach**:
- Variables: `cacheState`, `loadingSet` (processes currently loading)
- Actions: `CacheReadMiss`, `LoadFromDisk`, `CacheWriteInsert`, `CacheWriteExistingFound`

**Priority**: Low
**Rationale**: All three implementations have correct double-check after acquiring write lock. Duplicate disk loads waste I/O but don't corrupt state. This is a standard and well-understood pattern.

## 4. Lock Order Analysis

The codebase follows a consistent lock acquisition order with no inversions:

```
Level 0: AppState.{config, library, translation_queue} (RwLock, independent)
Level 1: AppState.watcher (Mutex)
Level 2: Library.books_cache (RwLock)
Level 3: LibraryBook (Mutex)
Level 4: LibraryTranslation (Mutex)
Level 5: DictionaryCache.cache (RwLock)
Level 6: LibraryDictionary (Mutex)

TranslationQueue.state (Mutex) — independent, never nested with L2-L9
run_saver.savers (Mutex) — local to saver task, never nested with L2-L9
```

**Verification paths** (all follow the order above):
- File watcher: L2.read → L3 → L4 → L5 → L6
- Translation request: L2.read → L3 (then drop) → L4 → L5 → L6
- Saver: L2.read → L3 → L4 → L5 → L6
- Tauri commands: L0.read → L2.read → L3 → L4 → L5 → L6

**No lock order inversion was found.**

## 5. Historical Deadlocks (Resolved)

| Commit | Issue | Resolution |
|--------|-------|------------|
| `68dc022` | Workers re-queued failed paragraphs while holding shared queue lock; bounded channel caused backpressure deadlock | Changed to bounded retry within worker, unbounded channels |
| `ea80c0c` | `blocking_lock()` on tokio Mutex in async context; watcher held locked during `recv().await` | Switched to `lock().await`; extracted `flume::Receiver` to avoid holding watcher lock during event wait |
| `d15e3aa` | Single `Arc<Mutex<App>>` caused all Tauri commands to serialize | Split into per-field `RwLock`/`Mutex` |
| `2efdeb7` | `Arc<Mutex<Library>>` serialized all book operations | Moved to internal `RwLock<HashMap>` with per-book `Mutex` |
| `ae2001d` | TranslationQueue used `Mutex` for rarely-written state | Changed to `RwLock` with double-check init pattern |

## 6. Modeling Recommendations

### 6.1 Model

| What | Why | How |
|------|-----|-----|
| Lock hierarchy (L2→L3→L4→L5→L6) | Core claim: no ABBA deadlock exists | Model each lock as a variable; actions acquire/release in order; verify NoDeadlock invariant |
| Concurrent task types | File watcher, Tauri commands, translator, saver all contend | Model as separate processes with distinct lock acquisition sequences |
| Book save() multi-step locking | Family 1: long lock hold with nested sub-locks | Split save into steps; measure max contention depth |
| Translation queue TOCTOU | Family 2: race between check and insert | Model as separate actions; check DuplicateRequest invariant |

### 6.2 Do Not Model

| What | Why |
|------|-----|
| Disk I/O timing and retry loops | The CAS retry in save() affects latency but not lock safety; modeling I/O timing would explode state space |
| Translation content / merge semantics | Already covered in existing modeling-brief.md (file sync spec) |
| UI event emission | Fire-and-forget via Tauri Emitter; no lock interaction |
| TranslationQueue progress callback (L12) | Uses `std::sync::Mutex` (not tokio); synchronous, no await while held; trivially safe |
| RwLock read-write upgrade windows (Family 4) | All implementations use correct double-check; duplicate loads are benign |

## 7. Proposed Extensions

| Extension | Variables | Purpose | Bug Family |
|-----------|-----------|---------|------------|
| Lock hierarchy model | `lockHolder[lock][process]`, `pc[process]` | Verify no ABBA ordering violation exists under all interleavings | All |
| Concurrent task model | `taskType[process] ∈ {Watcher, TauriCmd, Translator, Saver}` | Model different lock acquisition sequences per task type | Family 1 |
| Save contention depth | `waitingOn[process]`, `holdingLocks[process]` | Quantify maximum number of processes blocked on a single book | Family 1 |
| Translation request dedup | `requestMap`, `requestInFlight` | Model TOCTOU window in translate() | Family 2 |

## 8. Proposed Invariants

| Invariant | Type | Description | Targets |
|-----------|------|-------------|---------|
| `NoDeadlock` | Safety | No state exists where every process is waiting for a lock held by another process in the wait set | All families |
| `LockOrderConsistency` | Safety | No process holds lock at level N while acquiring lock at level < N | All families |
| `NoSelfDeadlock` | Safety | No process awaits a Mutex it already holds | Family 3 |
| `BoundedContention` | Liveness | Every lock acquisition eventually completes (no indefinite starvation) | Family 1 |
| `NoDuplicateActiveRequest` | Safety | At most one active translation request per (book_id, paragraph_id) at any time | Family 2 |

## 9. Findings Pending Verification

### 9.1 Model-Checkable

| ID | Description | Expected invariant violation | Bug Family |
|----|-------------|----------------------------|------------|
| M1 | Verify no ABBA deadlock exists across all 5 concurrent task types | `NoDeadlock` should hold | All |
| M2 | Quantify max processes blocked on single book Mutex during save | `BoundedContention` may reveal starvation under high load | Family 1 |
| M3 | Two concurrent translate() calls for same paragraph bypass dedup check | `NoDuplicateActiveRequest` should be violated | Family 2 |

### 9.2 Test-Verifiable

| ID | Description | Suggested test approach |
|----|-------------|----------------------|
| T1 | `get_or_create_translation` double-lock safety under refactoring | Add a test that binds guard to a named variable, verify it deadlocks (documents the fragility) |
| T2 | Concurrent `translate()` calls for same paragraph produce duplicate requests | Spawn 10 concurrent translate calls, verify at most 1 translation is persisted |
| T3 | `save()` under concurrent file watcher events doesn't corrupt data | Run save + watcher reload concurrently in a loop, verify book/translation integrity |

### 9.3 Code-Review-Only

| ID | Description | Suggested action |
|----|-------------|-----------------|
| C1 | `get_or_create_translation` double-lock pattern (library_book.rs:375-377) | Refactor to single lock: `let guard = t.lock().await; if guard.source == ... && guard.target == ... { drop(guard); return ... }` |
| C2 | `list_book_chapter_paragraphs` holds book lock while iterating all paragraphs with per-paragraph translation lock | Consider collecting paragraph data under book lock, then formatting outside the lock |
| C3 | `TranslationQueue::translate()` TOCTOU between check and insert | Consider atomic check-and-insert under single lock acquisition |

## 10. Reference Pointers

- **Key source files**:
  - `site/src-tauri/src/app.rs` — AppState with top-level locks (408 LOC)
  - `library/src/library.rs:199-260` — Library struct with books_cache RwLock
  - `library/src/library/library_book.rs:52-59, 369-396, 522-734` — LibraryBook Mutex, get_or_create_translation, save()
  - `library/src/library/library_dictionary.rs:232-354` — DictionaryCache with RwLock + per-dict Mutex
  - `site/src-tauri/src/app/translation_queue.rs:59-204, 338-416` — TranslationQueue state Mutex, saver task
  - `site/src-tauri/src/lib.rs:48-71` — Event loop startup
- **Historical deadlock fixes**: `68dc022`, `ea80c0c`, `d15e3aa`, `2efdeb7`, `ae2001d`
- **Existing spec**: `spec/modeling-brief.md` (file sync / persistence — Category A)
- **Repository memory notes**: "Never use blocking_lock() on tokio::sync::Mutex in async contexts" (from commit `68dc022`)
