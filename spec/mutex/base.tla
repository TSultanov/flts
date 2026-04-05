---- MODULE base ----
(***************************************************************************)
(* TLA+ specification of FLTS Mutex / Lock Safety.                         *)
(*                                                                         *)
(* Models the lock hierarchy and concurrent task patterns in FLTS to       *)
(* verify deadlock freedom, lock ordering consistency, and TOCTOU races.   *)
(*                                                                         *)
(* Source: library/src/library/ + site/src-tauri/src/app/                   *)
(*   library.rs              — Library struct, get_book, books_cache        *)
(*   library_book.rs         — LibraryBook, save(), get_or_create_trans    *)
(*   library_dictionary.rs   — DictionaryCache, get_dictionary             *)
(*   translation_queue.rs    — TranslationQueue, translate(), saver        *)
(*   library_view.rs         — list_book_chapter_paragraphs, mark_word     *)
(*   app.rs                  — AppState, handle_file_change_event          *)
(*                                                                         *)
(* Bug Families:                                                           *)
(*   F1 — Lock starvation via long-held book Mutex during save()           *)
(*   F2 — TOCTOU in translation queue request deduplication                *)
(*   F3 — Fragile double-lock in get_or_create_translation                 *)
(***************************************************************************)

EXTENDS Integers, Sequences, FiniteSets, TLC

\* ========================================================================
\* Constants
\* ========================================================================

CONSTANTS
    Task,           \* Set of concurrent task IDs
    Book,           \* Set of book IDs (typically 1-2 for model checking)
    Translation,    \* Set of translation IDs per book
    Dictionary,     \* Set of dictionary IDs
    Paragraph       \* Set of paragraph IDs (for Family 2 TOCTOU model)

ASSUME Task /= {}
ASSUME Book /= {}
ASSUME Translation /= {}

\* ========================================================================
\* Variables
\* ========================================================================

\* --- Lock state ---
\* Mutex locks: held by a task or "none"
\* Models tokio::sync::Mutex — at most one holder, others block
VARIABLES
    bookLock,       \* [Book -> Task ∪ {"none"}]
                    \* library.rs:201 — per-book Arc<Mutex<LibraryBook>>
    transLock,      \* [Translation -> Task ∪ {"none"}]
                    \* library_book.rs:57 — per-translation Arc<Mutex<LibraryTranslation>>
    dictLock,       \* [Dictionary -> Task ∪ {"none"}]
                    \* library_dictionary.rs:234 — per-dict Arc<Mutex<LibraryDictionary>>
    queueLock       \* Task ∪ {"none"}
                    \* translation_queue.rs:63 — TranslationQueue.state Mutex

lockVars == <<bookLock, transLock, dictLock, queueLock>>

\* --- Per-task state ---
VARIABLES
    pc,             \* [Task -> String] — program counter
    role,           \* [Task -> {"idle","watcher","tauri_list","tauri_mark",
                    \*           "translator","saver"}]
    taskBook,       \* [Task -> Book ∪ {"none"}] — current book target
    taskTrans,      \* [Task -> Translation ∪ {"none"}] — current translation
    taskDict,       \* [Task -> Dictionary ∪ {"none"}] — current dictionary
    taskParagraph   \* [Task -> Paragraph ∪ {"none"}] — current paragraph

taskVars == <<pc, role, taskBook, taskTrans, taskDict, taskParagraph>>

\* --- Family 1: Save contention tracking ---
VARIABLES
    waitingForBook  \* [Book -> {Task}] — tasks blocked waiting for book lock
                    \* Tracks contention depth on per-book Mutex

contentionVars == <<waitingForBook>>

\* --- Family 2: Translation request dedup ---
\* Models translation_queue.rs:155-184 — translate() atomic check+insert under lock
VARIABLES
    requestMap,     \* [Book × Paragraph -> Nat ∪ {0}]
                    \* 0 means no active request; >0 is request_id
                    \* translation_queue.rs:55 — paragraph_request_id_map
    nextRequestId,  \* Nat — monotonically increasing request counter
                    \* translation_queue.rs:60 — AtomicUsize
    requestSent     \* [Task -> BOOLEAN] — task has been assigned a request
                    \* (map entry exists, request sent or about to send)

dedupVars == <<requestMap, nextRequestId, requestSent>>

\* --- Family 3: Double-lock tracking ---
\* Models library_book.rs:375-377 — get_or_create_translation acquires
\* the same translation Mutex twice in an if-condition
VARIABLES
    transLockCount  \* [Task -> Nat] — how many times task has acquired
                    \* a translation lock (detects self-deadlock if >1
                    \* while still holding)

doubleLockVars == <<transLockCount>>

allVars == <<lockVars, taskVars, contentionVars, dedupVars, doubleLockVars>>

\* ========================================================================
\* Helpers
\* ========================================================================

\* A task can acquire a Mutex iff it is not held by anyone
MutexAvailable(holder) == holder = "none"

\* Lock level mapping for ordering invariant (library hierarchy)
\* Level 0: bookLock (L6 in brief = L3 in hierarchy)
\* Level 1: transLock (L7 = L4)
\* Level 2: dictLock (L9 = L6)
\* queueLock is independent (never nested with the above)
LockLevel(lockName) ==
    CASE lockName = "book" -> 0
      [] lockName = "trans" -> 1
      [] lockName = "dict" -> 2
      [] lockName = "queue" -> 99  \* independent

\* Max lock level currently held by task t
\* Returns -1 if no locks held
MaxLockLevel(t) ==
    LET bookLevel == IF \E b \in Book : bookLock[b] = t THEN 0 ELSE -1
        transLevel == IF \E tr \in Translation : transLock[tr] = t THEN 1 ELSE -1
        dictLevel == IF \E d \in Dictionary : dictLock[d] = t THEN 2 ELSE -1
    IN
    IF dictLevel >= 0 THEN dictLevel
    ELSE IF transLevel >= 0 THEN transLevel
    ELSE bookLevel

\* Whether a task holds any lock in the book hierarchy
HoldsAnyHierarchyLock(t) ==
    \/ \E b \in Book : bookLock[b] = t
    \/ \E tr \in Translation : transLock[tr] = t
    \/ \E d \in Dictionary : dictLock[d] = t

\* ========================================================================
\* Init
\* ========================================================================

Init ==
    \* All locks free
    /\ bookLock = [b \in Book |-> "none"]
    /\ transLock = [tr \in Translation |-> "none"]
    /\ dictLock = [d \in Dictionary |-> "none"]
    /\ queueLock = "none"
    \* All tasks idle
    /\ pc = [t \in Task |-> "idle"]
    /\ role = [t \in Task |-> "idle"]
    /\ taskBook = [t \in Task |-> "none"]
    /\ taskTrans = [t \in Task |-> "none"]
    /\ taskDict = [t \in Task |-> "none"]
    /\ taskParagraph = [t \in Task |-> "none"]
    \* Family 1
    /\ waitingForBook = [b \in Book |-> {}]
    \* Family 2
    /\ requestMap = [bp \in Book \X Paragraph |-> 0]
    /\ nextRequestId = 1
    /\ requestSent = [t \in Task |-> FALSE]
    \* Family 3
    /\ transLockCount = [t \in Task |-> 0]

\* ========================================================================
\* Watcher Actions (app.rs:187-245, library.rs:302-340)
\* ========================================================================
\* File watcher: detects changes, reloads books/translations.
\* Lock sequence: bookLock → transLock → dictLock (during save)

\* --- Start watcher task: choose a book to reload ---
\* Models library.rs:308-315 — books_cache.read() then book.lock()
BeginWatcher(t, b) ==
    /\ pc[t] = "idle"
    /\ pc' = [pc EXCEPT ![t] = "w_acq_book"]
    /\ role' = [role EXCEPT ![t] = "watcher"]
    /\ taskBook' = [taskBook EXCEPT ![t] = b]
    /\ UNCHANGED <<lockVars, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars, doubleLockVars>>

\* --- Acquire book lock ---
\* Models library.rs:311 — book.lock().await
WatcherAcqBook(t) ==
    /\ pc[t] = "w_acq_book"
    /\ LET b == taskBook[t] IN
       /\ MutexAvailable(bookLock[b])
       /\ bookLock' = [bookLock EXCEPT ![b] = t]
       /\ waitingForBook' = [waitingForBook EXCEPT ![b] =
            waitingForBook[b] \ {t}]
    /\ pc' = [pc EXCEPT ![t] = "w_hold_book"]
    /\ UNCHANGED <<transLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   dedupVars, doubleLockVars>>

\* --- Register as waiting for book lock (for contention tracking) ---
\* Family 1: tracks tasks blocked on book Mutex
WatcherWaitBook(t) ==
    /\ pc[t] = "w_acq_book"
    /\ LET b == taskBook[t] IN
       /\ ~MutexAvailable(bookLock[b])
       /\ bookLock[b] /= t
       /\ waitingForBook' = [waitingForBook EXCEPT ![b] =
            waitingForBook[b] \cup {t}]
    /\ UNCHANGED <<pc, lockVars, role, taskBook, taskTrans, taskDict,
                   taskParagraph, dedupVars, doubleLockVars>>

\* --- Inside save(): acquire translation lock ---
\* Models library_book.rs:534 — translation_arc.lock().await
WatcherAcqTrans(t, tr) ==
    /\ pc[t] = "w_hold_book"
    /\ MutexAvailable(transLock[tr])
    /\ transLock' = [transLock EXCEPT ![tr] = t]
    /\ taskTrans' = [taskTrans EXCEPT ![t] = tr]
    /\ transLockCount' = [transLockCount EXCEPT ![t] = 1]
    /\ pc' = [pc EXCEPT ![t] = "w_hold_trans"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Release translation lock after I/O, move to dict ---
\* Models library_book.rs:626-632 — after save translation, proceed to dict
WatcherRelTrans(t) ==
    /\ pc[t] = "w_hold_trans"
    /\ LET tr == taskTrans[t] IN
       /\ transLock' = [transLock EXCEPT ![tr] = "none"]
       /\ transLockCount' = [transLockCount EXCEPT ![t] = 0]
    /\ taskTrans' = [taskTrans EXCEPT ![t] = "none"]
    /\ pc' = [pc EXCEPT ![t] = "w_pre_dict"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Acquire dictionary lock ---
\* Models library_book.rs:639-641 — dict.lock().await.save()
WatcherAcqDict(t, d) ==
    /\ pc[t] = "w_pre_dict"
    /\ MutexAvailable(dictLock[d])
    /\ dictLock' = [dictLock EXCEPT ![d] = t]
    /\ taskDict' = [taskDict EXCEPT ![t] = d]
    /\ pc' = [pc EXCEPT ![t] = "w_hold_dict"]
    /\ UNCHANGED <<bookLock, transLock, queueLock, role,
                   taskBook, taskTrans, taskParagraph,
                   contentionVars, dedupVars, doubleLockVars>>

\* --- Release dictionary lock ---
\* Models library_book.rs:641 — after dict.save()
WatcherRelDict(t) ==
    /\ pc[t] = "w_hold_dict"
    /\ LET d == taskDict[t] IN
       dictLock' = [dictLock EXCEPT ![d] = "none"]
    /\ taskDict' = [taskDict EXCEPT ![t] = "none"]
    /\ pc' = [pc EXCEPT ![t] = "w_save_book"]
    /\ UNCHANGED <<bookLock, transLock, queueLock, role,
                   taskBook, taskTrans, taskParagraph,
                   contentionVars, dedupVars, doubleLockVars>>

\* --- Save book.dat and release book lock ---
\* Models library_book.rs:643-711 — write book.dat, then release
WatcherRelBook(t) ==
    /\ pc[t] = "w_save_book"
    /\ LET b == taskBook[t] IN
       /\ bookLock' = [bookLock EXCEPT ![b] = "none"]
       /\ waitingForBook' = [waitingForBook EXCEPT ![b] =
            waitingForBook[b] \ {t}]
    /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ role' = [role EXCEPT ![t] = "idle"]
    /\ UNCHANGED <<transLock, dictLock, queueLock,
                   taskTrans, taskDict, taskParagraph,
                   dedupVars, doubleLockVars>>

\* ========================================================================
\* Tauri Command: list_book_chapter_paragraphs (library_view.rs:184-218)
\* ========================================================================
\* Lock sequence: bookLock → transLock (per paragraph, while holding book)
\* Family 1: holds book lock while iterating paragraphs with trans lock

\* --- Begin: choose book ---
BeginTauriList(t, b) ==
    /\ pc[t] = "idle"
    /\ pc' = [pc EXCEPT ![t] = "tl_acq_book"]
    /\ role' = [role EXCEPT ![t] = "tauri_list"]
    /\ taskBook' = [taskBook EXCEPT ![t] = b]
    /\ UNCHANGED <<lockVars, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars, doubleLockVars>>

\* --- Acquire book lock ---
\* Models library_view.rs:190-191 — get_book then book.lock()
TauriListAcqBook(t) ==
    /\ pc[t] = "tl_acq_book"
    /\ LET b == taskBook[t] IN
       /\ MutexAvailable(bookLock[b])
       /\ bookLock' = [bookLock EXCEPT ![b] = t]
       /\ waitingForBook' = [waitingForBook EXCEPT ![b] =
            waitingForBook[b] \ {t}]
    /\ pc' = [pc EXCEPT ![t] = "tl_get_trans"]
    /\ UNCHANGED <<transLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   dedupVars, doubleLockVars>>

\* --- Wait for book lock (contention tracking) ---
TauriListWaitBook(t) ==
    /\ pc[t] = "tl_acq_book"
    /\ LET b == taskBook[t] IN
       /\ ~MutexAvailable(bookLock[b])
       /\ bookLock[b] /= t
       /\ waitingForBook' = [waitingForBook EXCEPT ![b] =
            waitingForBook[b] \cup {t}]
    /\ UNCHANGED <<pc, lockVars, role, taskBook, taskTrans, taskDict,
                   taskParagraph, dedupVars, doubleLockVars>>

\* --- get_or_create_translation: acquire trans lock (first time) ---
\* Models library_book.rs:375-376 — t.lock().await.translation.source_language
\* Family 3: this is the FIRST lock acquisition in the if-condition
TauriListGetTransFirst(t, tr) ==
    /\ pc[t] = "tl_get_trans"
    /\ MutexAvailable(transLock[tr])
    /\ transLock' = [transLock EXCEPT ![tr] = t]
    /\ taskTrans' = [taskTrans EXCEPT ![t] = tr]
    /\ transLockCount' = [transLockCount EXCEPT ![t] = 1]
    /\ pc' = [pc EXCEPT ![t] = "tl_get_trans_check1"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- get_or_create_translation: release first lock (async temporary drop) ---
\* Models library_book.rs:376 — MutexGuard dropped at end of expression
\* Family 3: in the real code, the guard is a temporary and gets dropped
\* before the second .await. We model this as the correct behavior.
TauriListGetTransRelFirst(t) ==
    /\ pc[t] = "tl_get_trans_check1"
    /\ LET tr == taskTrans[t] IN
       transLock' = [transLock EXCEPT ![tr] = "none"]
    /\ transLockCount' = [transLockCount EXCEPT ![t] = 0]
    /\ pc' = [pc EXCEPT ![t] = "tl_get_trans_second"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- get_or_create_translation: acquire trans lock (second time) ---
\* Models library_book.rs:377 — t.lock().await.translation.target_language
\* Family 3: this is the SECOND lock acquisition in the if-condition
TauriListGetTransSecond(t) ==
    /\ pc[t] = "tl_get_trans_second"
    /\ LET tr == taskTrans[t] IN
       /\ MutexAvailable(transLock[tr])
       /\ transLock' = [transLock EXCEPT ![tr] = t]
       /\ transLockCount' = [transLockCount EXCEPT ![t] = 1]
    /\ pc' = [pc EXCEPT ![t] = "tl_get_trans_check2"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Release second lock, ready to iterate paragraphs ---
TauriListGetTransRelSecond(t) ==
    /\ pc[t] = "tl_get_trans_check2"
    /\ LET tr == taskTrans[t] IN
       transLock' = [transLock EXCEPT ![tr] = "none"]
    /\ transLockCount' = [transLockCount EXCEPT ![t] = 0]
    /\ pc' = [pc EXCEPT ![t] = "tl_iter_paragraphs"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Iterate paragraphs: acquire trans lock per paragraph ---
\* Models library_view.rs:199 — book_translation.lock().await
\* Family 1: holds book lock while doing repeated trans lock acquisitions
TauriListAcqTransParagraph(t) ==
    /\ pc[t] = "tl_iter_paragraphs"
    /\ LET tr == taskTrans[t] IN
       /\ MutexAvailable(transLock[tr])
       /\ transLock' = [transLock EXCEPT ![tr] = t]
       /\ transLockCount' = [transLockCount EXCEPT ![t] = 1]
    /\ pc' = [pc EXCEPT ![t] = "tl_read_paragraph"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Release trans lock after reading paragraph ---
\* Models library_view.rs:199-214 — implicit drop at end of loop body
TauriListRelTransParagraph(t) ==
    /\ pc[t] = "tl_read_paragraph"
    /\ LET tr == taskTrans[t] IN
       transLock' = [transLock EXCEPT ![tr] = "none"]
    /\ transLockCount' = [transLockCount EXCEPT ![t] = 0]
    \* Non-deterministically continue iterating or finish
    /\ \/ pc' = [pc EXCEPT ![t] = "tl_iter_paragraphs"]
       \/ pc' = [pc EXCEPT ![t] = "tl_done"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Release book lock, finish ---
TauriListDone(t) ==
    /\ pc[t] = "tl_done"
    /\ LET b == taskBook[t] IN
       bookLock' = [bookLock EXCEPT ![b] = "none"]
    /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
    /\ taskTrans' = [taskTrans EXCEPT ![t] = "none"]
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ role' = [role EXCEPT ![t] = "idle"]
    /\ UNCHANGED <<transLock, dictLock, queueLock,
                   taskDict, taskParagraph,
                   contentionVars, dedupVars, doubleLockVars>>

\* ========================================================================
\* Translator: translate() + handle_request (translation_queue.rs:155-336)
\* ========================================================================
\* Fixed: check + insert are atomic under one lock acquisition.
\* Lock sequence: queueLock (check+insert) → bookLock → transLock (get_or_create)
\*   → release book → network → transLock (add result)

\* --- Begin translator: choose book + paragraph ---
BeginTranslator(t, b, p) ==
    /\ pc[t] = "idle"
    /\ pc' = [pc EXCEPT ![t] = "tr_check_dedup"]
    /\ role' = [role EXCEPT ![t] = "translator"]
    /\ taskBook' = [taskBook EXCEPT ![t] = b]
    /\ taskParagraph' = [taskParagraph EXCEPT ![t] = p]
    /\ requestSent' = [requestSent EXCEPT ![t] = FALSE]
    /\ UNCHANGED <<lockVars, taskTrans, taskDict,
                   contentionVars, requestMap, nextRequestId,
                   doubleLockVars>>

\* --- Check dedup: acquire queueLock, check requestMap ---
\* Models translation_queue.rs:162 — get_request_id(book_id, paragraph_id)
\* which calls state.lock().await then checks paragraph_request_id_map
TranslatorCheckDedup(t) ==
    /\ pc[t] = "tr_check_dedup"
    /\ MutexAvailable(queueLock)
    /\ queueLock' = t
    /\ pc' = [pc EXCEPT ![t] = "tr_check_dedup_read"]
    /\ UNCHANGED <<bookLock, transLock, dictLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars, doubleLockVars>>

\* --- Read requestMap; if miss, insert + allocate ID + release queueLock ---
\* Models translation_queue.rs:162-172 — atomic check+insert under one lock
TranslatorCheckDedupRead(t) ==
    /\ pc[t] = "tr_check_dedup_read"
    /\ queueLock = t
    /\ LET b == taskBook[t]
           p == taskParagraph[t]
           existing == requestMap[<<b, p>>]
       IN
       IF existing /= 0 THEN
            \* Found existing request, abort (return existing ID)
            /\ queueLock' = "none"
            /\ pc' = [pc EXCEPT ![t] = "idle"]
            /\ role' = [role EXCEPT ![t] = "idle"]
            /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
            /\ taskParagraph' = [taskParagraph EXCEPT ![t] = "none"]
            /\ UNCHANGED <<bookLock, transLock, dictLock,
                           taskTrans, taskDict,
                           contentionVars, dedupVars, doubleLockVars>>
       ELSE
            \* No existing request: allocate ID, insert into map, release lock
            /\ queueLock' = "none"
            /\ nextRequestId' = nextRequestId + 1
            /\ requestMap' = [requestMap EXCEPT ![<<b, p>>] = nextRequestId]
            /\ requestSent' = [requestSent EXCEPT ![t] = TRUE]
            /\ pc' = [pc EXCEPT ![t] = "tr_send_request"]
            /\ UNCHANGED <<bookLock, transLock, dictLock, role,
                           taskBook, taskTrans, taskDict, taskParagraph,
                           contentionVars, doubleLockVars>>

\* --- Send request via channel (no lock held, map already updated) ---
\* Models translation_queue.rs:174-183 — send_async after lock released
TranslatorSendRequest(t) ==
    /\ pc[t] = "tr_send_request"
    /\ pc' = [pc EXCEPT ![t] = "tr_acq_book"]
    /\ UNCHANGED <<lockVars, role, taskBook, taskTrans, taskDict,
                   taskParagraph, contentionVars, dedupVars,
                   doubleLockVars>>

\* --- Worker: acquire book lock to get translation ---
\* Models translation_queue.rs:218-219 — library.get_book().lock()
TranslatorAcqBook(t) ==
    /\ pc[t] = "tr_acq_book"
    /\ LET b == taskBook[t] IN
       /\ MutexAvailable(bookLock[b])
       /\ bookLock' = [bookLock EXCEPT ![b] = t]
       /\ waitingForBook' = [waitingForBook EXCEPT ![b] =
            waitingForBook[b] \ {t}]
    /\ pc' = [pc EXCEPT ![t] = "tr_get_trans"]
    /\ UNCHANGED <<transLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   dedupVars, doubleLockVars>>

\* --- Wait for book lock (contention tracking) ---
TranslatorWaitBook(t) ==
    /\ pc[t] = "tr_acq_book"
    /\ LET b == taskBook[t] IN
       /\ ~MutexAvailable(bookLock[b])
       /\ bookLock[b] /= t
       /\ waitingForBook' = [waitingForBook EXCEPT ![b] =
            waitingForBook[b] \cup {t}]
    /\ UNCHANGED <<pc, lockVars, role, taskBook, taskTrans, taskDict,
                   taskParagraph, dedupVars, doubleLockVars>>

\* --- get_or_create_translation while holding book lock ---
\* Models translation_queue.rs:220 → library_book.rs:369-396
\* Simplified: just acquire trans lock briefly, then release
TranslatorGetTrans(t, tr) ==
    /\ pc[t] = "tr_get_trans"
    /\ MutexAvailable(transLock[tr])
    /\ transLock' = [transLock EXCEPT ![tr] = t]
    /\ taskTrans' = [taskTrans EXCEPT ![t] = tr]
    /\ transLockCount' = [transLockCount EXCEPT ![t] = 1]
    /\ pc' = [pc EXCEPT ![t] = "tr_rel_trans_1"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Release trans lock (get_or_create just reads, releases) ---
TranslatorRelTrans1(t) ==
    /\ pc[t] = "tr_rel_trans_1"
    /\ LET tr == taskTrans[t] IN
       transLock' = [transLock EXCEPT ![tr] = "none"]
    /\ transLockCount' = [transLockCount EXCEPT ![t] = 0]
    /\ pc' = [pc EXCEPT ![t] = "tr_rel_book"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Release book lock before network call ---
\* Models translation_queue.rs:217-227 — drop(book) via block scope
TranslatorRelBook(t) ==
    /\ pc[t] = "tr_rel_book"
    /\ LET b == taskBook[t] IN
       bookLock' = [bookLock EXCEPT ![b] = "none"]
    /\ pc' = [pc EXCEPT ![t] = "tr_translate"]
    /\ UNCHANGED <<transLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars, doubleLockVars>>

\* --- Network translation (no locks held) ---
\* Models translation_queue.rs:301-303 — translator.get_translation()
TranslatorDoTranslation(t) ==
    /\ pc[t] = "tr_translate"
    /\ pc' = [pc EXCEPT ![t] = "tr_acq_trans_2"]
    /\ UNCHANGED <<lockVars, role, taskBook, taskTrans, taskDict,
                   taskParagraph, contentionVars, dedupVars,
                   doubleLockVars>>

\* --- Re-acquire trans lock to store result ---
\* Models translation_queue.rs:320-323 — translation.lock().await
TranslatorAcqTrans2(t) ==
    /\ pc[t] = "tr_acq_trans_2"
    /\ LET tr == taskTrans[t] IN
       /\ MutexAvailable(transLock[tr])
       /\ transLock' = [transLock EXCEPT ![tr] = t]
       /\ transLockCount' = [transLockCount EXCEPT ![t] = 1]
    /\ pc' = [pc EXCEPT ![t] = "tr_store_result"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Store result, release trans lock ---
\* Models translation_queue.rs:320-324 — add_paragraph_translation
TranslatorStoreResult(t) ==
    /\ pc[t] = "tr_store_result"
    /\ LET tr == taskTrans[t] IN
       transLock' = [transLock EXCEPT ![tr] = "none"]
    /\ transLockCount' = [transLockCount EXCEPT ![t] = 0]
    /\ pc' = [pc EXCEPT ![t] = "tr_notify_saver"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Notify saver, then worker cleanup: remove from requestMap ---
\* Models translation_queue.rs:139-143 — state.lock().await.remove()
TranslatorWorkerCleanup(t) ==
    /\ pc[t] = "tr_notify_saver"
    /\ MutexAvailable(queueLock)
    /\ queueLock' = t
    /\ LET b == taskBook[t]
           p == taskParagraph[t]
       IN
       requestMap' = [requestMap EXCEPT ![<<b, p>>] = 0]
    /\ pc' = [pc EXCEPT ![t] = "tr_cleanup_done"]
    /\ UNCHANGED <<bookLock, transLock, dictLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, nextRequestId, requestSent,
                   doubleLockVars>>

\* --- Release queueLock, finish ---
TranslatorDone(t) ==
    /\ pc[t] = "tr_cleanup_done"
    /\ queueLock' = "none"
    /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
    /\ taskTrans' = [taskTrans EXCEPT ![t] = "none"]
    /\ taskParagraph' = [taskParagraph EXCEPT ![t] = "none"]
    /\ requestSent' = [requestSent EXCEPT ![t] = FALSE]
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ role' = [role EXCEPT ![t] = "idle"]
    /\ UNCHANGED <<bookLock, transLock, dictLock,
                   taskDict, contentionVars, requestMap,
                   nextRequestId, doubleLockVars>>

\* ========================================================================
\* Saver: save_book (translation_queue.rs:338-422)
\* ========================================================================
\* Lock sequence: bookLock → transLock → dictLock (same as watcher)
\* Reuses the same save() code path as watcher

\* --- Begin saver ---
\* Models translation_queue.rs:418-421 — get_book then lock
BeginSaver(t, b) ==
    /\ pc[t] = "idle"
    /\ pc' = [pc EXCEPT ![t] = "s_acq_book"]
    /\ role' = [role EXCEPT ![t] = "saver"]
    /\ taskBook' = [taskBook EXCEPT ![t] = b]
    /\ UNCHANGED <<lockVars, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars, doubleLockVars>>

\* --- Acquire book lock ---
SaverAcqBook(t) ==
    /\ pc[t] = "s_acq_book"
    /\ LET b == taskBook[t] IN
       /\ MutexAvailable(bookLock[b])
       /\ bookLock' = [bookLock EXCEPT ![b] = t]
       /\ waitingForBook' = [waitingForBook EXCEPT ![b] =
            waitingForBook[b] \ {t}]
    /\ pc' = [pc EXCEPT ![t] = "s_hold_book"]
    /\ UNCHANGED <<transLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   dedupVars, doubleLockVars>>

\* --- Wait for book (contention) ---
SaverWaitBook(t) ==
    /\ pc[t] = "s_acq_book"
    /\ LET b == taskBook[t] IN
       /\ ~MutexAvailable(bookLock[b])
       /\ bookLock[b] /= t
       /\ waitingForBook' = [waitingForBook EXCEPT ![b] =
            waitingForBook[b] \cup {t}]
    /\ UNCHANGED <<pc, lockVars, role, taskBook, taskTrans, taskDict,
                   taskParagraph, dedupVars, doubleLockVars>>

\* --- save(): acquire translation lock ---
SaverAcqTrans(t, tr) ==
    /\ pc[t] = "s_hold_book"
    /\ MutexAvailable(transLock[tr])
    /\ transLock' = [transLock EXCEPT ![tr] = t]
    /\ taskTrans' = [taskTrans EXCEPT ![t] = tr]
    /\ transLockCount' = [transLockCount EXCEPT ![t] = 1]
    /\ pc' = [pc EXCEPT ![t] = "s_hold_trans"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Release translation lock after I/O ---
SaverRelTrans(t) ==
    /\ pc[t] = "s_hold_trans"
    /\ LET tr == taskTrans[t] IN
       transLock' = [transLock EXCEPT ![tr] = "none"]
    /\ transLockCount' = [transLockCount EXCEPT ![t] = 0]
    /\ taskTrans' = [taskTrans EXCEPT ![t] = "none"]
    /\ pc' = [pc EXCEPT ![t] = "s_pre_dict"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Acquire dictionary lock ---
SaverAcqDict(t, d) ==
    /\ pc[t] = "s_pre_dict"
    /\ MutexAvailable(dictLock[d])
    /\ dictLock' = [dictLock EXCEPT ![d] = t]
    /\ taskDict' = [taskDict EXCEPT ![t] = d]
    /\ pc' = [pc EXCEPT ![t] = "s_hold_dict"]
    /\ UNCHANGED <<bookLock, transLock, queueLock, role,
                   taskBook, taskTrans, taskParagraph,
                   contentionVars, dedupVars, doubleLockVars>>

\* --- Release dictionary lock ---
SaverRelDict(t) ==
    /\ pc[t] = "s_hold_dict"
    /\ LET d == taskDict[t] IN
       dictLock' = [dictLock EXCEPT ![d] = "none"]
    /\ taskDict' = [taskDict EXCEPT ![t] = "none"]
    /\ pc' = [pc EXCEPT ![t] = "s_save_book"]
    /\ UNCHANGED <<bookLock, transLock, queueLock, role,
                   taskBook, taskTrans, taskParagraph,
                   contentionVars, dedupVars, doubleLockVars>>

\* --- Save book.dat and release ---
SaverRelBook(t) ==
    /\ pc[t] = "s_save_book"
    /\ LET b == taskBook[t] IN
       bookLock' = [bookLock EXCEPT ![b] = "none"]
    /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ role' = [role EXCEPT ![t] = "idle"]
    /\ UNCHANGED <<transLock, dictLock, queueLock,
                   taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars, doubleLockVars>>

\* ========================================================================
\* Tauri Command: mark_word_visible (library_view.rs:355-377)
\* ========================================================================
\* Lock sequence: bookLock → transLock → (release trans) → save()
\* Demonstrates nested book → trans → save pattern

\* --- Begin mark_word ---
BeginTauriMark(t, b, tr) ==
    /\ pc[t] = "idle"
    /\ pc' = [pc EXCEPT ![t] = "tm_acq_book"]
    /\ role' = [role EXCEPT ![t] = "tauri_mark"]
    /\ taskBook' = [taskBook EXCEPT ![t] = b]
    /\ taskTrans' = [taskTrans EXCEPT ![t] = tr]
    /\ UNCHANGED <<lockVars, taskDict, taskParagraph,
                   contentionVars, dedupVars, doubleLockVars>>

\* --- Acquire book lock ---
\* Models library_view.rs:362-363
TauriMarkAcqBook(t) ==
    /\ pc[t] = "tm_acq_book"
    /\ LET b == taskBook[t] IN
       /\ MutexAvailable(bookLock[b])
       /\ bookLock' = [bookLock EXCEPT ![b] = t]
    /\ pc' = [pc EXCEPT ![t] = "tm_acq_trans"]
    /\ UNCHANGED <<transLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars, doubleLockVars>>

\* --- Acquire trans lock ---
\* Models library_view.rs:367
TauriMarkAcqTrans(t) ==
    /\ pc[t] = "tm_acq_trans"
    /\ LET tr == taskTrans[t] IN
       /\ MutexAvailable(transLock[tr])
       /\ transLock' = [transLock EXCEPT ![tr] = t]
       /\ transLockCount' = [transLockCount EXCEPT ![t] = 1]
    /\ pc' = [pc EXCEPT ![t] = "tm_mark_word"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Mark word, release trans lock ---
\* Models library_view.rs:367-369
TauriMarkRelTrans(t) ==
    /\ pc[t] = "tm_mark_word"
    /\ LET tr == taskTrans[t] IN
       transLock' = [transLock EXCEPT ![tr] = "none"]
    /\ transLockCount' = [transLockCount EXCEPT ![t] = 0]
    \* Proceeds to save() which needs trans+dict locks again (via save path)
    /\ pc' = [pc EXCEPT ![t] = "tm_save"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Save (simplified: just release book lock) ---
\* In a full model, this would expand into the save() sequence.
\* For lock hierarchy, the ordering is the same as watcher/saver.
TauriMarkSave(t) ==
    /\ pc[t] = "tm_save"
    /\ LET b == taskBook[t] IN
       bookLock' = [bookLock EXCEPT ![b] = "none"]
    /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
    /\ taskTrans' = [taskTrans EXCEPT ![t] = "none"]
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ role' = [role EXCEPT ![t] = "idle"]
    /\ UNCHANGED <<transLock, dictLock, queueLock,
                   taskDict, taskParagraph,
                   contentionVars, dedupVars, doubleLockVars>>

\* ========================================================================
\* Fault Injection: Double-Lock Refactoring (Family 3)
\* ========================================================================
\* Models what happens if get_or_create_translation is refactored to
\* hold the guard across both checks (binding to a named variable).
\* This is the fragile pattern at library_book.rs:375-377.

\* --- Refactored get_or_create: acquire trans lock and HOLD it ---
\* Instead of dropping the guard between checks, the refactored version
\* binds it to a variable: `let guard = t.lock().await;`
\* Then tries to acquire again for second check → self-deadlock
RefactoredGetTransHold(t, tr) ==
    /\ pc[t] = "tl_get_trans"
    /\ MutexAvailable(transLock[tr])
    /\ transLock' = [transLock EXCEPT ![tr] = t]
    /\ taskTrans' = [taskTrans EXCEPT ![t] = tr]
    /\ transLockCount' = [transLockCount EXCEPT ![t] = 1]
    \* Go directly to second acquire WITHOUT releasing
    /\ pc' = [pc EXCEPT ![t] = "tl_get_trans_hold_second"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* --- Try to acquire same lock again (WILL DEADLOCK) ---
\* This models the self-deadlock that would occur
RefactoredGetTransSecond(t) ==
    /\ pc[t] = "tl_get_trans_hold_second"
    /\ LET tr == taskTrans[t] IN
       \* Lock is already held by t — this would block forever
       /\ MutexAvailable(transLock[tr])  \* Can never be true since t holds it
       /\ transLock' = [transLock EXCEPT ![tr] = t]
       /\ transLockCount' = [transLockCount EXCEPT ![t] = 2]
    /\ pc' = [pc EXCEPT ![t] = "tl_iter_paragraphs"]
    /\ UNCHANGED <<bookLock, dictLock, queueLock, role,
                   taskBook, taskTrans, taskDict, taskParagraph,
                   contentionVars, dedupVars>>

\* ========================================================================
\* Next State
\* ========================================================================

Next ==
    \E t \in Task :
        \* --- Watcher ---
        \/ \E b \in Book : BeginWatcher(t, b)
        \/ WatcherAcqBook(t)
        \/ WatcherWaitBook(t)
        \/ \E tr \in Translation : WatcherAcqTrans(t, tr)
        \/ WatcherRelTrans(t)
        \/ \E d \in Dictionary : WatcherAcqDict(t, d)
        \/ WatcherRelDict(t)
        \/ WatcherRelBook(t)
        \* --- Tauri list_book_chapter_paragraphs ---
        \/ \E b \in Book : BeginTauriList(t, b)
        \/ TauriListAcqBook(t)
        \/ TauriListWaitBook(t)
        \/ \E tr \in Translation : TauriListGetTransFirst(t, tr)
        \/ TauriListGetTransRelFirst(t)
        \/ TauriListGetTransSecond(t)
        \/ TauriListGetTransRelSecond(t)
        \/ TauriListAcqTransParagraph(t)
        \/ TauriListRelTransParagraph(t)
        \/ TauriListDone(t)
        \* --- Tauri mark_word_visible ---
        \/ \E b \in Book : \E tr \in Translation : BeginTauriMark(t, b, tr)
        \/ TauriMarkAcqBook(t)
        \/ TauriMarkAcqTrans(t)
        \/ TauriMarkRelTrans(t)
        \/ TauriMarkSave(t)
        \* --- Translator ---
        \/ \E b \in Book : \E p \in Paragraph : BeginTranslator(t, b, p)
        \/ TranslatorCheckDedup(t)
        \/ TranslatorCheckDedupRead(t)
        \/ TranslatorSendRequest(t)
        \/ TranslatorAcqBook(t)
        \/ TranslatorWaitBook(t)
        \/ \E tr \in Translation : TranslatorGetTrans(t, tr)
        \/ TranslatorRelTrans1(t)
        \/ TranslatorRelBook(t)
        \/ TranslatorDoTranslation(t)
        \/ TranslatorAcqTrans2(t)
        \/ TranslatorStoreResult(t)
        \/ TranslatorWorkerCleanup(t)
        \/ TranslatorDone(t)
        \* --- Saver ---
        \/ \E b \in Book : BeginSaver(t, b)
        \/ SaverAcqBook(t)
        \/ SaverWaitBook(t)
        \/ \E tr \in Translation : SaverAcqTrans(t, tr)
        \/ SaverRelTrans(t)
        \/ \E d \in Dictionary : SaverAcqDict(t, d)
        \/ SaverRelDict(t)
        \/ SaverRelBook(t)

Spec == Init /\ [][Next]_allVars

\* ========================================================================
\* Invariants
\* ========================================================================

\* --- Standard Safety: No Deadlock ---
\* No state where every non-idle task is blocked waiting for a lock held
\* by another task in the blocked set. (Classical circular wait detection.)
\* A task is "blocked" if its pc indicates it's trying to acquire a lock
\* that is held by another task.
IsBlocked(t) ==
    \/ /\ pc[t] \in {"w_acq_book", "s_acq_book", "tl_acq_book",
                      "tr_acq_book", "tm_acq_book"}
       /\ taskBook[t] /= "none"
       /\ bookLock[taskBook[t]] /= "none"
       /\ bookLock[taskBook[t]] /= t
    \/ /\ pc[t] \in {"tl_get_trans", "tl_get_trans_second",
                      "tl_iter_paragraphs", "tm_acq_trans",
                      "tr_get_trans", "tr_acq_trans_2",
                      "tl_get_trans_hold_second"}
       /\ taskTrans[t] /= "none"
       /\ transLock[taskTrans[t]] /= "none"
       /\ transLock[taskTrans[t]] /= t
    \/ /\ pc[t] \in {"w_pre_dict", "s_pre_dict"}
       /\ taskDict[t] /= "none"
       /\ dictLock[taskDict[t]] /= "none"
       /\ dictLock[taskDict[t]] /= t
    \/ /\ pc[t] \in {"tr_check_dedup", "tr_notify_saver"}
       /\ queueLock /= "none"
       /\ queueLock /= t

WaitsFor(t1, t2) ==
    \/ /\ pc[t1] \in {"w_acq_book", "s_acq_book", "tl_acq_book",
                       "tr_acq_book", "tm_acq_book"}
       /\ taskBook[t1] /= "none"
       /\ bookLock[taskBook[t1]] = t2
    \/ /\ pc[t1] \in {"tl_get_trans", "tl_get_trans_second",
                       "tl_iter_paragraphs", "tm_acq_trans",
                       "tr_get_trans", "tr_acq_trans_2",
                       "tl_get_trans_hold_second"}
       /\ taskTrans[t1] /= "none"
       /\ transLock[taskTrans[t1]] = t2
    \/ /\ pc[t1] \in {"w_pre_dict", "s_pre_dict"}
       /\ taskDict[t1] /= "none"
       /\ dictLock[taskDict[t1]] = t2
    \/ /\ pc[t1] \in {"tr_check_dedup", "tr_notify_saver"}
       /\ queueLock = t2

\* NoDeadlock: No cycle in the wait-for graph among non-idle tasks.
\* For small Task sets, check that no subset of 2+ tasks forms a cycle.
NoDeadlock ==
    ~\E S \in SUBSET Task :
        /\ Cardinality(S) >= 2
        /\ \A t \in S : \E t2 \in S : t /= t2 /\ WaitsFor(t, t2)

\* --- Safety: Lock Order Consistency ---
\* No task holds a lock at level N while trying to acquire a lock at level < N.
\* Level 0 = book, Level 1 = trans, Level 2 = dict
\* Queue lock is independent (level 99).
LockOrderConsistency ==
    \A t \in Task :
        LET maxHeld == MaxLockLevel(t) IN
        \* If trying to acquire book lock (level 0), must not hold trans/dict
        /\ (pc[t] \in {"w_acq_book", "s_acq_book", "tl_acq_book",
                        "tr_acq_book", "tm_acq_book"}
            => maxHeld < 0)
        \* If trying to acquire trans lock (level 1), must not hold dict
        /\ (pc[t] \in {"tl_get_trans", "tl_get_trans_second",
                        "tl_iter_paragraphs", "tm_acq_trans",
                        "tr_get_trans", "tr_acq_trans_2"}
            => maxHeld <= 0)
        \* Dict lock (level 2) can be acquired while holding book (0) or trans (1)
        \* — this is already consistent

\* --- Safety: No Self-Deadlock (Family 3) ---
\* No task attempts to acquire a Mutex it already holds.
NoSelfDeadlock ==
    \A t \in Task :
        \* A task at a trans-acquire PC must not already hold the target lock
        /\ (pc[t] \in {"tl_get_trans_second", "tl_get_trans_hold_second",
                        "tr_acq_trans_2"}
            /\ taskTrans[t] /= "none"
            => transLock[taskTrans[t]] /= t)

\* --- Safety: No Duplicate Active Request (Family 2) ---
\* At most one task should be actively translating the same (book, paragraph)
\* pair at any time. With the fix, the map insert is atomic under the lock,
\* so a second task will see the existing entry and abort.
NoDuplicateActiveRequest ==
    \A b \in Book : \A p \in Paragraph :
        Cardinality({t \in Task :
            /\ role[t] = "translator"
            /\ taskBook[t] = b
            /\ taskParagraph[t] = p
            /\ requestSent[t] = TRUE
            /\ pc[t] \in {"tr_send_request", "tr_acq_book",
                          "tr_get_trans", "tr_rel_trans_1", "tr_rel_book",
                          "tr_translate", "tr_acq_trans_2", "tr_store_result",
                          "tr_notify_saver", "tr_cleanup_done"}
        }) <= 1

\* --- Structural: Mutex Exclusivity ---
\* Each Mutex is held by at most one task.
MutexExclusivity ==
    /\ \A b \in Book :
        Cardinality({t \in Task : bookLock[b] = t}) <= 1
    /\ \A tr \in Translation :
        Cardinality({t \in Task : transLock[tr] = t}) <= 1
    /\ \A d \in Dictionary :
        Cardinality({t \in Task : dictLock[d] = t}) <= 1
    /\ Cardinality({t \in Task : queueLock = t}) <= 1

\* --- Structural: PC Consistency ---
\* Tasks at non-idle PCs must have a valid role assigned.
PCConsistency ==
    \A t \in Task :
        (pc[t] /= "idle") => (role[t] /= "idle")

\* --- Family 1: Bounded Contention ---
\* The number of tasks waiting for any single book lock is bounded.
\* This invariant parameterizes the bound; it's set in MC.cfg.
CONSTANT MaxBookContention
BoundedContention ==
    \A b \in Book : Cardinality(waitingForBook[b]) <= MaxBookContention

====
