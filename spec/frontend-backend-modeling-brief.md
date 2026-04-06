# Modeling Brief: FLTS Frontend–Backend Command/Event Protocol

## 1. System Overview

- **System**: FLTS — Svelte 5 frontend communicating with a Tauri/Rust backend via typed command invocations and event subscriptions. Background tasks (translator, saver, file watcher) produce events that update UI state.
- **Language**: TypeScript (Svelte 5) + Rust (Tokio async), ~1200 LOC frontend data layer + ~2000 LOC backend command/event handlers
- **Category**: **Category B (Concurrent / Lock-Free / Runtime)** — multiple async Tauri command handlers, background task actors (translator, saver, status updater, file watcher), and a frontend reactive store layer that processes events and command results without sequencing guarantees.
- **Concurrency model**: Backend: Tokio async runtime with per-request command handlers + long-lived background tasks sharing `Arc<Library>` via `Arc<RwLock<..>>`. Frontend: single-threaded JS event loop receiving async command results and event payloads that race with each other.
- **Key architectural choices**:
  - Frontend uses `eventToReadable` (event payload IS the store value, no re-query) for `library_updated` and `config_updated`
  - Frontend uses `getterToReadableWithEventsAndPatches` for paragraph views (patches applied in arrival order)
  - Backend emits `library_updated` from 7+ distinct code sites, each carrying a full book-list snapshot
  - Background tasks (translator worker, saver) capture `Arc<Library>` at init time; config changes create a new Library but don't cancel old tasks
  - No event sequencing, versioning, or cancellation tokens exist

## 2. Bug Families

### Family 1: Stale Library Reference After Config Reconfiguration (HIGH)

**Mechanism**: `update_config()` sets `translation_queue` to `None` and creates a new `Library` instance in `eval_config()`. Background tasks (translator worker, saver, status updater) spawned by the old `TranslationQueue` continue running with `Arc<Library>` references to the OLD Library. These tasks save translations to the old Library's disk path and emit events with stale data. No cancellation mechanism exists.

**Evidence**:
- Code analysis: `app.rs:112` — `*self.translation_queue.write().await = None;` drops queue reference but spawned tasks keep running
- Code analysis: `app.rs:152-153` — `Arc::new(Library::open(...))` creates new Library, old one lives via background Arc refs
- Code analysis: `translation_queue.rs:94-99` — `tokio::spawn(run_saver(library.clone(), ...))` captures old Library at init
- Code analysis: `translation_queue.rs:106-146` — translator worker also captures `library.clone()`
- Code analysis: `translation_queue.rs:439,459,469` — `save_and_emit()` uses captured (stale) Library for both save and event emission
- Code analysis: No `abort`, `cancel`, `JoinHandle`, or `CancellationToken` found in `site/src-tauri/src/`
- Historical: `ea80c0c` "Fix deadlock at config change" — prior fix addressed blocking but not stale-reference issue

**Affected code paths**:
- `AppState::update_config()` (app.rs:104-142)
- `AppState::eval_config()` (app.rs:144-173)
- `run_saver()` (translation_queue.rs:348-403)
- `save_and_emit()` → `emit_updates()` (translation_queue.rs:434-472)
- `handle_request()` (translation_queue.rs:216-346)

**Suggested modeling approach**:
- Variables: `currentLibrary` (the Library in AppState), `taskLibrary[t]` (Library captured by each background task), `libraryPath[lib]` (disk path per Library instance)
- Actions: `ConfigChange` (creates new Library, old tasks keep old ref), `TranslateComplete(t)` (saves via taskLibrary[t]), `SaveAndEmit(t)` (emits event via taskLibrary[t])
- Granularity: ConfigChange is one atomic action; translate/save/emit are separate steps
- Key: model that `taskLibrary[t] /= currentLibrary` can be true after ConfigChange

**Priority**: High
**Rationale**: Verified race condition. Translation data can be saved to the wrong disk path (data loss when library path changes). Stale events corrupt UI state. No existing mitigation. Config changes are infrequent but the window is unbounded (old tasks run until channel drains).

---

### Family 2: Event Ordering / Stale Snapshot Overwrites (HIGH)

**Mechanism**: Multiple concurrent operations emit `library_updated` events, each carrying a full book-list snapshot computed at different points in time. The frontend's `eventToReadable` sets the event payload directly as the store value (no re-query, no versioning). The last event to arrive wins, regardless of when it was generated. Two concurrent operations can produce snapshots where each reflects only its own changes, and the older snapshot overwrites the newer one.

**Evidence**:
- Code analysis: `tauri.ts:10-12` — `setter(event.payload)` directly sets store value from event payload
- Code analysis: 7+ emit sites for `library_updated`: `app.rs:166,208,228`, `library_view.rs:283,298,340,351`, `translation_queue.rs`
- Code analysis: `paragraph_updated` event removed — paragraphs now update via `book_updated` re-fetch
- Code analysis: No `sequence`, `version`, `counter`, or `seqNo` fields found in event payloads or stores
- Code analysis: `library_view.rs:137-165` — `list_books()` reads from disk (not cache), so snapshot reflects disk state at query time

**Affected code paths**:
- All `app.emit("library_updated", books)?` call sites (7+ locations)
- `eventToReadable()` (tauri.ts:7-42)
- `getterToReadableWithEventsAndPatches()` for paragraph patches (tauri.ts:136-202)
- Frontend `booksStore` (library.ts:135)

**Suggested modeling approach**:
- Variables: `uiBookList` (current frontend store), `pendingEvents` (in-flight events from backend), `eventSeq` (logical timestamp per event)
- Actions: `EmitLibraryUpdated(snapshot, seq)` (backend emits event), `DeliverEvent(ev)` (frontend receives and sets store — nondeterministic order)
- Invariant: `UIConsistency` — after all pending events delivered, UI reflects the latest operation's state
- Granularity: each emit and each delivery is a separate action; delivery order is nondeterministic

**Priority**: High
**Rationale**: Affects core UI correctness. User sees stale book list after concurrent operations (delete + translate, import + file watcher, etc.). No sequencing mechanism exists. Every pair of concurrent operations that emit `library_updated` creates a race window.

---

### Family 3: Unsaved In-Memory Modifications / No Shutdown Persistence (MEDIUM)

**Mechanism**: `mark_word_visible` DOES call `save()` immediately after marking (verified). However, `get_or_create_translation` creates translation state in memory without saving; persistence depends on a later trigger (translation completion, file watcher). More critically, **no shutdown handler exists** — if the app terminates abnormally between any dirty-state modification and its save trigger, changes are lost. Additionally, during file watcher reloads, `save()` triggers a merge with on-disk state via `translation.merge()`; the merge's preservation of `visible_words` depends on the `Translation::merge()` implementation which unions visible_words sets (verified safe). The `changed` flag is never reset to `false` after save, which inadvertently ensures dirty state survives across multiple save cycles.

**Evidence**:
- Code analysis: `library_view.rs:355-377` — `mark_word_visible` command DOES call `book.save().await?` immediately after marking (verified)
- Code analysis: `library_book.rs:187-193` — `mark_word_visible` sets `self.changed = true` in the translation object
- Code analysis: `library_book.rs:381-420` — `get_or_create_translation` creates translation in memory, not saved until explicit save trigger
- Code analysis: `library_book.rs:521-544` — `reload_translations()` triggers `save()` if disk is newer, which merges disk state with memory
- Code analysis: `library_book.rs:581-602` — save's merge path: if disk file is newer, loads and merges via `translation.merge()`
- Code analysis: `translation.rs:399-479` — `Translation::merge()` unions `visible_words` sets — marked words ARE preserved through merges
- Code analysis: `lib.rs:1-100` — NO `on_exit()`, `close_requested`, or shutdown handler found in Tauri setup
- Code analysis: `changed` flag is set at `library_book.rs:90,173,190` but never reset to `false` after save
- Historical: `2e1db60` / `44c28d7` — book save overwrite semantics debated; disk-wins was chosen by design

**Affected code paths**:
- `mark_word_visible` → `LibraryBook::save()` (library_view.rs:355-377) — saves immediately ✓
- `get_or_create_translation` (library_book.rs:381-420) — creates in memory, no immediate save
- `reload_translations` (library_book.rs:521-544) → `save()` → merge with disk
- App shutdown — no flush mechanism (lib.rs)

**Suggested modeling approach**:
- Variables: `memDirty[book]` (whether in-memory state has unsaved changes), `diskVersion[book]` (logical timestamp of on-disk file), `appAlive` (boolean)
- Actions: `MarkWordVisible(b)` (sets memDirty, triggers SaveBook), `CreateTranslation(b)` (sets memDirty, no save), `SaveBook(b)` (persists, updates diskVersion — note: does NOT clear memDirty due to never-reset bug), `AppClose` (sets appAlive=false, check memDirty), `FileWatcherReload(b)` (triggers merge+save if disk newer)
- Invariant: `NoPersistenceLoss` — all memDirty state is eventually persisted before AppClose

**Priority**: Medium
**Rationale**: The immediate `mark_word_visible` persistence gap is closed (it calls save). The remaining risks are: (1) `get_or_create_translation` not saving immediately — but this is always followed by a translation that triggers save via saver task; (2) **no shutdown persistence** — abnormal termination loses all dirty state; (3) the `changed` flag never resetting is benign (causes redundant saves) but indicates incomplete state management.

---

### Family 4: Translation Lifecycle Cross-Component State (MEDIUM)

**Mechanism**: The full translation lifecycle spans multiple components and lock acquisitions: (1) queue dedup check, (2) book lock to get paragraph text, (3) external API call (no locks), (4) translation lock to store result, (5) saver lock to write to disk, (6) event emission. Between steps 2 and 5, the book can be modified by file watcher events, other Tauri commands, or config changes. The translation stores a paragraph index that could become stale if the book is reloaded with different content.

**Evidence**:
- Code analysis: `translation_queue.rs:227-237` — handler acquires book lock, reads paragraph text, drops book lock
- Code analysis: `translation_queue.rs:330-334` — acquires translation lock, adds paragraph translation by index
- Code analysis: `translation_queue.rs:428-431` — saver acquires book lock again to save
- Code analysis: `library_book.rs:512-519` — `reload_book()` can reload book content between steps 2 and 5
- Code analysis: `library.rs:309-316` — `handle_file_change_event` → `reload_book` can fire asynchronously

**Affected code paths**:
- `handle_request()` (translation_queue.rs:216-346) — steps 1-4
- `save_book()` (translation_queue.rs:428-432) — step 5
- `emit_updates()` (translation_queue.rs:446-472) — step 6
- `LibraryBook::reload_book()` (library_book.rs:512-519) — concurrent modifier

**Suggested modeling approach**:
- Variables: `bookVersion[b]` (incremented on reload), `translationTargetVersion[t]` (version when paragraph was read), `currentBookVersion[b]`
- Actions: `ReadParagraph(t, b)` (captures bookVersion), `StoreTranslation(t, b)` (stores result), `ReloadBook(b)` (increments version)
- Invariant: `NoStaleTranslation` — translation is not stored if `translationTargetVersion /= currentBookVersion`

**Priority**: Medium
**Rationale**: The paragraph index is an integer position. If the book is reloaded with different content (e.g., sync conflict resolution replaces the book), the index could point to a different paragraph. However, this requires a specific sequence: external modification + file watcher reload + in-flight translation, which is uncommon. The translation result itself is idempotent (same paragraph text → same translation).

## 3. Modeling Recommendations

### 3.1 Model (with rationale)

| What | Why | How |
|------|-----|-----|
| Config change with in-flight tasks | Family 1: verified race, data loss possible | `ConfigChange` action creates new Library; tasks retain old ref; check `StaleLibrarySafety` |
| Event emission from multiple sites | Family 2: no sequencing, last-write-wins on UI | Nondeterministic event delivery order; check `UIConsistency` after all deliveries |
| Background task lifecycle | Family 1: tasks outlive queue drop | Model task spawning at init and graceful/ungraceful termination |
| Translation lifecycle steps | Family 4: book can change between read and write | Multi-step action with interleaving book reloads |
| In-memory dirty tracking | Family 3: unsaved modifications | `memDirty` flag with `SaveBook` action clearing it |

### 3.2 Do Not Model (with rationale)

| What | Why |
|------|-----|
| Lock ordering / deadlock | Already covered in existing `spec/mutex/` spec with exhaustive model checking |
| File merge semantics (book.dat conflicts) | Already covered in existing `spec/base.tla` (file sync spec) |
| Translation API internals | External service call; no lock interaction; idempotent |
| Platform-specific commands (macOS dictionary, iOS) | No shared state, no concurrency concerns |
| Translation progress polling (500ms interval) | Read-only status queries; no state modification; trivially safe |
| RwLock read-write upgrade windows | Covered in mutex spec (Family 4); double-check pattern is correct |
| Individual lock acquisition/release | Covered in mutex spec; this spec operates at command/event granularity |

## 4. Proposed Extensions

| Extension | Variables | Purpose | Bug Family |
|-----------|-----------|---------|------------|
| Library lifecycle | `currentLibrary`, `taskLibrary[t]`, `libraryPath[lib]` | Track stale Library references after config change | Family 1 |
| Event delivery model | `uiState`, `pendingEvents`, `eventSeq` | Model nondeterministic event delivery order | Family 2 |
| Task lifecycle | `taskAlive[t]`, `taskQueue[t]` | Model background task spawning and termination | Family 1 |
| Dirty state tracking | `memDirty[b]`, `diskVersion[b]` | Track unsaved in-memory modifications | Family 3 |
| Translation pipeline | `pipelineStage[t]`, `capturedVersion[t]` | Track multi-step translation with interleaving | Family 4 |

## 5. Proposed Invariants

| Invariant | Type | Description | Targets |
|-----------|------|-------------|---------|
| `StaleLibrarySafety` | Safety | No background task emits events using a Library instance different from the current AppState Library | Family 1 |
| `NoDataLoss` | Safety | Translations are never saved to a Library whose path differs from the current configured path | Family 1 |
| `UIConsistency` | Safety | After all pending events are delivered, the UI book list matches the result of a fresh `list_books` query | Family 2 |
| `EventMonotonicity` | Safety | The UI never displays a book list that is older than a previously displayed one (requires sequencing) | Family 2 |
| `ParagraphPatchConsistency` | Safety | Paragraph patches applied to the UI always reflect the latest version of each paragraph | Family 2 |
| `NoPersistenceLoss` | Liveness | Every in-memory modification (`memDirty = true`) is eventually persisted to disk | Family 3 |
| `NoStaleTranslation` | Safety | A translation is not stored against a book version different from the one its source paragraph was read from | Family 4 |
| `TaskTermination` | Liveness | After config change, all old background tasks eventually terminate | Family 1 |

## 6. Findings Pending Verification

### 6.1 Model-Checkable

| ID | Description | Expected invariant violation | Bug Family |
|----|-------------|----------------------------|------------|
| MC-1 | Config change while translation in-flight: old saver emits stale `library_updated` | `StaleLibrarySafety` violation — task emits via old Library | Family 1 |
| MC-2 | Config change with library path change: translation saved to wrong path | `NoDataLoss` violation — save goes to old path | Family 1 |
| MC-3 | Two concurrent `library_updated` events delivered out of order | `UIConsistency` violation — UI shows stale snapshot | Family 2 |
| MC-4 | Paragraph patch from old translation overwrites newer translation | `ParagraphPatchConsistency` violation | Family 2 |
| MC-5 | Book reloaded between paragraph read and translation store | `NoStaleTranslation` violation — index mismatch | Family 4 |
| MC-6 | Old background tasks never terminate after config change | `TaskTermination` liveness violation | Family 1 |

### 6.2 Test-Verifiable

| ID | Description | Suggested test approach |
|----|-------------|----------------------|
| TV-1 | Config change during active translation: verify translation appears in new library | Start translation, change config mid-flight, check new library has the translation |
| TV-2 | Concurrent `library_updated` events: verify UI shows latest state | Trigger import + delete concurrently, check final UI matches disk state |
| TV-3 | `mark_word_visible` persistence: verify marks survive app restart | Mark words, close app without triggering save, reopen, check marks |
| TV-4 | Rapid config changes: verify no orphaned background tasks | Change config 5 times rapidly, check task count doesn't grow |

### 6.3 Code-Review-Only

| ID | Description | Suggested action |
|----|-------------|-----------------|
| CR-1 | No cancellation mechanism for background tasks | Add `CancellationToken` to TranslationQueue; cancel old tasks on config change |
| CR-2 | No event sequencing in `library_updated` | Add monotonic sequence number to event payload; frontend ignores older events |
| CR-3 | `eventToReadable` has no protection against stale payloads | Either add version check in setter, or switch to re-query pattern (`getterToReadableWithEvents`) |
| CR-4 | Saver task captures Library at init, never refreshes | Read current Library from AppState on each save instead of using captured ref |

## 7. Reference Pointers

- **Key source files**:
  - `site/src-tauri/src/app.rs:104-173` — `update_config()` and `eval_config()` (config change flow)
  - `site/src-tauri/src/app/translation_queue.rs:74-153` — TranslationQueue::init (task spawning)
  - `site/src-tauri/src/app/translation_queue.rs:348-472` — run_saver, save_and_emit, emit_updates
  - `site/src-tauri/src/lib.rs:48-71` — Event loop startup, watcher integration
  - `site/src/lib/data/tauri.ts:7-202` — eventToReadable, getterToReadableWithEventsAndPatches
  - `site/src/lib/data/library.ts` — Frontend store bindings and command wrappers
  - `library/src/library/library_book.rs:381-420,512-545` — get_or_create_translation, reload_book/translations
- **Existing specs** (do not duplicate):
  - `spec/base.tla` — File sync / persistence model (Category A)
  - `spec/mutex/base.tla` — Lock hierarchy / deadlock freedom (Category B)
  - `spec/mutex-modeling-brief.md` — Mutex lock safety analysis
- **Historical bug fixes**:
  - `ea80c0c` — "Fix deadlock at config change" (partial fix, addressed blocking but not stale reference)
  - `d15e3aa` — "Replace global App mutex with internal locks" (architectural improvement)
  - `44300d8` — "Fix TOCTOU race in TranslationQueue::translate()" (dedup now atomic)
- **Frontend architecture**:
  - `eventToReadable`: event payload IS the store value (no re-query)
  - `getterToReadableWithEvents`: events trigger re-query (safer)
  - `getterToReadableWithEventsAndPatches`: hybrid — refresh events + in-place patches
