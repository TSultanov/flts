---- MODULE Trace ----
(***************************************************************************)
(* Trace validation spec for FLTS Mutex / Lock Safety (Category B).        *)
(*                                                                         *)
(* Works with named-lock events from the TracedMutex wrapper:              *)
(*   {"event": "Acq", "lock": "b1", "start": N, "end": N, ...}           *)
(*   {"event": "Rel", "lock": "b1", "start": N, "end": N, ...}           *)
(*                                                                         *)
(* Lock names are mapped by the preprocessor:                              *)
(*   "book:<uuid>"     -> b1, b2, ...  (member of BookLock)                *)
(*   "trans:eng_fra"   -> tr1, tr2, ... (member of TransLock)              *)
(*   "dict:eng_fra"    -> d1, d2, ...   (member of DictLock)               *)
(*                                                                         *)
(* Category B: per-task cursors + ViablePIDs for partial-order replay.     *)
(* Lock hierarchy ordering is enforced by the Acq action precondition.     *)
(***************************************************************************)

EXTENDS Sequences, FiniteSets, Integers, Json, IOUtils, TLC

\* ========================================================================
\* Constants
\* ========================================================================

CONSTANTS
    Task,           \* e.g. {t1, t2, t3}
    BookLock,       \* e.g. {b1}  — book mutexes
    TransLock,      \* e.g. {tr1} — translation mutexes
    DictLock        \* e.g. {d1}  — dictionary mutexes

Lock == BookLock \union TransLock \union DictLock

\* Lock hierarchy: book (0) < trans (1) < dict (2)
Level(l) ==
    CASE l \in BookLock  -> 0
      [] l \in TransLock -> 1
      [] l \in DictLock  -> 2

ASSUME Task /= {}

\* ========================================================================
\* Variables
\* ========================================================================

VARIABLES
    lockHolder,     \* [Lock -> Task ∪ {"none"}]
    locksHeld,      \* [Task -> SUBSET Lock]
    cursor          \* [Task -> Nat] — per-task event index (1-based)

vars == <<lockHolder, locksHeld, cursor>>

\* ========================================================================
\* Trace Loading
\* ========================================================================

\* Override with: java ... -DJSON=path/to/trace.json
JsonFile ==
    IF "JSON" \in DOMAIN IOEnv THEN IOEnv.JSON
    ELSE "../../traces/mutex_trace.json"

RawTraces == JsonDeserialize(JsonFile)

traces == [t \in Task |->
    IF ToString(t) \in DOMAIN RawTraces
    THEN RawTraces[ToString(t)]
    ELSE << >>]

\* ========================================================================
\* Helpers
\* ========================================================================

ThreadsWithEvents ==
    { t \in Task : cursor[t] <= Len(traces[t]) }

Logline(t) == traces[t][cursor[t]]

\* ViablePIDs: partial-order constraint.
\* A task can step iff no other task has a pending event that completed
\* (end timestamp) before this task's next event started.
ViablePIDs ==
    { t \in ThreadsWithEvents :
        ~ \E t2 \in ThreadsWithEvents :
            /\ t2 /= t
            /\ traces[t2][cursor[t2]].end < traces[t][cursor[t]].start }

AdvanceCursor(t) == cursor' = [cursor EXCEPT ![t] = cursor[t] + 1]

\* Resolve a JSON string lock name to its model value constant.
LockOf(name) == CHOOSE l \in Lock : ToString(l) = name
IsKnownLock(name) == \E l \in Lock : ToString(l) = name

\* Highest lock level currently held by a task (-1 if none held).
MaxHeldLevel(t) ==
    IF locksHeld[t] = {} THEN -1
    ELSE LET levels == { Level(l) : l \in locksHeld[t] }
         IN CHOOSE mx \in levels : \A lv \in levels : lv <= mx

\* ========================================================================
\* Event Actions
\* ========================================================================

AcqLock(t) ==
    /\ Logline(t).event = "Acq"
    /\ IsKnownLock(Logline(t).lock)
    /\ LET l == LockOf(Logline(t).lock) IN
       /\ lockHolder[l] = "none"
       \* Lock hierarchy: new lock level must be >= max level already held.
       \* Violation here means the trace shows an out-of-order acquisition,
       \* causing TLC to report the trace cannot be fully consumed.
       /\ Level(l) >= MaxHeldLevel(t)
       /\ lockHolder' = [lockHolder EXCEPT ![l] = t]
       /\ locksHeld' = [locksHeld EXCEPT ![t] = locksHeld[t] \union {l}]
    /\ AdvanceCursor(t)

RelLock(t) ==
    /\ Logline(t).event = "Rel"
    /\ IsKnownLock(Logline(t).lock)
    /\ LET l == LockOf(Logline(t).lock) IN
       /\ lockHolder[l] = t
       /\ lockHolder' = [lockHolder EXCEPT ![l] = "none"]
       /\ locksHeld' = [locksHeld EXCEPT ![t] = locksHeld[t] \ {l}]
    /\ AdvanceCursor(t)

\* ========================================================================
\* Event Dispatch
\* ========================================================================

MatchEvent(t) ==
    LET e == Logline(t).event IN
    CASE e = "Acq" -> AcqLock(t)
      [] e = "Rel" -> RelLock(t)

\* ========================================================================
\* Init, Next, Spec
\* ========================================================================

TraceInit ==
    /\ lockHolder = [l \in Lock |-> "none"]
    /\ locksHeld = [t \in Task |-> {}]
    /\ cursor = [t \in Task |-> 1]

TraceNext ==
    \/ /\ ThreadsWithEvents /= {}
       /\ \E t \in ViablePIDs :
            MatchEvent(t)
    \/ /\ ThreadsWithEvents = {}
       /\ UNCHANGED vars

TraceSpec == TraceInit /\ [][TraceNext]_vars /\ WF_vars(TraceNext)

\* ========================================================================
\* Safety Invariants
\* ========================================================================

\* Each lock is held by at most one task at any time.
MutexExclusivity ==
    \A l \in Lock :
        Cardinality({t \in Task : lockHolder[l] = t}) <= 1

\* ========================================================================
\* Trace completion — checked as temporal property
\* ========================================================================

\* The full trace must be consumed. If TLC reports this property violated
\* or a deadlock, the trace is inconsistent with the lock safety rules.
TraceFullyConsumed == <>(ThreadsWithEvents = {})

====
