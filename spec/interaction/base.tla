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
(*   F1 — Stale library reference after config reconfiguration             *)
(*   F2 — Event ordering / stale snapshot overwrites                       *)
(*   F3 — Unsaved in-memory modifications / no shutdown persistence        *)
(*   F4 — Translation lifecycle cross-component atomicity                  *)
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

\* --- Event delivery (F2: Stale Snapshot Overwrites) ---
\* Models the eventToReadable pattern (tauri.ts:7-42) where event
\* payload IS the store value, delivered in FIFO emit order.
\* Multiple concurrent emitters produce snapshots at different times;
\* the last-delivered event wins regardless of freshness.
VARIABLES
    pendingEvents,      \* Seq of Nat — FIFO queue of snapshot versions
                        \* Each element is the truthVersion at snapshot time
    taskSnapshot,       \* [Task -> Nat] — version captured at ComputeSnapshot
                        \* 0 = no snapshot computed
    truthVersion,       \* Nat — logical clock of backend state
                        \* Incremented on each modification (import/delete/translate)
    uiVersion,          \* Nat — version currently shown in frontend UI
                        \* tauri.ts:12 — setter(event.payload)
    maxDeliveredVersion \* Nat — highest version ever delivered to UI
                        \* (tracking variable for EventMonotonicity invariant)

eventVars == <<pendingEvents, taskSnapshot, truthVersion,
               uiVersion, maxDeliveredVersion>>

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

\* --- Persistence (F3: No Shutdown Persistence) ---
\* Models in-memory vs on-disk state for dirty tracking.
\* In the implementation, get_or_create_translation creates in-memory
\* state without saving; no shutdown handler persists dirty state.
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
    /\ pendingEvents = <<>>
    /\ taskSnapshot = [t \in Task |-> 0]
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
\*   1. Sets translation_queue to None (app.rs:112)
\*      *self.translation_queue.write().await = None
\*   2. Creates new Library instance (app.rs:152)
\*      Arc::new(Library::open(PathBuf::from(&library_path)).await?)
\*   3. Emits library_updated with new library (app.rs:163-166)
\*      LibraryView::create(self.app.clone(), library.clone())
\*      ... self.app.emit("library_updated", books)?
\*
\* Key: spawned background tasks keep their old Arc<Library> references.
\* No cancellation mechanism exists (no abort/cancel/JoinHandle found).

ConfigChange ==
    /\ appAlive
    \* app.rs:152 — creates new Library, replaces old in AppState
    /\ currentLib' = currentLib + 1
    \* app.rs:163-166 — eval_config emits with the NEW library
    /\ truthVersion' = truthVersion + 1
    /\ pendingEvents' = Append(pendingEvents, truthVersion + 1)
    \* Existing tasks keep their stale taskLib — this is the F1 bug
    /\ UNCHANGED <<taskLib, taskVars, taskSnapshot,
                   uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>

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
\* PC states: idle → w_read → w_api → w_store → w_save →
\*            w_snapshot → w_emit → idle

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
\* Models translation_queue.rs:330-334:
\*   translation.lock().await
\*       .add_paragraph_translation(request.paragraph_id, &p_translation, ...)
\* This is a backend state modification — increments truthVersion.
WorkerStoreResult(t) ==
    /\ appAlive
    /\ pc[t] = "w_store"
    \* Backend state changes: new translation stored
    /\ truthVersion' = truthVersion + 1
    /\ pc' = [pc EXCEPT ![t] = "w_save"]
    /\ UNCHANGED <<libVars, taskType, taskBook,
                   pendingEvents, taskSnapshot, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>

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
    /\ pc' = [pc EXCEPT ![t] = "w_snapshot"]
    /\ UNCHANGED <<libVars, taskType, taskBook, eventVars,
                   versionVars, memVersion, appAlive>>

\* --- Compute snapshot (list_books) ---
\* Models emit_updates (translation_queue.rs:446-469):
\*   let lv = LibraryView::create(app.clone(), library.clone());  — line 451
\*   let books = lv.list_books(Some(&msg.target_language)).await?; — line 467
\* The snapshot reflects truthVersion at THIS moment.
\* Uses captured library (taskLib) for the query.
\* If taskLib /= currentLib, the snapshot is from the wrong library (F1).
WorkerComputeSnapshot(t) ==
    /\ appAlive
    /\ pc[t] = "w_snapshot"
    \* translation_queue.rs:467 — lv.list_books(...)
    \* Captures current truthVersion as the snapshot version
    /\ taskSnapshot' = [taskSnapshot EXCEPT ![t] = truthVersion]
    /\ pc' = [pc EXCEPT ![t] = "w_emit"]
    /\ UNCHANGED <<libVars, taskType, taskBook,
                   pendingEvents, truthVersion, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>

\* --- Emit library_updated event ---
\* Models translation_queue.rs:468-469:
\*   app.emit("library_updated", books)?
\* Adds snapshot to pending events FIFO.
\* app.emit() is synchronous — no interleaving within a single emit call.
WorkerEmit(t) ==
    /\ appAlive
    /\ pc[t] = "w_emit"
    \* translation_queue.rs:469 — app.emit("library_updated", books)
    /\ pendingEvents' = Append(pendingEvents, taskSnapshot[t])
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ taskType' = [taskType EXCEPT ![t] = "idle"]
    /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
    /\ taskSnapshot' = [taskSnapshot EXCEPT ![t] = 0]
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
\* PC states: idle → tc_modify → tc_snapshot → tc_emit → idle

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
    /\ pc' = [pc EXCEPT ![t] = "tc_snapshot"]
    /\ UNCHANGED <<libVars, taskType, taskBook,
                   pendingEvents, taskSnapshot, uiVersion, maxDeliveredVersion,
                   versionVars, appAlive>>

\* --- Compute snapshot for Tauri command ---
\* Models library_view.rs:282,297,339,350:
\*   let books = self.list_books(target_language).await?;
TauriComputeSnapshot(t) ==
    /\ appAlive
    /\ pc[t] = "tc_snapshot"
    /\ taskSnapshot' = [taskSnapshot EXCEPT ![t] = truthVersion]
    /\ pc' = [pc EXCEPT ![t] = "tc_emit"]
    /\ UNCHANGED <<libVars, taskType, taskBook,
                   pendingEvents, truthVersion, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>

\* --- Emit library_updated from Tauri command ---
\* Models library_view.rs:283,298,340,351:
\*   self.app.emit("library_updated", books)?;
TauriEmit(t) ==
    /\ appAlive
    /\ pc[t] = "tc_emit"
    /\ pendingEvents' = Append(pendingEvents, taskSnapshot[t])
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ taskType' = [taskType EXCEPT ![t] = "idle"]
    /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
    /\ taskSnapshot' = [taskSnapshot EXCEPT ![t] = 0]
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
\* PC states: idle → fw_reload → fw_snapshot → fw_emit → idle

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
    /\ pc' = [pc EXCEPT ![t] = "fw_snapshot"]
    /\ UNCHANGED <<libVars, taskType, taskBook,
                   pendingEvents, taskSnapshot, uiVersion, maxDeliveredVersion,
                   taskReadVersion, memVersion, appAlive>>

\* --- Compute snapshot for watcher ---
\* Models app.rs:205-210:
\*   let library_view = LibraryView::create(self.app.clone(), library.clone());
\*   self.app.emit("library_updated",
\*       library_view.list_books(target_language.as_ref()).await?)?;
WatcherComputeSnapshot(t) ==
    /\ appAlive
    /\ pc[t] = "fw_snapshot"
    /\ taskSnapshot' = [taskSnapshot EXCEPT ![t] = truthVersion]
    /\ pc' = [pc EXCEPT ![t] = "fw_emit"]
    /\ UNCHANGED <<libVars, taskType, taskBook,
                   pendingEvents, truthVersion, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>

\* --- Emit library_updated from watcher ---
\* Models app.rs:208-211:
\*   self.app.emit("library_updated",
\*       library_view.list_books(target_language.as_ref()).await?)?;
WatcherEmit(t) ==
    /\ appAlive
    /\ pc[t] = "fw_emit"
    /\ pendingEvents' = Append(pendingEvents, taskSnapshot[t])
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ taskType' = [taskType EXCEPT ![t] = "idle"]
    /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
    /\ taskSnapshot' = [taskSnapshot EXCEPT ![t] = 0]
    /\ UNCHANGED <<libVars, truthVersion, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>

\* ========================================================================
\* Frontend Event Delivery (F2)
\* ========================================================================
\* Models tauri.ts:7-42 — eventToReadable:
\*   listen<T>(eventName, (event) => {
\*       if (setter) { setter(event.payload); }      — line 12
\*   })
\* Events are delivered in FIFO order (Tauri IPC is ordered within
\* the same process). The payload IS the store value (no re-query).

DeliverEvent ==
    /\ pendingEvents /= <<>>
    /\ LET v == Head(pendingEvents) IN
       \* tauri.ts:12 — setter(event.payload)
       \* The frontend blindly applies the payload, no version check
       /\ uiVersion' = v
       /\ maxDeliveredVersion' = Max(maxDeliveredVersion, v)
       /\ pendingEvents' = Tail(pendingEvents)
    /\ UNCHANGED <<libVars, taskVars, taskSnapshot, truthVersion,
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
\* App Close (F3: lib.rs — no shutdown handler)
\* ========================================================================
\* Models app termination — no shutdown handler exists.
\* lib.rs:1-100 — NO on_exit(), close_requested, or shutdown callback.
\* Any unsaved in-memory state (memVersion > diskVersion) is LOST.

AppClose ==
    /\ appAlive
    /\ appAlive' = FALSE
    /\ UNCHANGED <<libVars, taskVars, eventVars, versionVars,
                   memVersion, diskVersion>>

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
        \/ WorkerComputeSnapshot(t)
        \/ WorkerEmit(t)
        \* Tauri command lifecycle (F2)
        \/ TauriModify(t)
        \/ TauriComputeSnapshot(t)
        \/ TauriEmit(t)
        \* Watcher lifecycle (F2, F4)
        \/ WatcherReload(t)
        \/ WatcherComputeSnapshot(t)
        \/ WatcherEmit(t)
    \/ DeliverEvent
    \/ \E b \in Book : MarkWordVisible(b)
    \/ AppClose

Spec == Init /\ [][Next]_allVars

\* ========================================================================
\* Invariants
\* ========================================================================

\* --- F1: Stale Library Safety ---
\* No background worker task operates with a stale library reference.
\* Violation means a task captured an old Arc<Library> before ConfigChange
\* and is now saving/emitting using the stale reference.
\* In the implementation: translation_queue.rs:95 captures library at init,
\* but ConfigChange (app.rs:112,152) creates a new library without
\* cancelling existing tasks.
StaleLibrarySafety ==
    \A t \in Task :
        (taskType[t] = "worker" /\ pc[t] /= "idle") =>
            taskLib[t] = currentLib

\* --- F1: No Data Loss ---
\* Translation data is never saved to a library whose path differs from
\* the current configured path. Specifically, WorkerSave must use the
\* current library.
\* Violation: save_book (translation_queue.rs:439) uses captured library
\* reference that points to the old library's disk directory.
NoDataLoss ==
    \A t \in Task :
        pc[t] = "w_save" => taskLib[t] = currentLib

\* --- F2: Event Monotonicity ---
\* The frontend UI version never regresses. A stale event should not
\* overwrite a fresher one. Violation means eventToReadable (tauri.ts:12)
\* applied an older snapshot after a newer one.
\* This fails when two tasks emit in an order where the later emit
\* carries an older snapshot (computed before a concurrent modification).
EventMonotonicity ==
    uiVersion >= maxDeliveredVersion

\* --- F2: UI Consistency ---
\* When all pending events have been delivered and no task is in the
\* middle of a compute-snapshot→emit flow, the UI should reflect the
\* latest backend state. Violation means stale events corrupted the UI.
\* Guard: only applies after the UI has received at least one event
\* (maxDeliveredVersion > 0), since the UI starts empty.
UIConsistency ==
    (/\ pendingEvents = <<>>
     /\ \A t \in Task : pc[t] \notin {"w_read", "w_api", "w_store",
                                       "w_save", "w_snapshot", "w_emit",
                                       "tc_modify", "tc_snapshot", "tc_emit",
                                       "fw_reload", "fw_snapshot", "fw_emit"}
     /\ maxDeliveredVersion > 0)
    => uiVersion = truthVersion

\* --- F3: No Persistence Loss ---
\* When the app terminates, all in-memory modifications must have been
\* persisted to disk. Violation means data loss on app close.
\* In the implementation: no shutdown handler exists (lib.rs has no on_exit).
NoPersistenceLoss ==
    ~appAlive => \A b \in Book : memVersion[b] <= diskVersion[b]

\* --- F4: No Stale Translation ---
\* A translation is not stored against a book version different from the
\* one its source paragraph was read from. Violation means the book was
\* reloaded (e.g., by file watcher) between paragraph read and translation
\* store, so the translation may reference the wrong paragraph content.
\* In the implementation: translation_queue.rs:227-237 reads paragraph,
\* then later lines 330-334 store the result — with no version check.
NoStaleTranslation ==
    \A t \in Task :
        pc[t] = "w_store" =>
            taskReadVersion[t] = bookVersion[taskBook[t]]

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
