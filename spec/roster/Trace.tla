--------------------------- MODULE Trace ---------------------------
(*
 * Trace-validation wrapper for the roster-mesh spec.
 *
 * Replays an NDJSON trace (one event per membership transition, emitted per the
 * sibling instrumentation-spec.md) against base.tla. Each event carries the
 * emitting node's POST-state: its roster (active: dev->addedAtMs, tomb:
 * dev->removedAtMs) and engine peer set. The wrappers fire the matching base
 * action for that node and assert the node's resulting roster/engine match.
 *
 * SCOPE / STATUS: this is the Phase-3 scaffold. `gseq` / `lastOp` are
 * modeling-only ground truth with no code counterpart, so they advance but are
 * NOT validated (the causal invariants stay MC-only — see brief-coverage.md).
 * Wall-clock ms are mapped to a dense rank (DenseTs) so timestamps stay in
 * 0..MaxClock. It must be exercised against real harness traces in
 * harness-generation + validation-workflow; `Node` and `MaxClock` in Trace.cfg
 * are set from the harness's actual device ids / distinct-timestamp count, and
 * silent actions may need to be added for state the harness does not log.
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
\* DENSE TIMESTAMP MAPPING (real ms -> small rank in 0..MaxClock)
\* ============================================================================

EventTsValues(ev) ==
    LET act == ev.roster.active
        tmb == ev.roster.tomb
    IN  ({ act[d] : d \in DOMAIN act } \cup { tmb[d] : d \in DOMAIN tmb }
         \cup {ev.ts}) \ {0}

ObservedTs ==
    UNION { EventTsValues(TraceLog[i].event) : i \in 1..Len(TraceLog) }

DenseTs(t) ==
    IF t = 0 THEN 0
    ELSE Cardinality({ x \in ObservedTs : x <= t })

\* ============================================================================
\* EXPECTED POST-STATE FROM THE EVENT
\* ============================================================================

EvNode   == logline.event.node
EvActive == logline.event.roster.active          \* record: deviceId -> addedAtMs
EvTomb   == logline.event.roster.tomb            \* record: deviceId -> removedAtMs
EvEngine == { logline.event.engine[i] : i \in DOMAIN logline.event.engine }

\* The node's roster and engine after the action must match the event exactly:
\* same active / tombstoned device sets, dense-ranked timestamps, and peer set.
ValidateNode(n) ==
    /\ { d \in Node : Active(roster'[n], d) }     = DOMAIN EvActive
    /\ { d \in Node : Tombstoned(roster'[n], d) } = DOMAIN EvTomb
    /\ \A d \in DOMAIN EvActive : roster'[n].active[d].ts = DenseTs(EvActive[d])
    /\ \A d \in DOMAIN EvTomb   : roster'[n].tomb[d].ts   = DenseTs(EvTomb[d])
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
    /\ PairOn(EvNode, logline.event.target, DenseTs(logline.event.ts))
    /\ ValidateNode(EvNode)
    /\ StepTrace

UnpairOnIfLogged ==
    /\ IsEvent("UnpairOn")
    /\ UnpairOn(EvNode, logline.event.target, DenseTs(logline.event.ts))
    /\ ValidateNode(EvNode)
    /\ StepTrace

ApprovePendingIfLogged ==
    /\ IsEvent("ApprovePending")
    /\ ApprovePending(EvNode, logline.event.target, DenseTs(logline.event.ts))
    /\ ValidateNode(EvNode)
    /\ StepTrace

EnsureSelfIfLogged ==
    /\ IsEvent("EnsureSelf")
    /\ EnsureSelf(EvNode, DenseTs(logline.event.ts))
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
