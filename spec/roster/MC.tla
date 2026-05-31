--------------------------- MODULE MC ---------------------------
(*
 * Model-checking wrapper for the FLTS roster-mesh spec.
 *
 * Bounded (introduce nondeterminism — user/membership operations):
 *   - PairOn, UnpairOn, ApprovePending, EnsureSelf
 *
 * Unbounded (reactive — converge existing state):
 *   - RosterSync   (Syncthing replication + conflict-sibling merge)
 *   - ReconcileNode (the 10s reconcile poller)
 *
 * Fairness (used only by the Family-3 liveness property MeshConverges; it does
 * not affect safety-invariant checking) forces the reactive loops, and pairing
 * progress, to make progress so a single mesh eventually closes.
 *)

EXTENDS base

rm == INSTANCE base

\* ============================================================================
\* COUNTER CONSTANTS / VARIABLES
\* ============================================================================

CONSTANTS
    MaxPairLimit,
    MaxUnpairLimit,
    MaxApproveLimit,
    MaxEnsureSelfLimit

VARIABLE faultCounters
faultVars == <<faultCounters>>

CounterType ==
    [pair       : 0..MaxPairLimit,
     unpair     : 0..MaxUnpairLimit,
     approve    : 0..MaxApproveLimit,
     ensureSelf : 0..MaxEnsureSelfLimit]

mcvars == <<roster, engine, gAdd, gRem, faultCounters>>

\* ============================================================================
\* BOUNDED MEMBERSHIP ACTIONS
\* ============================================================================

MCPairOn(n, m) ==
    /\ faultCounters.pair < MaxPairLimit
    /\ rm!PairOn(n, m)
    /\ faultCounters' = [faultCounters EXCEPT !.pair = @ + 1]

MCUnpairOn(n, m) ==
    /\ faultCounters.unpair < MaxUnpairLimit
    /\ rm!UnpairOn(n, m)
    /\ faultCounters' = [faultCounters EXCEPT !.unpair = @ + 1]

MCApprovePending(n, p) ==
    /\ faultCounters.approve < MaxApproveLimit
    /\ rm!ApprovePending(n, p)
    /\ faultCounters' = [faultCounters EXCEPT !.approve = @ + 1]

MCEnsureSelf(n) ==
    /\ faultCounters.ensureSelf < MaxEnsureSelfLimit
    /\ rm!EnsureSelf(n)
    /\ faultCounters' = [faultCounters EXCEPT !.ensureSelf = @ + 1]

\* ============================================================================
\* UNBOUNDED REACTIVE ACTIONS
\* ============================================================================

MCRosterSync(src, dst) ==
    /\ rm!RosterSync(src, dst)
    /\ UNCHANGED faultVars

MCReconcileNode(n) ==
    /\ rm!ReconcileNode(n)
    /\ UNCHANGED faultVars

\* ============================================================================
\* INIT / NEXT
\* ============================================================================

MCInit ==
    /\ Init
    /\ faultCounters = [pair |-> 0, unpair |-> 0, approve |-> 0, ensureSelf |-> 0]

PairStep    == \E n, m \in Node : MCPairOn(n, m)
UnpairStep  == \E n, m \in Node : MCUnpairOn(n, m)
ApproveStep == \E n, p \in Node : MCApprovePending(n, p)
EnsureStep  == \E n \in Node    : MCEnsureSelf(n)
SyncStep    == \E src, dst \in Node : MCRosterSync(src, dst)
ReconStep   == \E n \in Node    : MCReconcileNode(n)

MCNext ==
    \/ PairStep
    \/ UnpairStep
    \/ ApproveStep
    \/ EnsureStep
    \/ SyncStep
    \/ ReconStep

\* Force the auto-mesh loops (and continued pairing progress) so MeshConverges is
\* a meaningful liveness check. Irrelevant to safety invariants.
Fairness ==
    /\ WF_mcvars(SyncStep)
    /\ WF_mcvars(ReconStep)
    /\ WF_mcvars(ApproveStep)

MCSpec == MCInit /\ [][MCNext]_mcvars /\ Fairness

\* ============================================================================
\* SYMMETRY / VIEW
\* ============================================================================

Symmetry == Permutations(Node)

\* Exclude the fault counters from the fingerprint so counter values don't blow up
\* the state graph.
View == <<roster, engine, gAdd, gRem>>

\* ============================================================================
\* FAMILY 3 — MESH CLOSURE (liveness)
\* ============================================================================

\* From the pairings performed, the mesh should eventually become fully connected
\* WITHOUT every pair being manually approved: roster sync + reconcile must close
\* it. Checked only in MC_hunt_f3.cfg (needs Fairness).
MeshConverges == <>AllConnected

=============================================================================
