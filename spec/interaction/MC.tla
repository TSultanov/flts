---- MODULE MC ----
(***************************************************************************)
(* Model checking wrapper for FLTS Frontend–Backend Command/Event spec.    *)
(* Counter-bounds fault-injection actions (ConfigChange, AppClose).        *)
(* Reactive actions (worker/tauri/watcher lifecycle, DeliverEvent,         *)
(* MarkWordVisible) are NOT bounded.                                       *)
(***************************************************************************)

EXTENDS base

\* ========================================================================
\* Constants (counter limits and state space bounds)
\* ========================================================================

CONSTANTS
    ConfigChangeLimit,  \* Max number of config reconfigurations (F1 fault injection)
    AppCloseLimit,      \* Max number of AppClose events (0 or 1)
    MaxTruthVersion     \* State constraint: cap truthVersion to bound state space

\* ========================================================================
\* Counter-bounded fault injection
\* ========================================================================

VARIABLES
    configChangeCount,  \* Number of ConfigChange actions fired
    appCloseCount       \* Number of AppClose actions fired

mcVars == <<configChangeCount, appCloseCount>>

\* --- Bounded ConfigChange (F1: stale library reference) ---
MCConfigChange ==
    /\ configChangeCount < ConfigChangeLimit
    /\ ConfigChange
    /\ configChangeCount' = configChangeCount + 1
    /\ UNCHANGED appCloseCount

\* --- Bounded AppClose (F3: no shutdown persistence) ---
MCAppClose ==
    /\ appCloseCount < AppCloseLimit
    /\ AppClose
    /\ appCloseCount' = appCloseCount + 1
    /\ UNCHANGED configChangeCount

\* ========================================================================
\* Unconstrained reactive actions (pass-through with UNCHANGED mcVars)
\* ========================================================================

\* --- Worker lifecycle ---
MCBeginWorker(t, b) == BeginWorker(t, b) /\ UNCHANGED mcVars
MCWorkerReadParagraph(t) == WorkerReadParagraph(t) /\ UNCHANGED mcVars
MCWorkerCallAPI(t) == WorkerCallAPI(t) /\ UNCHANGED mcVars
MCWorkerStoreResult(t) == WorkerStoreResult(t) /\ UNCHANGED mcVars
MCWorkerSave(t) == WorkerSave(t) /\ UNCHANGED mcVars
MCWorkerEmit(t) == WorkerEmit(t) /\ UNCHANGED mcVars

\* --- Tauri command lifecycle ---
MCBeginTauri(t, b) == BeginTauri(t, b) /\ UNCHANGED mcVars
MCTauriModify(t) == TauriModify(t) /\ UNCHANGED mcVars
MCTauriEmit(t) == TauriEmit(t) /\ UNCHANGED mcVars

\* --- File watcher lifecycle ---
MCBeginWatcher(t, b) == BeginWatcher(t, b) /\ UNCHANGED mcVars
MCWatcherReload(t) == WatcherReload(t) /\ UNCHANGED mcVars
MCWatcherEmit(t) == WatcherEmit(t) /\ UNCHANGED mcVars

\* --- Event delivery ---
MCDeliverEvent == DeliverEvent /\ UNCHANGED mcVars

\* --- Mark word visible ---
MCMarkWordVisible(b) == MarkWordVisible(b) /\ UNCHANGED mcVars

\* ========================================================================
\* Init and Next
\* ========================================================================

MCInit ==
    /\ Init
    /\ configChangeCount = 0
    /\ appCloseCount = 0

MCNext ==
    \* --- Fault injection (bounded) ---
    \/ MCConfigChange
    \/ MCAppClose
    \* --- Worker lifecycle ---
    \/ \E t \in Task, b \in Book :
        MCBeginWorker(t, b)
    \/ \E t \in Task :
        \/ MCWorkerReadParagraph(t)
        \/ MCWorkerCallAPI(t)
        \/ MCWorkerStoreResult(t)
        \/ MCWorkerSave(t)
        \/ MCWorkerEmit(t)
    \* --- Tauri command lifecycle ---
    \/ \E t \in Task, b \in Book :
        MCBeginTauri(t, b)
    \/ \E t \in Task :
        \/ MCTauriModify(t)
        \/ MCTauriEmit(t)
    \* --- File watcher lifecycle ---
    \/ \E t \in Task, b \in Book :
        MCBeginWatcher(t, b)
    \/ \E t \in Task :
        \/ MCWatcherReload(t)
        \/ MCWatcherEmit(t)
    \* --- Event delivery ---
    \/ MCDeliverEvent
    \* --- Mark word visible ---
    \/ \E b \in Book : MCMarkWordVisible(b)

MCSpec == MCInit /\ [][MCNext]_<<allVars, mcVars>>

\* ========================================================================
\* State constraint (prune state space)
\* ========================================================================

StateConstraint ==
    /\ truthVersion <= MaxTruthVersion
    /\ pendingEvents <= MaxTruthVersion
    /\ \A b \in Book : memVersion[b] <= MaxTruthVersion

\* ========================================================================
\* Structural Invariants (always checked)
\* ========================================================================

MCPCConsistency == PCConsistency
MCTaskLibraryValidity == TaskLibraryValidity

\* ========================================================================
\* Safety Invariants
\* ========================================================================

MCStaleLibrarySafety == StaleLibrarySafety
MCNoDataLoss == NoDataLoss
MCEventMonotonicity == EventMonotonicity
MCUIConsistency == UIConsistency
MCNoPersistenceLoss == NoPersistenceLoss

\* F4: Action property — must use PROPERTY not INVARIANT in config.
\* NoStaleTranslation is already a temporal formula ([][...]_allVars).
\* We re-express it over allVars + mcVars so stuttering steps from MC
\* counter changes are also covered.
MCNoStaleTranslation ==
    [][\A t \in Task :
        (pc[t] = "w_store" /\ pc'[t] = "w_save") =>
            taskReadVersion[t] = bookVersion[taskBook[t]]]_<<allVars, mcVars>>

====
