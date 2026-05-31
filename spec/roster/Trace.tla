--------------------------- MODULE Trace ---------------------------
(*
 * Trace-validation wrapper for the roster-mesh spec (vector-clock model).
 *
 * Replays an NDJSON trace (one event per membership transition, emitted per the
 * sibling instrumentation-spec.md) against base.tla. Each event carries the
 * emitting node's POST-state: per device its add/remove vector clocks, plus its
 * engine peer set. The wrappers fire the matching base action for that node and
 * assert the node's resulting roster/engine match.
 *
 * STATUS: Phase-3 scaffold, exercised by harness/roster/run.sh traces. `gAdd` /
 * `gRem` are modeling-only ground truth with no code counterpart, so they advance
 * but are NOT validated (the causal invariants stay MC-only). `Node`/`MaxClock`
 * in Trace.cfg come from the harness's device ids / op counts.
 *)

EXTENDS base, Json, IOUtils, Sequences, TLC

\* ============================================================================
\* TRACE LOADING
\* ============================================================================

JsonFile ==
    IF "JSON" \in DOMAIN IOEnv THEN IOEnv.JSON
    ELSE "../../traces/roster.ndjson"

TraceLog ==
    TLCEval(
        LET all == ndJsonDeserialize(JsonFile)
        IN SelectSeq(all, LAMBDA x :
            /\ "tag" \in DOMAIN x
            /\ x.tag = "trace"
            /\ "event" \in DOMAIN x))

ASSUME Len(TraceLog) > 0

VARIABLE l
traceVars == <<l>>
logline == TraceLog[l]

\* ============================================================================
\* EXPECTED POST-STATE FROM THE EVENT
\* ============================================================================

EvNode   == logline.event.node
EvRoster == logline.event.roster        \* record: deviceId -> [add |-> vc, rem |-> vc]
EvEngine == { logline.event.engine[i] : i \in DOMAIN logline.event.engine }

\* A sparse JSON vclock `{deviceId: counter}` as a total VC (missing = 0).
ToVC(m) == [k \in Node |-> IF k \in DOMAIN m THEN m[k] ELSE 0]

\* The node's roster and engine after the action must match the event: every
\* device's add/rem clocks, devices absent from the event are EmptyEntry, and the
\* peer set is exact.
ValidateNode(n) ==
    /\ \A d \in DOMAIN EvRoster :
          /\ roster'[n][d].add = ToVC(EvRoster[d].add)
          /\ roster'[n][d].rem = ToVC(EvRoster[d].rem)
    /\ \A d \in Node \ DOMAIN EvRoster : roster'[n][d] = EmptyEntry
    /\ engine'[n] = EvEngine

IsEvent(name) ==
    /\ l <= Len(TraceLog)
    /\ logline.event.name = name

StepTrace == l' = l + 1

\* ============================================================================
\* ACTION WRAPPERS
\* ============================================================================

PairOnIfLogged ==
    /\ IsEvent("PairOn")
    /\ PairOn(EvNode, logline.event.target)
    /\ ValidateNode(EvNode)
    /\ StepTrace

UnpairOnIfLogged ==
    /\ IsEvent("UnpairOn")
    /\ UnpairOn(EvNode, logline.event.target)
    /\ ValidateNode(EvNode)
    /\ StepTrace

ApprovePendingIfLogged ==
    /\ IsEvent("ApprovePending")
    /\ ApprovePending(EvNode, logline.event.target)
    /\ ValidateNode(EvNode)
    /\ StepTrace

EnsureSelfIfLogged ==
    /\ IsEvent("EnsureSelf")
    /\ EnsureSelf(EvNode)
    /\ ValidateNode(EvNode)
    /\ StepTrace

RosterSyncIfLogged ==
    /\ IsEvent("RosterSync")
    /\ RosterSync(logline.event.src, EvNode)
    /\ ValidateNode(EvNode)
    /\ StepTrace

ReconcileNodeIfLogged ==
    /\ IsEvent("ReconcileNode")
    /\ ReconcileNode(EvNode)
    /\ ValidateNode(EvNode)
    /\ StepTrace

\* ============================================================================
\* INIT / NEXT
\* ============================================================================

TraceInit ==
    /\ Init
    /\ l = 1

TraceNext ==
    \/ PairOnIfLogged
    \/ UnpairOnIfLogged
    \/ ApprovePendingIfLogged
    \/ EnsureSelfIfLogged
    \/ RosterSyncIfLogged
    \/ ReconcileNodeIfLogged
    \/ /\ l > Len(TraceLog)
       /\ UNCHANGED <<vars, l>>

TraceSpec == TraceInit /\ [][TraceNext]_<<vars, l>> /\ WF_<<vars, l>>(TraceNext)

TraceView == <<vars, l>>

\* Liveness goal: the whole trace was consumed (every event matched a transition).
TraceMatched == <>(l > Len(TraceLog))

=============================================================================
