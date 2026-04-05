---- MODULE Trace ----
(***************************************************************************)
(* Trace validation spec for FLTS Frontend–Backend Command/Event Protocol  *)
(* (Category B: concurrent, not distributed).                              *)
(*                                                                         *)
(* Works with NDJSON events emitted from Tauri backend instrumentation:    *)
(*   {"event":"BeginWorker","task":"t1","book":"b1","lib":1,...}           *)
(*   {"event":"WorkerSave","task":"t1","lib":1,...}                        *)
(*   {"event":"ConfigChange","newLib":2,...}                               *)
(*   {"event":"Emit","task":"t1","snapshot":3,...}                         *)
(*   {"event":"DeliverEvent","version":3,...}                              *)
(*                                                                         *)
(* Category B: per-task cursors + ViablePIDs for partial-order replay.     *)
(***************************************************************************)

EXTENDS Sequences, FiniteSets, Integers, Json, IOUtils, TLC

\* ========================================================================
\* Constants
\* ========================================================================

CONSTANTS
    Task,       \* e.g. {t1, t2, t3}
    Book        \* e.g. {b1}

ASSUME Task /= {}
ASSUME Book /= {}

\* ========================================================================
\* Variables
\* ========================================================================

\* --- Spec state (same as base.tla, minus non-observable) ---
VARIABLES
    currentLib,
    taskLib,
    pc,
    taskType,
    taskBook,
    pendingEvents,
    taskSnapshot,
    truthVersion,
    uiVersion,
    maxDeliveredVersion,
    bookVersion,
    taskReadVersion,
    memVersion,
    diskVersion,
    appAlive,
    cursor          \* [Task ∪ {"ui"} -> Nat] — per-task/frontend event index

\* Variable group aliases (matching base.tla for UNCHANGED clauses)
libVars == <<currentLib, taskLib>>
taskVars == <<pc, taskType, taskBook>>
eventVars == <<pendingEvents, taskSnapshot, truthVersion,
               uiVersion, maxDeliveredVersion>>
versionVars == <<bookVersion, taskReadVersion>>
persistVars == <<memVersion, diskVersion, appAlive>>

specVars == <<currentLib, taskLib, pc, taskType, taskBook,
              pendingEvents, taskSnapshot, truthVersion,
              uiVersion, maxDeliveredVersion,
              bookVersion, taskReadVersion,
              memVersion, diskVersion, appAlive>>

vars == <<specVars, cursor>>

\* ========================================================================
\* Trace Loading
\* ========================================================================

\* Override with: JSON=path/to/trace.json java -cp ...
JsonFile ==
    IF "JSON" \in DOMAIN IOEnv THEN IOEnv.JSON
    ELSE "../../traces/interaction_trace.json"

RawTraces == JsonDeserialize(JsonFile)

\* Traces are keyed by task name + a special "ui" key for frontend events.
AllActors == Task \union {"ui"}

traces == [a \in AllActors |->
    IF ToString(a) \in DOMAIN RawTraces
    THEN RawTraces[ToString(a)]
    ELSE << >>]

\* ========================================================================
\* Helpers
\* ========================================================================

ActorsWithEvents ==
    { a \in AllActors : cursor[a] <= Len(traces[a]) }

Logline(a) == traces[a][cursor[a]]

\* ViablePIDs: partial-order constraint for Category B.
\* An actor can step iff no other actor has a pending event that completed
\* before this actor's next event started.
ViablePIDs ==
    { a \in ActorsWithEvents :
        ~ \E a2 \in ActorsWithEvents :
            /\ a2 /= a
            /\ traces[a2][cursor[a2]].end < traces[a][cursor[a]].start }

AdvanceCursor(a) == cursor' = [cursor EXCEPT ![a] = cursor[a] + 1]

\* Resolve a JSON string to its model value constant.
TaskOf(name) == CHOOSE t \in Task : ToString(t) = name
BookOf(name) == CHOOSE b \in Book : ToString(b) = name
IsKnownTask(name) == \E t \in Task : ToString(t) = name
IsKnownBook(name) == \E b \in Book : ToString(b) = name

Max(a, b) == IF a >= b THEN a ELSE b

\* ========================================================================
\* Event Actions
\* ========================================================================

\* --- ConfigChange ---
TraceConfigChange(a) ==
    /\ a \in Task  \* ConfigChange is logged on the thread that triggers it
    /\ Logline(a).event = "ConfigChange"
    /\ appAlive
    /\ currentLib' = Logline(a).newLib
    /\ truthVersion' = truthVersion + 1
    /\ pendingEvents' = Append(pendingEvents, truthVersion + 1)
    /\ UNCHANGED <<taskLib, pc, taskType, taskBook,
                   taskSnapshot, uiVersion, maxDeliveredVersion,
                   bookVersion, taskReadVersion,
                   memVersion, diskVersion, appAlive>>
    /\ AdvanceCursor(a)

\* --- BeginWorker ---
TraceBeginWorker(a) ==
    /\ a \in Task
    /\ Logline(a).event = "BeginWorker"
    /\ LET t == TaskOf(Logline(a).task)
           b == BookOf(Logline(a).book) IN
       /\ appAlive
       /\ pc[t] = "idle"
       /\ pc' = [pc EXCEPT ![t] = "w_read"]
       /\ taskType' = [taskType EXCEPT ![t] = "worker"]
       /\ taskBook' = [taskBook EXCEPT ![t] = b]
       /\ taskLib' = [taskLib EXCEPT ![t] = Logline(a).lib]
    /\ UNCHANGED <<currentLib, eventVars, versionVars, persistVars>>
    /\ AdvanceCursor(a)

\* --- WorkerReadParagraph ---
TraceWorkerReadParagraph(a) ==
    /\ a \in Task
    /\ Logline(a).event = "WorkerReadParagraph"
    /\ LET t == TaskOf(Logline(a).task)
           b == taskBook[t] IN
       /\ appAlive
       /\ pc[t] = "w_read"
       /\ taskReadVersion' = [taskReadVersion EXCEPT ![t] = bookVersion[b]]
       /\ memVersion' = [memVersion EXCEPT ![b] = memVersion[b] + 1]
       /\ pc' = [pc EXCEPT ![t] = "w_api"]
    /\ UNCHANGED <<libVars, taskType, taskBook, eventVars,
                   bookVersion, diskVersion, appAlive>>
    /\ AdvanceCursor(a)

\* --- WorkerCallAPI ---
TraceWorkerCallAPI(a) ==
    /\ a \in Task
    /\ Logline(a).event = "WorkerCallAPI"
    /\ LET t == TaskOf(Logline(a).task) IN
       /\ appAlive
       /\ pc[t] = "w_api"
       /\ pc' = [pc EXCEPT ![t] = "w_store"]
    /\ UNCHANGED <<libVars, taskType, taskBook, eventVars,
                   versionVars, persistVars>>
    /\ AdvanceCursor(a)

\* --- WorkerStoreResult ---
TraceWorkerStoreResult(a) ==
    /\ a \in Task
    /\ Logline(a).event = "WorkerStoreResult"
    /\ LET t == TaskOf(Logline(a).task) IN
       /\ appAlive
       /\ pc[t] = "w_store"
       /\ truthVersion' = truthVersion + 1
       /\ pc' = [pc EXCEPT ![t] = "w_save"]
    /\ UNCHANGED <<libVars, taskType, taskBook,
                   pendingEvents, taskSnapshot, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>
    /\ AdvanceCursor(a)

\* --- WorkerSave ---
TraceWorkerSave(a) ==
    /\ a \in Task
    /\ Logline(a).event = "WorkerSave"
    /\ LET t == TaskOf(Logline(a).task)
           b == taskBook[t] IN
       /\ appAlive
       /\ pc[t] = "w_save"
       /\ IF taskLib[t] = currentLib
          THEN diskVersion' = [diskVersion EXCEPT ![b] = memVersion[b]]
          ELSE UNCHANGED diskVersion
       /\ pc' = [pc EXCEPT ![t] = "w_snapshot"]
    /\ UNCHANGED <<libVars, taskType, taskBook, eventVars,
                   versionVars, memVersion, appAlive>>
    /\ AdvanceCursor(a)

\* --- WorkerComputeSnapshot ---
TraceWorkerComputeSnapshot(a) ==
    /\ a \in Task
    /\ Logline(a).event = "WorkerComputeSnapshot"
    /\ LET t == TaskOf(Logline(a).task) IN
       /\ appAlive
       /\ pc[t] = "w_snapshot"
       /\ taskSnapshot' = [taskSnapshot EXCEPT ![t] = truthVersion]
       /\ pc' = [pc EXCEPT ![t] = "w_emit"]
    /\ UNCHANGED <<libVars, taskType, taskBook,
                   pendingEvents, truthVersion, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>
    /\ AdvanceCursor(a)

\* --- WorkerEmit ---
TraceWorkerEmit(a) ==
    /\ a \in Task
    /\ Logline(a).event = "WorkerEmit"
    /\ LET t == TaskOf(Logline(a).task) IN
       /\ appAlive
       /\ pc[t] = "w_emit"
       /\ pendingEvents' = Append(pendingEvents, taskSnapshot[t])
       /\ pc' = [pc EXCEPT ![t] = "idle"]
       /\ taskType' = [taskType EXCEPT ![t] = "idle"]
       /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
       /\ taskSnapshot' = [taskSnapshot EXCEPT ![t] = 0]
    /\ UNCHANGED <<libVars, truthVersion, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>
    /\ AdvanceCursor(a)

\* --- BeginTauri ---
TraceBeginTauri(a) ==
    /\ a \in Task
    /\ Logline(a).event = "BeginTauri"
    /\ LET t == TaskOf(Logline(a).task)
           b == BookOf(Logline(a).book) IN
       /\ appAlive
       /\ pc[t] = "idle"
       /\ pc' = [pc EXCEPT ![t] = "tc_modify"]
       /\ taskType' = [taskType EXCEPT ![t] = "tauri"]
       /\ taskBook' = [taskBook EXCEPT ![t] = b]
       /\ taskLib' = [taskLib EXCEPT ![t] = currentLib]
    /\ UNCHANGED <<currentLib, eventVars, versionVars, persistVars>>
    /\ AdvanceCursor(a)

\* --- TauriModify ---
TraceTauriModify(a) ==
    /\ a \in Task
    /\ Logline(a).event = "TauriModify"
    /\ LET t == TaskOf(Logline(a).task)
           b == taskBook[t] IN
       /\ appAlive
       /\ pc[t] = "tc_modify"
       /\ truthVersion' = truthVersion + 1
       /\ memVersion' = [memVersion EXCEPT ![b] = memVersion[b] + 1]
       /\ diskVersion' = [diskVersion EXCEPT ![b] = memVersion[b] + 1]
       /\ pc' = [pc EXCEPT ![t] = "tc_snapshot"]
    /\ UNCHANGED <<libVars, taskType, taskBook,
                   pendingEvents, taskSnapshot, uiVersion, maxDeliveredVersion,
                   versionVars, appAlive>>
    /\ AdvanceCursor(a)

\* --- TauriComputeSnapshot ---
TraceTauriComputeSnapshot(a) ==
    /\ a \in Task
    /\ Logline(a).event = "TauriComputeSnapshot"
    /\ LET t == TaskOf(Logline(a).task) IN
       /\ appAlive
       /\ pc[t] = "tc_snapshot"
       /\ taskSnapshot' = [taskSnapshot EXCEPT ![t] = truthVersion]
       /\ pc' = [pc EXCEPT ![t] = "tc_emit"]
    /\ UNCHANGED <<libVars, taskType, taskBook,
                   pendingEvents, truthVersion, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>
    /\ AdvanceCursor(a)

\* --- TauriEmit ---
TraceTauriEmit(a) ==
    /\ a \in Task
    /\ Logline(a).event = "TauriEmit"
    /\ LET t == TaskOf(Logline(a).task) IN
       /\ appAlive
       /\ pc[t] = "tc_emit"
       /\ pendingEvents' = Append(pendingEvents, taskSnapshot[t])
       /\ pc' = [pc EXCEPT ![t] = "idle"]
       /\ taskType' = [taskType EXCEPT ![t] = "idle"]
       /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
       /\ taskSnapshot' = [taskSnapshot EXCEPT ![t] = 0]
    /\ UNCHANGED <<libVars, truthVersion, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>
    /\ AdvanceCursor(a)

\* --- BeginWatcher ---
TraceBeginWatcher(a) ==
    /\ a \in Task
    /\ Logline(a).event = "BeginWatcher"
    /\ LET t == TaskOf(Logline(a).task)
           b == BookOf(Logline(a).book) IN
       /\ appAlive
       /\ pc[t] = "idle"
       /\ pc' = [pc EXCEPT ![t] = "fw_reload"]
       /\ taskType' = [taskType EXCEPT ![t] = "watcher"]
       /\ taskBook' = [taskBook EXCEPT ![t] = b]
       /\ taskLib' = [taskLib EXCEPT ![t] = currentLib]
    /\ UNCHANGED <<currentLib, eventVars, versionVars, persistVars>>
    /\ AdvanceCursor(a)

\* --- WatcherReload ---
TraceWatcherReload(a) ==
    /\ a \in Task
    /\ Logline(a).event = "WatcherReload"
    /\ LET t == TaskOf(Logline(a).task)
           b == taskBook[t] IN
       /\ appAlive
       /\ pc[t] = "fw_reload"
       /\ bookVersion' = [bookVersion EXCEPT ![b] = bookVersion[b] + 1]
       /\ truthVersion' = truthVersion + 1
       /\ diskVersion' = [diskVersion EXCEPT ![b] = memVersion[b]]
       /\ pc' = [pc EXCEPT ![t] = "fw_snapshot"]
    /\ UNCHANGED <<libVars, taskType, taskBook,
                   pendingEvents, taskSnapshot, uiVersion, maxDeliveredVersion,
                   taskReadVersion, memVersion, appAlive>>
    /\ AdvanceCursor(a)

\* --- WatcherComputeSnapshot ---
TraceWatcherComputeSnapshot(a) ==
    /\ a \in Task
    /\ Logline(a).event = "WatcherComputeSnapshot"
    /\ LET t == TaskOf(Logline(a).task) IN
       /\ appAlive
       /\ pc[t] = "fw_snapshot"
       /\ taskSnapshot' = [taskSnapshot EXCEPT ![t] = truthVersion]
       /\ pc' = [pc EXCEPT ![t] = "fw_emit"]
    /\ UNCHANGED <<libVars, taskType, taskBook,
                   pendingEvents, truthVersion, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>
    /\ AdvanceCursor(a)

\* --- WatcherEmit ---
TraceWatcherEmit(a) ==
    /\ a \in Task
    /\ Logline(a).event = "WatcherEmit"
    /\ LET t == TaskOf(Logline(a).task) IN
       /\ appAlive
       /\ pc[t] = "fw_emit"
       /\ pendingEvents' = Append(pendingEvents, taskSnapshot[t])
       /\ pc' = [pc EXCEPT ![t] = "idle"]
       /\ taskType' = [taskType EXCEPT ![t] = "idle"]
       /\ taskBook' = [taskBook EXCEPT ![t] = "none"]
       /\ taskSnapshot' = [taskSnapshot EXCEPT ![t] = 0]
    /\ UNCHANGED <<libVars, truthVersion, uiVersion, maxDeliveredVersion,
                   versionVars, persistVars>>
    /\ AdvanceCursor(a)

\* --- DeliverEvent (UI actor) ---
TraceDeliverEvent(a) ==
    /\ a = "ui"
    /\ Logline(a).event = "DeliverEvent"
    /\ pendingEvents /= <<>>
    /\ LET v == Head(pendingEvents) IN
       /\ uiVersion' = v
       /\ maxDeliveredVersion' = Max(maxDeliveredVersion, v)
       /\ pendingEvents' = Tail(pendingEvents)
    /\ UNCHANGED <<libVars, taskVars, taskSnapshot, truthVersion,
                   versionVars, persistVars>>
    /\ AdvanceCursor(a)

\* --- MarkWordVisible ---
TraceMarkWordVisible(a) ==
    /\ a \in Task
    /\ Logline(a).event = "MarkWordVisible"
    /\ appAlive
    /\ LET b == BookOf(Logline(a).book) IN
       /\ memVersion' = [memVersion EXCEPT ![b] = memVersion[b] + 1]
       /\ diskVersion' = [diskVersion EXCEPT ![b] = memVersion[b] + 1]
    /\ UNCHANGED <<libVars, taskVars, eventVars, versionVars, appAlive>>
    /\ AdvanceCursor(a)

\* --- AppClose ---
TraceAppClose(a) ==
    /\ a \in Task
    /\ Logline(a).event = "AppClose"
    /\ appAlive
    /\ appAlive' = FALSE
    /\ UNCHANGED <<libVars, taskVars, eventVars, versionVars,
                   memVersion, diskVersion>>
    /\ AdvanceCursor(a)

\* ========================================================================
\* Event Dispatch
\* ========================================================================

MatchEvent(a) ==
    LET e == Logline(a).event IN
    CASE e = "ConfigChange"            -> TraceConfigChange(a)
      [] e = "BeginWorker"             -> TraceBeginWorker(a)
      [] e = "WorkerReadParagraph"     -> TraceWorkerReadParagraph(a)
      [] e = "WorkerCallAPI"           -> TraceWorkerCallAPI(a)
      [] e = "WorkerStoreResult"       -> TraceWorkerStoreResult(a)
      [] e = "WorkerSave"              -> TraceWorkerSave(a)
      [] e = "WorkerComputeSnapshot"   -> TraceWorkerComputeSnapshot(a)
      [] e = "WorkerEmit"              -> TraceWorkerEmit(a)
      [] e = "BeginTauri"              -> TraceBeginTauri(a)
      [] e = "TauriModify"             -> TraceTauriModify(a)
      [] e = "TauriComputeSnapshot"    -> TraceTauriComputeSnapshot(a)
      [] e = "TauriEmit"               -> TraceTauriEmit(a)
      [] e = "BeginWatcher"            -> TraceBeginWatcher(a)
      [] e = "WatcherReload"           -> TraceWatcherReload(a)
      [] e = "WatcherComputeSnapshot"  -> TraceWatcherComputeSnapshot(a)
      [] e = "WatcherEmit"             -> TraceWatcherEmit(a)
      [] e = "DeliverEvent"            -> TraceDeliverEvent(a)
      [] e = "MarkWordVisible"         -> TraceMarkWordVisible(a)
      [] e = "AppClose"                -> TraceAppClose(a)

\* ========================================================================
\* Init, Next, Spec
\* ========================================================================

TraceInit ==
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
    /\ cursor = [a \in AllActors |-> 1]

TraceNext ==
    \/ /\ ActorsWithEvents /= {}
       /\ \E a \in ViablePIDs :
            MatchEvent(a)
    \/ /\ ActorsWithEvents = {}
       /\ UNCHANGED vars

TraceSpec == TraceInit /\ [][TraceNext]_vars /\ WF_vars(TraceNext)

\* ========================================================================
\* Safety Invariants (checked during trace validation)
\* ========================================================================

\* Structural
TracePCConsistency ==
    \A t \in Task : (pc[t] /= "idle") => (taskType[t] /= "idle")

\* F2: Event monotonicity
TraceEventMonotonicity ==
    uiVersion >= maxDeliveredVersion

\* ========================================================================
\* Trace completion — checked as temporal property
\* ========================================================================

TraceFullyConsumed == <>(ActorsWithEvents = {})

====
