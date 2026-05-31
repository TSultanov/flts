--------------------------- MODULE base ---------------------------
(*
 * TLA+ specification for the FLTS Syncthing **roster-mesh** membership protocol,
 * after the F1 fix: membership is a vector-clock CRDT (remove-wins), not
 * wall-clock last-writer-wins.
 *
 * SCOPE: the app-managed device mesh layered on top of Syncthing — brief
 * Families 1-3 of ../modeling-brief.md. The file-merge layer (Family 4) is a
 * separate spec (../base.tla); the save/watch echo gate (Family 5) is not here.
 *
 * Derived from:
 *   - library/src/sync/roster.rs    (VClock, AddStamp/RemStamp, is_present, merge)
 *   - library/src/sync/reconcile.rs (reconcile: present devices vs engine set)
 *   - library/src/sync/engine.rs    (pair/unpair/reconcile_once, reshare)
 *
 * MODEL. Each device d carries two vector clocks per node's roster: an add
 * context `add` and a remove context `rem` (functions Node -> 0..MaxClock; the
 * all-zero clock `Bottom` means "nothing seen"). A local op at node n stamps the
 * relevant context with n's causal context advanced by one in n's own component
 * (roster.rs `next_vc`). RosterSync joins the two contexts pointwise (the merge).
 * A device is **present iff its add context strictly dominates its remove
 * context** (roster.rs `is_present`): remove-wins, because a causally-later or
 * concurrent removal carries a component the adds cannot cover.
 *
 * GROUND TRUTH. `gAdd[d]` / `gRem[d]` accumulate (pointwise join) the context of
 * every add / remove op ever issued for d — used by the safety invariant to ask
 * "did a removal causally cover all adds?" without trusting the spec's own merge.
 *
 * Every node IS a device. `engine[n]` is the set of PEER devices n shares with
 * (excludes self); two nodes replicate iff they are mutual (engine.rs reshare).
 *)

EXTENDS Naturals, FiniteSets, TLC

CONSTANTS
    Node,        \* set of mesh devices (each is also a running app node)
    MaxClock     \* per-component bound on a vector clock (logical, not wall time)

\* ============================================================================
\* VECTOR CLOCKS
\* ============================================================================

VC == [Node -> 0..MaxClock]            \* a vector clock (canonical: 0 = unknown)
Bottom == [n \in Node |-> 0]

VcDominates(a, b) == \A n \in Node : a[n] >= b[n]
VcStrictlyDominates(a, b) == a # b /\ VcDominates(a, b)
VcJoin(a, b) == [n \in Node |-> IF a[n] >= b[n] THEN a[n] ELSE b[n]]

\* ============================================================================
\* VARIABLES
\* ============================================================================

\* Per node, per device: the add and remove contexts (the CRDT state).
\* roster[n][d] = [add |-> VC, rem |-> VC].
VARIABLE roster

\* Each node's Syncthing peer set (excludes self).
VARIABLE engine

\* Ground-truth global join of every add / remove context issued, per device.
VARIABLE gAdd
VARIABLE gRem

vars == <<roster, engine, gAdd, gRem>>

\* ============================================================================
\* HELPERS
\* ============================================================================

Entry == [add : VC, rem : VC]
RosterType == [Node -> Entry]

\* A device is present (a member) iff its add context strictly dominates its
\* remove context. roster.rs is_present.
Present(n, d) == VcStrictlyDominates(roster[n][d].add, roster[n][d].rem)

\* Tombstoned: not present but a removal has been seen (so reconcile may drop it).
Tombstoned(n, d) == ~Present(n, d) /\ roster[n][d].rem # Bottom

\* This node's causal context = join of every add/rem context it holds. Computed
\* component-wise as the max over all device contexts (always includes 0, so the
\* set is non-empty).
Context(n) ==
    [k \in Node |->
        LET vals == {0} \cup {roster[n][d].add[k] : d \in Node}
                        \cup {roster[n][d].rem[k] : d \in Node}
        IN CHOOSE mx \in vals : \A x \in vals : mx >= x]

\* The clock to stamp on n's next op: its context, advanced by one in component n.
NextVc(n) == [Context(n) EXCEPT ![n] = @ + 1]

\* Two nodes replicate the library folder iff each has added the other.
Mutual(a, b) == b \in engine[a] /\ a \in engine[b]

\* reconcile (reconcile.rs): add present roster devices missing from the engine;
\* remove engine devices the roster tombstones. Never self.
ReconcilePlan(n) ==
    [add    |-> { d \in Node : d # n /\ Present(n, d) /\ d \notin engine[n] },
     remove |-> { d \in engine[n] : d # n /\ Tombstoned(n, d) }]

\* ============================================================================
\* INIT
\* ============================================================================

EmptyEntry == [add |-> Bottom, rem |-> Bottom]

Init ==
    /\ roster = [n \in Node |-> [d \in Node |-> EmptyEntry]]
    /\ engine = [n \in Node |-> {}]
    /\ gAdd = [d \in Node |-> Bottom]
    /\ gRem = [d \in Node |-> Bottom]

\* ============================================================================
\* ACTIONS
\* ============================================================================

\* Stamp an add of device `target` issued by node `n`. Used by PairOn / Approve /
\* EnsureSelf — the new add context dominates anything n had observed for target,
\* so it wins (re-add works). engine gains the peer (except for EnsureSelf=self).
DoAdd(n, target, touchesEngine) ==
    /\ Context(n)[n] < MaxClock                  \* clock headroom (bounded model)
    /\ LET v == NextVc(n) IN
       /\ roster' = [roster EXCEPT ![n][target].add = v]
       /\ gAdd' = [gAdd EXCEPT ![target] = VcJoin(@, v)]
       /\ IF touchesEngine
            THEN engine' = [engine EXCEPT ![n] = @ \cup {target}]
            ELSE UNCHANGED engine
    /\ UNCHANGED gRem

\* PairOn: node n pairs peer m (engine.rs pair_device).
PairOn(n, m) == n # m /\ DoAdd(n, m, TRUE)

\* ApprovePending: the second side of a first pairing (sync.rs approve a pending
\* device). Same effect as PairOn, gated on the peer already connecting to us.
ApprovePending(n, p) ==
    /\ p # n
    /\ n \in engine[p]
    /\ p \notin engine[n]
    /\ DoAdd(n, p, TRUE)

\* EnsureSelf: set_device_name re-adds self (engine.rs set_device_name). Touches
\* the roster only.
EnsureSelf(n) == DoAdd(n, n, FALSE)

\* UnpairOn: node n removes peer m (engine.rs unpair_device). The remove context
\* dominates the add it observed → remove-wins.
UnpairOn(n, m) ==
    /\ n # m
    /\ Context(n)[n] < MaxClock
    /\ LET v == NextVc(n) IN
       /\ roster' = [roster EXCEPT ![n][m].rem = v]
       /\ gRem' = [gRem EXCEPT ![m] = VcJoin(@, v)]
    /\ engine' = [engine EXCEPT ![n] = @ \ {m}]
    /\ UNCHANGED gAdd

\* RosterSync: Syncthing replicates src's roster to dst, which union-merges it by
\* joining both contexts pointwise per device (roster.rs merge). Only between
\* mutual peers, and only when it changes dst's roster.
RosterSync(src, dst) ==
    /\ src # dst
    /\ Mutual(src, dst)
    /\ LET merged == [d \in Node |->
                        [add |-> VcJoin(roster[dst][d].add, roster[src][d].add),
                         rem |-> VcJoin(roster[dst][d].rem, roster[src][d].rem)]]
       IN /\ merged # roster[dst]
          /\ roster' = [roster EXCEPT ![dst] = merged]
    /\ UNCHANGED <<engine, gAdd, gRem>>

\* ReconcileNode: bring the engine peer set in line with the merged roster
\* (engine.rs reconcile_once). Roster untouched; only when the plan changes engine.
ReconcileNode(n) ==
    /\ LET plan == ReconcilePlan(n)
           next == (engine[n] \cup plan.add) \ plan.remove
       IN /\ next # engine[n]
          /\ engine' = [engine EXCEPT ![n] = next]
    /\ UNCHANGED <<roster, gAdd, gRem>>

Next ==
    \/ \E n, m \in Node : PairOn(n, m)
    \/ \E n, m \in Node : UnpairOn(n, m)
    \/ \E n, p \in Node : ApprovePending(n, p)
    \/ \E n \in Node    : EnsureSelf(n)
    \/ \E src, dst \in Node : RosterSync(src, dst)
    \/ \E n \in Node    : ReconcileNode(n)

Spec == Init /\ [][Next]_vars

\* ============================================================================
\* INVARIANTS
\* ============================================================================

TypeOK ==
    /\ roster \in [Node -> RosterType]
    /\ engine \in [Node -> SUBSET Node]
    /\ gAdd \in [Node -> VC]
    /\ gRem \in [Node -> VC]

NoSelfPeer == \A n \in Node : n \notin engine[n]

\* ---- Convergence gate ------------------------------------------------------
FullyMerged ==
    \A s, d \in Node : (s # d /\ Mutual(s, d)) =>
        [k \in Node |->
            [add |-> VcJoin(roster[d][k].add, roster[s][k].add),
             rem |-> VcJoin(roster[d][k].rem, roster[s][k].rem)]] = roster[d]

AllConnected == \A a, b \in Node : a # b => Mutual(a, b)
Converged == FullyMerged /\ AllConnected

PresentAnywhere(d) == \E n \in Node : Present(n, d)

\* ---- Family 1: convergent + causally correct (the F1 fix) ------------------
\* At a closed, fully-merged mesh, all nodes hold an identical roster.
ConvergenceAgreement ==
    Converged => \A a, b \in Node : roster[a] = roster[b]

\* At a closed, fully-merged mesh, a device whose removals causally cover all of
\* its adds must NOT be present anywhere. Wall-clock LWW violated this (a skewed
\* or equal-timestamp removal was lost); causal remove-wins must honor it.
NoSpuriousResurrection ==
    Converged =>
        \A d \in Node : VcDominates(gRem[d], gAdd[d]) /\ gRem[d] # Bottom
                          => ~PresentAnywhere(d)

\* ---- Family 2: reconcile never tears down a present device -----------------
ReconcileNeverDropsActive ==
    \A n \in Node : \A d \in ReconcilePlan(n).remove : ~Present(n, d)

\* ---- Family 3: mesh closure (safety surrogate) -----------------------------
InMesh(n) == \E m \in Node : m # n /\ (m \in engine[n] \/ n \in engine[m])
ReconcileSettled ==
    \A n \in Node : ReconcilePlan(n).add = {} /\ ReconcilePlan(n).remove = {}
PendingApprovalExists ==
    \E n, p \in Node : n # p /\ n \in engine[p] /\ p \notin engine[n]
Settled == FullyMerged /\ ReconcileSettled /\ ~PendingApprovalExists
MeshClosesWhenSettled ==
    Settled =>
        \A a, b \in Node : (a # b /\ InMesh(a) /\ InMesh(b)) => Mutual(a, b)

=============================================================================
