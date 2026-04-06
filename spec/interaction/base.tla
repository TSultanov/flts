---- MODULE base ----
(***************************************************************************)
(* TLA+ specification of FLTS Frontend–Backend Command/Event Protocol.     *)
(*                                                                         *)
(* Models the interaction between background tasks (translator, saver),    *)
(* Tauri command handlers, file watcher, and frontend event delivery to    *)
(* verify data consistency across config changes and concurrent operations. *)
(*                                                                         *)
(* Source: site/src-tauri/src/app/ + site/src/lib/data/                     *)
(*   app.rs              — AppState, update_config, eval_config             *)
(*   translation_queue.rs — TranslationQueue, run_saver, emit_updates      *)
(*   library_view.rs     — import/delete/move + emit library_updated       *)
(*   tauri.ts            — eventToReadable (direct payload to store)        *)
(*                                                                         *)
(* Bug Families:                                                           *)
(*   F1 — Stale library reference after config reconfiguration  [FIXED]    *)
(*   F2 — Event ordering / stale snapshot overwrites            [FIXED]    *)
(*   F3 — Unsaved in-memory modifications / no shutdown persist [FIXED]    *)
(*   F4 — Translation lifecycle cross-component atomicity       [FIXED]    *)
(*                                                                         *)
(* F1 fix: TranslationQueue::Drop aborts spawned tasks via JoinHandle.     *)
(* F2 fix: library_updated and config_updated switched from eventToReadable *)
(*     (payload IS store value) to getterToReadableWithEvents (signal-only  *)
(*     event → frontend re-fetches via invoke). Backend no longer snapshots *)
(*     at emit time; UI always reads current backend state at delivery time.*)
(* F3 fix: RunEvent::Exit handler calls save_all() to flush dirty books.   *)
(* F4 fix: handle_request() re-reads paragraph after API call and discards *)
(*   translation if content changed (version guard before store).          *)
(***************************************************************************)

EXTENDS Integers, Sequences, FiniteSets, TLC

\* ========================================================================
\* Constants
\* ========================================================================

CONSTANTS
    Task,           \* Set of concurrent task IDs
    Book            \* Set of book IDs

ASSUME Task /= {}
ASSUME Book /= {}

\* ========================================================================
\* Variables
\* ========================================================================

\* --- Library lifecycle (F1: Stale Library Reference) ---
\* Models the captured Arc<Library> in background tasks vs current AppState.
\* In the implementation, TranslationQueue::init (translation_queue.rs:74-153)
\* captures library.clone() and passes it to spawned tasks. ConfigChange
\* creates a new Library but does NOT cancel existing tasks.
VARIABLES
    currentLib,     \* Nat — ID of Library currently in AppState
                    \* app.rs:66 — library: RwLock<Option<Arc<Library>>>
                    \* Incremented by ConfigChange (app.rs:152)
    taskLib         \* [Task -> Nat] — Library ID captured by each task
                    \* 0 = no library (task not started)
                    \* translation_queue.rs:95 — library.clone() at init

libVars == <<currentLib, taskLib>>

\* --- Task state ---
VARIABLES
    pc,             \* [Task -> String] — program counter
    taskType,       \* [Task -> {"idle","worker","tauri","watcher"}]
                    \*   worker  = translator+saver combined lifecycle
                    \*   tauri   = short-lived command handler
                    \*   watcher = file watcher triggered reload
    taskBook        \* [Task -> Book ∪ {"none"}] — current book target

taskVars == <<pc, taskType, taskBook>>

\* --- Event delivery (F2: Signal-Based Re-Fetch) ---
\* Models the getterToReadableWithEvents pattern (tauri.ts) where event
\* is a signal-only trigger (no payload); the frontend re-fetches via
\* invoke() on each signal, always reading the current backend state.
\* F2 FIX: stale snapshots eliminated because backend no longer computes
\* snapshots at emit time — the UI reads truthVersion at delivery time.
VARIABLES
    pendingEvents,      \* Nat — count of undelivered signal events
                        \* Each emit increments; each delivery decrements
    truthVersion,       \* Nat — logical clock of backend state
                        \* Incremented on each modification (import/delete/translate)
    uiVersion,          \* Nat — version currently shown in frontend UI
                        \* Set to truthVersion on each re-fetch (invoke)
    maxDeliveredVersion \* Nat — highest version ever delivered to UI
                        \* (tracking variable for EventMonotonicity invariant)

eventVars == <<pendingEvents, truthVersion, uiVersion, maxDeliveredVersion>>

\* --- Book versioning (F4: Translation Lifecycle Atomicity) ---
\* Models book content version changing between paragraph read and store.
\* In the implementation, file watcher can reload book content between
\* handle_request reading a paragraph and storing the translation.
VARIABLES
    bookVersion,        \* [Book -> Nat] — current book content version
                        \* library_book.rs:512-519 — incremented on reload
    taskReadVersion     \* [Task -> Nat] — version captured at ReadParagraph
                        \* translation_queue.rs:227-237

versionVars == <<bookVersion, taskReadVersion>>

\* --- Persistence (F3: Shutdown Persistence) ---
\* Models in-memory vs on-disk state for dirty tracking.
\* FIX: RunEvent::Exit handler calls save_all() to flush dirty books.
\* lib.rs:102-108 — block_on(app_state.save_all())
\* library.rs:303-314 — Library::save_all()
VARIABLES
    memVersion,     \* [Book -> Nat] — in-memory modification counter
                    \* Incremented by get_or_create_translation, mark_word_visible
    diskVersion,    \* [Book -> Nat] — on-disk modification counter
                    \* Updated by save() (library_book.rs:547-759)
    appAlive        \* BOOLEAN — whether app is running (FALSE after AppClose)

persistVars == <<memVersion, diskVersion, appAlive>>

allVars == <<libVars, taskVars, eventVars, versionVars, persistVars>>

\* ========================================================================
\* Helpers
\* ========================================================================

Max(a, b) == IF a >= b THEN a ELSE b

\* ========================================================================
\* Init
\* ========================================================================

Init ==
    /\ currentLib = 1
    /\ taskLib = [t \in Task |-> 0]
    /\ pc = [t \in Task |-> "idle"]
    /\ taskType = [t \in Task |-> "idle"]
    /\ taskBook = [t \in Task |-> "none"]
    /\ pendingEvents = 0
    /\ truthVersion = 1
    /\ uiVersion = 0
    /\ maxDeliveredVersion = 0
    /\ bookVersion = [b \in Book |-> 1]
    /\ taskReadVersion = [t \in Task |-> 0]
    /\ memVersion = [b \in Book |-> 0]
    /\ diskVersion = [b \in Book |-> 0]
    /\ appAlive = TRUE

\* ========================================================================
\* ConfigChange (F1: app.rs:104-173)
\* ========================================================================
\* Models update_config → eval_config flow:
\*   1. Sets translation_queue to None (app.rs:113)
\*      *self.translation_queue.write().await = None
\*   2. Creates new Library instance (app.rs:153)
\*      Arc::new(Library::open(PathBuf::from(&library_path)).await?)
\*   3. Emits library_updated with new library (app.rs:166-167)
\*
\* F1 FIX: TranslationQueue stores JoinHandles for all spawned tasks.
\* When dropped (step 1), Drop impl aborts all tasks immediately.
\* translation_queue.rs:71-77 — impl Drop for TranslationQueue
\* This prevents workers from continuing with a stale Arc<Library>.
\* Tauri commands and watcher are NOT affected (they read current lib).

ConfigChange ==
    /\ appAlive
    \* app.rs:153 — creates new Library, replaces old in AppState
    /\ currentLib' = currentLib + 1
    \* app.rs:166-167 — eval_config emits with the NEW library
    /\ truthVersion' = truthVersion + 1
    /\ pendingEvents' = pendingEvents + 1
    \* F1 FIX: abort all worker tasks — they captured the old library
    \* translation_queue.rs:71-77 — Drop calls abort() on all JoinHandles
    /\ pc' = [t \in Task |->
        IF taskType[t] = "worker" THEN "idle" ELSE pc[t]]
    /\ taskType' = [t \in Task |->
        IF taskType[t] = "worker" THEN "idle" ELSE taskType[t]]
    /\ taskBook' = [t \in Task |->
        IF taskType[t] = "worker" THEN "none" ELSE taskBook[t]]
    /\ taskLib' = [t \in Task |->
        IF taskType[t] = "worker" THEN 0 ELSE taskLib[t]]
    /\ taskReadVersion' = [t \in Task |->
        IF taskType[t] = "worker" THEN 0 ELSE taskReadVersion[t]]
    /\ UNCHANGED <<uiVersion, maxDeliveredVersion, bookVersion,
                   persistVars>>

\* ========================================================================
\* Worker Actions (F1, F3, F4)
\* ========================================================================
\* Models the combined translator→saver→emit lifecycle.
\* In the implementation, these are separate tasks communicating via channels:
\*   - Translator worker (translation_queue.rs:106-146)
\*   - Saver (translation_queue.rs:348-403)
\*   - Emitter (translation_queue.rs:446-472)
\* We combine them because the key issue (F1) is that ALL of them
\* capture Arc<Library> at init time (translation_queue.rs:75,95,108).
\*
\* PC states: idle → w_read → w_api → w_store → w_save → w_emit → idle

\* --- Start worker: assign book, capture current library ---
\* Models TranslationQueue::init (translation_queue.rs:74-153).
\* The library is captured at init time and passed to all spawned tasks:
\*   tokio::spawn(run_saver(library.clone(), ...)) — line 94-98
\*   let library = library.clone()                 — line 108
BeginWorker(t, b) ==
    /\ appAlive
    /\ pc[t] = "idle"
    /\ pc' = [pc EXCEPT ![t] = "w_read"]
    /\ taskType' = [taskType EXCEPT ![t] = "worker"]
    /\ taskBook' = [taskBook EXCEPT ![t] = b]
    \* Captures currentLib at this moment — never refreshed
    \* translation_queue.rs:75 — library: Arc<Library> parameter
    /\ taskLib' = [taskLib EXCEPT ![t] = currentLib]
    /\ UNCHANGED <<currentLib, eventVars, versionVars, persistVars>>

\* --- Read paragraph text + create translation in memory ---
\* Models handle_request (translation_queue.rs:227-237):
\*   let book = library.get_book(&request.book_id).await?;     — line 228
\*   let mut book = book.lock().await;                          — line 229
\*   let translation = book.get_or_create_translation(...);     — line 230
\*   let paragraph = book.book.paragraph_view(request...);      — line 231
\* Also models get_or_create_translation (library_book.rs:381-420) which
\* creates a new LibraryTranslation in memory without saving (F3).
WorkerReadParagraph(t) ==
    /\ appAlive
    /\ pc[t] = "w_read"
    /\ LET b == taskBook[t] IN
       \* F4: Capture book version at read time
       \* translation_queue.rs:228-231 — reads paragraph from book
       /\ taskReadVersion' = [taskReadVersion EXCEPT ![t] = bookVersion[b]]
       \* F3: get_or_create_translation creates in-memory state without save
       \* library_book.rs:381-420 — modifies translations vec, sets changed=true
       /\ memVersion' = [memVersion EXCEPT ![b] = memVersion[b] + 1]
    /\ pc' = [pc EXCEPT ![t] = "w_api"]
    /\ UNCHANGED <<libVars, taskType, taskBook, eventVars,
                   bookVersion, diskVersion, appAlive>>

\* --- External API call (no locks, no state change) ---
\* Models the translator.get_translation() call.
\* translation_queue.rs:311-313 — async HTTP request to LLM API.
\* No locks held; other tasks can interleave freely.
WorkerCallAPI(t) ==
    /\ appAlive
    /\ pc[t] = "w_api"
    /\ pc' = [pc EXCEPT ![t] = "w_store"]
    /\ UNCHANGED <<libVars, taskType, taskBook, eventVars,
                   versionVars, persistVars>>

\* --- Store translation result ---
\* Models translation_queue.rs:347-372 (post-F4 fix):
\*   Re-reads paragraph and compares with original snapshot.
\*   If bookVersion changed since WorkerReadParagraph, the paragraph content
\*   may have changed — discard the translation (return Err).
\*   Otherwise, store via add_paragraph_translation.
\* FIX (F4): The version guard prevents stale translations from being stored.
WorkerStoreResult(t) ==
    /\ appAlive
    /\ pc[t] = "w_store"
    /\ LET b == taskBook[t] IN
       IF taskReadVersion[t] = bookVersion[b]
       THEN \* Book unchanged since read — safe to store
            /\ truthVersion' = truthVersion + 1
            /\ pc' = [pc EXCEPT ![t] = "w_save"]
            /\ UNCHANGED <<libVars, taskType, taskBook,
                           pendingEvents, uiVersion, maxDeliveredVersion,
                           versionVars, persistVars>>
       ELSE \* Book changed — discard translation, return to idle
            /\ pc' = [pc EXCEPT ![t] = "idle"]
            /\ taskType' = [taskType EXCEPT ![t] = "idle"]
            /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
            /\ taskReadVersion' = [taskReadVersion EXCEPT ![t] = 0]
            /\ UNCHANGED <<libVars, pendingEvents,
                           truthVersion, uiVersion, maxDeliveredVersion,
                           bookVersion, persistVars>>

\* --- Save book to disk using captured library ---
\* Models save_book (translation_queue.rs:428-432):
\*   let book_handle = library.get_book(&book_id).await?;   — line 429
\*   let mut book = book_handle.lock().await;                — line 430
\*   book.save().await                                       — line 431
\* CRITICAL (F1): uses captured library (taskLib), NOT current AppState.
\* If taskLib /= currentLib, save goes to wrong disk path.
WorkerSave(t) ==
    /\ appAlive
    /\ pc[t] = "w_save"
    /\ LET b == taskBook[t] IN
       \* translation_queue.rs:439 — save_book(library.clone(), msg.book_id)
       \* If taskLib == currentLib: save reaches correct path, persists data
       \* If taskLib /= currentLib: save goes to old library's directory
       IF taskLib[t] = currentLib
       THEN diskVersion' = [diskVersion EXCEPT ![b] = memVersion[b]]
       ELSE UNCHANGED diskVersion  \* Data written to wrong path — lost!
    /\ pc' = [pc EXCEPT ![t] = "w_emit"]
    /\ UNCHANGED <<libVars, taskType, taskBook, eventVars,
                   versionVars, memVersion, appAlive>>

\* --- Emit library_updated signal ---
\* Models translation_queue.rs emit_updates:
\*   app.emit("library_updated", ())?
\* F2 FIX: emits signal only (no snapshot payload).
\* Frontend re-fetches via invoke on delivery.
WorkerEmit(t) ==
    /\ appAlive
    /\ pc[t] = "w_emit"
    /\ pendingEvents' = pendingEvents + 1
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ taskType' = [taskType EXCEPT ![t] = "idle"]
    /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
    /\ UNCHANGED <<libVars, truthVersion, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>

\* ========================================================================
\* Tauri Command Actions (F2)
\* ========================================================================
\* Models short-lived Tauri commands that modify book list and emit.
\* These use the CURRENT library from AppState (not a captured copy).
\* Affected commands:
\*   import_plain_text (library_view.rs:269-286)
\*   import_epub       (library_view.rs:288-301)
\*   delete_book       (library_view.rs:344-353)
\*   move_book         (library_view.rs:327-342)
\*
\* PC states: idle → tc_modify → tc_emit → idle

\* --- Start Tauri command ---
BeginTauri(t, b) ==
    /\ appAlive
    /\ pc[t] = "idle"
    /\ pc' = [pc EXCEPT ![t] = "tc_modify"]
    /\ taskType' = [taskType EXCEPT ![t] = "tauri"]
    /\ taskBook' = [taskBook EXCEPT ![t] = b]
    \* Tauri commands read current library from AppState each time
    \* library_view.rs:393 — state.library.read().await.clone()
    /\ taskLib' = [taskLib EXCEPT ![t] = currentLib]
    /\ UNCHANGED <<currentLib, eventVars, versionVars, persistVars>>

\* --- Modify book list (import/delete/move) ---
\* Models the actual mutation:
\*   library_view.rs:277-279 — library.create_book_plain/epub
\*   library_view.rs:349     — library.delete_book
\*   library_view.rs:335-336 — book.update_folder_path
\* Each modifies the backend state and persists atomically.
TauriModify(t) ==
    /\ appAlive
    /\ pc[t] = "tc_modify"
    /\ LET b == taskBook[t] IN
       /\ truthVersion' = truthVersion + 1
       \* Import/delete are immediately persisted to disk
       /\ memVersion' = [memVersion EXCEPT ![b] = memVersion[b] + 1]
       /\ diskVersion' = [diskVersion EXCEPT ![b] = memVersion[b] + 1]
    /\ pc' = [pc EXCEPT ![t] = "tc_emit"]
    /\ UNCHANGED <<libVars, taskType, taskBook,
                   pendingEvents, uiVersion, maxDeliveredVersion,
                   versionVars, appAlive>>

\* --- Emit library_updated signal from Tauri command ---
\* Models library_view.rs import/delete/move:
\*   self.app.emit("library_updated", ())?;
\* F2 FIX: signal only, no snapshot.
TauriEmit(t) ==
    /\ appAlive
    /\ pc[t] = "tc_emit"
    /\ pendingEvents' = pendingEvents + 1
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ taskType' = [taskType EXCEPT ![t] = "idle"]
    /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
    /\ UNCHANGED <<libVars, truthVersion, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>

\* ========================================================================
\* File Watcher Actions (F2, F4)
\* ========================================================================
\* Models app.rs:187-245 — handle_file_change_event.
\* File watcher reads CURRENT library from AppState (safe for F1):
\*   app.rs:188 — let library = { self.library.read().await.clone() };
\* But reloads book content (F4) and emits events (F2).
\*
\* PC states: idle → fw_reload → fw_emit → idle

\* --- Start file watcher handling ---
BeginWatcher(t, b) ==
    /\ appAlive
    /\ pc[t] = "idle"
    /\ pc' = [pc EXCEPT ![t] = "fw_reload"]
    /\ taskType' = [taskType EXCEPT ![t] = "watcher"]
    /\ taskBook' = [taskBook EXCEPT ![t] = b]
    \* Watcher reads current library from AppState each time (not captured)
    \* app.rs:188 — self.library.read().await.clone()
    /\ taskLib' = [taskLib EXCEPT ![t] = currentLib]
    /\ UNCHANGED <<currentLib, eventVars, versionVars, persistVars>>

\* --- Reload book content ---
\* Models library.rs:308-316 → library_book.rs:512-519 reload_book:
\*   Detects external modification, reloads book from disk.
\*   This changes the book's content version (F4).
WatcherReload(t) ==
    /\ appAlive
    /\ pc[t] = "fw_reload"
    /\ LET b == taskBook[t] IN
       \* library_book.rs:512-519 — reload_book/reload_translations
       /\ bookVersion' = [bookVersion EXCEPT ![b] = bookVersion[b] + 1]
       /\ truthVersion' = truthVersion + 1
       \* Reload triggers save of dirty state (merge path)
       \* library_book.rs:521-540 — reload_translations → save() if newer
       /\ diskVersion' = [diskVersion EXCEPT ![b] = memVersion[b]]
    /\ pc' = [pc EXCEPT ![t] = "fw_emit"]
    /\ UNCHANGED <<libVars, taskType, taskBook,
                   pendingEvents, uiVersion, maxDeliveredVersion,
                   taskReadVersion, memVersion, appAlive>>

\* --- Emit library_updated signal from watcher ---
\* Models app.rs handle_file_change_event:
\*   self.app.emit("library_updated", ())?;
\* F2 FIX: signal only, no snapshot.
WatcherEmit(t) ==
    /\ appAlive
    /\ pc[t] = "fw_emit"
    /\ pendingEvents' = pendingEvents + 1
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ taskType' = [taskType EXCEPT ![t] = "idle"]
    /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
    /\ UNCHANGED <<libVars, truthVersion, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>

\* ========================================================================
\* Frontend Event Delivery (F2: FIXED)
\* ========================================================================
\* Models the getterToReadableWithEvents pattern (tauri.ts):
\* Signal arrives → frontend calls invoke("list_books") → reads current
\* backend state (truthVersion).
\*
\* F2 FIX: The frontend no longer uses event payload as store value.
\* Instead, each signal triggers a re-fetch that always reads the latest
\* backend state. This eliminates stale snapshot overwrites.

DeliverEvent ==
    /\ pendingEvents > 0
    \* Frontend re-fetches via invoke — reads current truthVersion
    /\ uiVersion' = truthVersion
    /\ maxDeliveredVersion' = Max(maxDeliveredVersion, truthVersion)
    /\ pendingEvents' = pendingEvents - 1
    /\ UNCHANGED <<libVars, taskVars, truthVersion,
                   versionVars, persistVars>>

\* ========================================================================
\* Mark Word Visible (F3: library_view.rs:355-377)
\* ========================================================================
\* Models mark_word_visible command:
\*   let mut bt = book_translation.lock().await;            — line 367
\*   bt.mark_word_visible(paragraph_id, flat_index)          — line 368
\*   if result { book.save().await?; }                       — line 372-373
\* Atomic: modifies memory AND saves to disk in one step.
\* NOT a persistence gap (verified: save is called immediately).

MarkWordVisible(b) ==
    /\ appAlive
    \* library_book.rs:187-193 — mark_word_visible sets changed=true
    /\ memVersion' = [memVersion EXCEPT ![b] = memVersion[b] + 1]
    \* library_view.rs:372-373 — book.save().await? (immediate persist)
    /\ diskVersion' = [diskVersion EXCEPT ![b] = memVersion[b] + 1]
    /\ UNCHANGED <<libVars, taskVars, eventVars, versionVars, appAlive>>

\* ========================================================================
\* App Close (F3: lib.rs:101-109 — shutdown handler)
\* ========================================================================
\* F3 FIX: RunEvent::Exit handler calls save_all() before exit.
\* lib.rs:102-108 — block_on(app_state.save_all())
\* library.rs:303-314 — Library::save_all() iterates books, saves dirty ones
\* All in-memory state (memVersion) is flushed to disk (diskVersion).

AppClose ==
    /\ appAlive
    \* F3 FIX: save_all() persists all dirty books before exit
    /\ diskVersion' = [b \in Book |-> memVersion[b]]
    /\ appAlive' = FALSE
    /\ UNCHANGED <<libVars, taskVars, eventVars, versionVars, memVersion>>

\* ========================================================================
\* Next
\* ========================================================================

Next ==
    \/ ConfigChange
    \/ \E t \in Task, b \in Book :
        \/ BeginWorker(t, b)
        \/ BeginTauri(t, b)
        \/ BeginWatcher(t, b)
    \/ \E t \in Task :
        \* Worker lifecycle (F1, F3, F4)
        \/ WorkerReadParagraph(t)
        \/ WorkerCallAPI(t)
        \/ WorkerStoreResult(t)
        \/ WorkerSave(t)
        \/ WorkerEmit(t)
        \* Tauri command lifecycle (F2: fixed)
        \/ TauriModify(t)
        \/ TauriEmit(t)
        \* Watcher lifecycle (F2: fixed, F4)
        \/ WatcherReload(t)
        \/ WatcherEmit(t)
    \/ DeliverEvent
    \/ \E b \in Book : MarkWordVisible(b)
    \/ AppClose

Spec == Init /\ [][Next]_allVars

\* ========================================================================
\* Invariants
\* ========================================================================

\* --- F1: Stale Library Safety --- [FIXED: should pass]
\* No background worker task operates with a stale library reference.
\* FIX: ConfigChange now aborts all worker tasks (TranslationQueue::Drop
\* calls JoinHandle::abort on all spawned tasks). Workers are reset to
\* idle before any can continue with a stale library reference.
StaleLibrarySafety ==
    \A t \in Task :
        (taskType[t] = "worker" /\ pc[t] /= "idle") =>
            taskLib[t] = currentLib

\* --- F1: No Data Loss --- [FIXED: should pass]
\* Translation data is never saved to a library whose path differs from
\* the current configured path. Workers are aborted before reaching
\* the save step with a stale library reference.
NoDataLoss ==
    \A t \in Task :
        pc[t] = "w_save" => taskLib[t] = currentLib

\* --- F2: Event Monotonicity --- [FIXED: should pass]
\* The frontend UI version never regresses.
\* FIX: getterToReadableWithEvents re-fetches via invoke on each signal,
\* always reading current truthVersion. Since truthVersion is monotonically
\* non-decreasing, uiVersion can only increase.
EventMonotonicity ==
    uiVersion >= maxDeliveredVersion

\* --- F2: UI Consistency --- [FIXED: should pass]
\* When all pending signals have been delivered and no task is in the
\* middle of an operation, the UI should reflect the latest backend state.
\* FIX: last DeliverEvent sets uiVersion = truthVersion; with no tasks
\* active and no pending signals, no further changes have occurred.
UIConsistency ==
    (/\ pendingEvents = 0
     /\ \A t \in Task : pc[t] \notin {"w_read", "w_api", "w_store",
                                       "w_save", "w_emit",
                                       "tc_modify", "tc_emit",
                                       "fw_reload", "fw_emit"}
     /\ maxDeliveredVersion > 0)
    => uiVersion = truthVersion

\* --- F3: No Persistence Loss --- [FIXED: should pass]
\* When the app terminates, all in-memory modifications must have been
\* persisted to disk.
\* FIX: RunEvent::Exit handler calls save_all() which iterates all books
\* and saves any with unsaved changes (lib.rs:102-108, library.rs:303-314).
NoPersistenceLoss ==
    ~appAlive => \A b \in Book : memVersion[b] <= diskVersion[b]

\* --- F4: No Stale Translation --- [FIXED]
\* A translation must not be stored against a book version different from the
\* one its source paragraph was read from.
\* FIX: handle_request() re-reads the paragraph after the API call and
\* compares text — if changed, the translation is discarded (translation_queue.rs:346-365).
\* In the spec, WorkerStoreResult checks taskReadVersion[t] = bookVersion[b].
\*
\* This is an ACTION property (not a state invariant): the state w_store with
\* mismatched versions is reachable, but the w_store→w_save transition is
\* guarded — the worker discards and returns to idle instead.
\* We express this as [][A => P]_vars: for every step, if a task transitions
\* from w_store to w_save, the versions must have matched.
NoStaleTranslation ==
    [][\A t \in Task :
        (pc[t] = "w_store" /\ pc'[t] = "w_save") =>
            taskReadVersion[t] = bookVersion[taskBook[t]]]_allVars

\* --- Structural: PC Consistency ---
\* Non-idle tasks must have a valid type assigned.
PCConsistency ==
    \A t \in Task :
        (pc[t] /= "idle") => (taskType[t] /= "idle")

\* --- Structural: Task Library Validity ---
\* A non-idle task must have a valid library reference.
TaskLibraryValidity ==
    \A t \in Task :
        (pc[t] /= "idle") => (taskLib[t] > 0)

====
