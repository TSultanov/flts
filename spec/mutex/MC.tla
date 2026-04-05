---- MODULE MC ----
(***************************************************************************)
(* Model checking wrapper for FLTS Mutex / Lock Safety specification.      *)
(* Counter-bounds fault-injection actions (RefactoredGetTrans for F3).     *)
(* Reactive actions (normal lock acquire/release) are NOT bounded.         *)
(***************************************************************************)

EXTENDS base

\* ========================================================================
\* Constants (counter limits and state space bounds)
\* ========================================================================

CONSTANTS
    RefactorLimit,      \* Max number of refactored double-lock events (Family 3)
    MaxTranslatorOps    \* Max total translator operations (state constraint)

\* ========================================================================
\* Counter-bounded fault injection
\* ========================================================================

VARIABLES
    refactorCount       \* Number of RefactoredGetTrans actions fired

mcVars == <<refactorCount>>

\* --- Bounded RefactoredGetTrans (Family 3: fragile double-lock) ---
\* Injects the refactored version of get_or_create_translation where
\* the guard is held across both checks (library_book.rs:375-377)
MCRefactoredGetTransHold(t, tr) ==
    /\ refactorCount < RefactorLimit
    /\ RefactoredGetTransHold(t, tr)
    /\ refactorCount' = refactorCount + 1

MCRefactoredGetTransSecond(t) ==
    /\ RefactoredGetTransSecond(t)
    /\ UNCHANGED mcVars

\* ========================================================================
\* Unconstrained reactive actions (pass-through with UNCHANGED mcVars)
\* ========================================================================

\* --- Watcher ---
MCBeginWatcher(t, b) == BeginWatcher(t, b) /\ UNCHANGED mcVars
MCWatcherAcqBook(t) == WatcherAcqBook(t) /\ UNCHANGED mcVars
MCWatcherWaitBook(t) == WatcherWaitBook(t) /\ UNCHANGED mcVars
MCWatcherAcqTrans(t, tr) == WatcherAcqTrans(t, tr) /\ UNCHANGED mcVars
MCWatcherRelTrans(t) == WatcherRelTrans(t) /\ UNCHANGED mcVars
MCWatcherAcqDict(t, d) == WatcherAcqDict(t, d) /\ UNCHANGED mcVars
MCWatcherRelDict(t) == WatcherRelDict(t) /\ UNCHANGED mcVars
MCWatcherRelBook(t) == WatcherRelBook(t) /\ UNCHANGED mcVars

\* --- Tauri list ---
MCBeginTauriList(t, b) == BeginTauriList(t, b) /\ UNCHANGED mcVars
MCTauriListAcqBook(t) == TauriListAcqBook(t) /\ UNCHANGED mcVars
MCTauriListWaitBook(t) == TauriListWaitBook(t) /\ UNCHANGED mcVars
MCTauriListGetTransFirst(t, tr) == TauriListGetTransFirst(t, tr) /\ UNCHANGED mcVars
MCTauriListGetTransRelFirst(t) == TauriListGetTransRelFirst(t) /\ UNCHANGED mcVars
MCTauriListGetTransSecond(t) == TauriListGetTransSecond(t) /\ UNCHANGED mcVars
MCTauriListGetTransRelSecond(t) == TauriListGetTransRelSecond(t) /\ UNCHANGED mcVars
MCTauriListAcqTransParagraph(t) == TauriListAcqTransParagraph(t) /\ UNCHANGED mcVars
MCTauriListRelTransParagraph(t) == TauriListRelTransParagraph(t) /\ UNCHANGED mcVars
MCTauriListDone(t) == TauriListDone(t) /\ UNCHANGED mcVars

\* --- Tauri mark ---
MCBeginTauriMark(t, b, tr) == BeginTauriMark(t, b, tr) /\ UNCHANGED mcVars
MCTauriMarkAcqBook(t) == TauriMarkAcqBook(t) /\ UNCHANGED mcVars
MCTauriMarkAcqTrans(t) == TauriMarkAcqTrans(t) /\ UNCHANGED mcVars
MCTauriMarkRelTrans(t) == TauriMarkRelTrans(t) /\ UNCHANGED mcVars
MCTauriMarkSave(t) == TauriMarkSave(t) /\ UNCHANGED mcVars

\* --- Translator ---
MCBeginTranslator(t, b, p) == BeginTranslator(t, b, p) /\ UNCHANGED mcVars
MCTranslatorCheckDedup(t) == TranslatorCheckDedup(t) /\ UNCHANGED mcVars
MCTranslatorCheckDedupRead(t) == TranslatorCheckDedupRead(t) /\ UNCHANGED mcVars
MCTranslatorSendRequest(t) == TranslatorSendRequest(t) /\ UNCHANGED mcVars
MCTranslatorAcqBook(t) == TranslatorAcqBook(t) /\ UNCHANGED mcVars
MCTranslatorWaitBook(t) == TranslatorWaitBook(t) /\ UNCHANGED mcVars
MCTranslatorGetTrans(t, tr) == TranslatorGetTrans(t, tr) /\ UNCHANGED mcVars
MCTranslatorRelTrans1(t) == TranslatorRelTrans1(t) /\ UNCHANGED mcVars
MCTranslatorRelBook(t) == TranslatorRelBook(t) /\ UNCHANGED mcVars
MCTranslatorDoTranslation(t) == TranslatorDoTranslation(t) /\ UNCHANGED mcVars
MCTranslatorAcqTrans2(t) == TranslatorAcqTrans2(t) /\ UNCHANGED mcVars
MCTranslatorStoreResult(t) == TranslatorStoreResult(t) /\ UNCHANGED mcVars
MCTranslatorWorkerCleanup(t) == TranslatorWorkerCleanup(t) /\ UNCHANGED mcVars
MCTranslatorDone(t) == TranslatorDone(t) /\ UNCHANGED mcVars

\* --- Saver ---
MCBeginSaver(t, b) == BeginSaver(t, b) /\ UNCHANGED mcVars
MCSaverAcqBook(t) == SaverAcqBook(t) /\ UNCHANGED mcVars
MCSaverWaitBook(t) == SaverWaitBook(t) /\ UNCHANGED mcVars
MCSaverAcqTrans(t, tr) == SaverAcqTrans(t, tr) /\ UNCHANGED mcVars
MCSaverRelTrans(t) == SaverRelTrans(t) /\ UNCHANGED mcVars
MCSaverAcqDict(t, d) == SaverAcqDict(t, d) /\ UNCHANGED mcVars
MCSaverRelDict(t) == SaverRelDict(t) /\ UNCHANGED mcVars
MCSaverRelBook(t) == SaverRelBook(t) /\ UNCHANGED mcVars

\* ========================================================================
\* Init and Next
\* ========================================================================

MCInit ==
    /\ Init
    /\ refactorCount = 0

MCNext ==
    \E t \in Task :
        \* --- Watcher ---
        \/ \E b \in Book : MCBeginWatcher(t, b)
        \/ MCWatcherAcqBook(t)
        \/ MCWatcherWaitBook(t)
        \/ \E tr \in Translation : MCWatcherAcqTrans(t, tr)
        \/ MCWatcherRelTrans(t)
        \/ \E d \in Dictionary : MCWatcherAcqDict(t, d)
        \/ MCWatcherRelDict(t)
        \/ MCWatcherRelBook(t)
        \* --- Tauri list ---
        \/ \E b \in Book : MCBeginTauriList(t, b)
        \/ MCTauriListAcqBook(t)
        \/ MCTauriListWaitBook(t)
        \/ \E tr \in Translation : MCTauriListGetTransFirst(t, tr)
        \/ MCTauriListGetTransRelFirst(t)
        \/ MCTauriListGetTransSecond(t)
        \/ MCTauriListGetTransRelSecond(t)
        \/ MCTauriListAcqTransParagraph(t)
        \/ MCTauriListRelTransParagraph(t)
        \/ MCTauriListDone(t)
        \* --- Tauri mark ---
        \/ \E b \in Book : \E tr \in Translation : MCBeginTauriMark(t, b, tr)
        \/ MCTauriMarkAcqBook(t)
        \/ MCTauriMarkAcqTrans(t)
        \/ MCTauriMarkRelTrans(t)
        \/ MCTauriMarkSave(t)
        \* --- Translator ---
        \/ \E b \in Book : \E p \in Paragraph : MCBeginTranslator(t, b, p)
        \/ MCTranslatorCheckDedup(t)
        \/ MCTranslatorCheckDedupRead(t)
        \/ MCTranslatorSendRequest(t)
        \/ MCTranslatorAcqBook(t)
        \/ MCTranslatorWaitBook(t)
        \/ \E tr \in Translation : MCTranslatorGetTrans(t, tr)
        \/ MCTranslatorRelTrans1(t)
        \/ MCTranslatorRelBook(t)
        \/ MCTranslatorDoTranslation(t)
        \/ MCTranslatorAcqTrans2(t)
        \/ MCTranslatorStoreResult(t)
        \/ MCTranslatorWorkerCleanup(t)
        \/ MCTranslatorDone(t)
        \* --- Saver ---
        \/ \E b \in Book : MCBeginSaver(t, b)
        \/ MCSaverAcqBook(t)
        \/ MCSaverWaitBook(t)
        \/ \E tr \in Translation : MCSaverAcqTrans(t, tr)
        \/ MCSaverRelTrans(t)
        \/ \E d \in Dictionary : MCSaverAcqDict(t, d)
        \/ MCSaverRelDict(t)
        \/ MCSaverRelBook(t)
        \* --- Fault injection (Family 3) ---
        \/ \E tr \in Translation : MCRefactoredGetTransHold(t, tr)
        \/ MCRefactoredGetTransSecond(t)

MCSpec == MCInit /\ [][MCNext]_<<allVars, mcVars>>

\* ========================================================================
\* State constraint (prune state space)
\* ========================================================================

StateConstraint ==
    /\ nextRequestId <= MaxTranslatorOps

\* ========================================================================
\* Structural Invariants (always checked)
\* ========================================================================

MCMutexExclusivity == MutexExclusivity
MCPCConsistency == PCConsistency

\* ========================================================================
\* Safety Invariants
\* ========================================================================

MCNoDeadlock == NoDeadlock
MCLockOrderConsistency == LockOrderConsistency
MCNoSelfDeadlock == NoSelfDeadlock

\* ========================================================================
\* Extension Invariants (bug-family specific, commented out in MC.cfg)
\* ========================================================================

MCNoDuplicateActiveRequest == NoDuplicateActiveRequest
MCBoundedContention == BoundedContention

====
