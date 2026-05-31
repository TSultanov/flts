--------------------------- MODULE base ---------------------------
(*
 * TLA+ specification for the FLTS Syncthing **roster-mesh** membership protocol.
 *
 * SCOPE: the app-managed device mesh layered on top of Syncthing — brief
 * Families 1-3 of ../modeling-brief.md. The file-merge layer (Family 4) is a
 * separate spec (../base.tla); the save/watch echo gate (Family 5) is not modeled
 * here.
 *
 * Derived from:
 *   - library/src/sync/roster.rs      (Roster::merge, RosterStore add/remove/load)
 *   - library/src/sync/reconcile.rs   (reconcile: roster vs engine device set)
 *   - library/src/sync/engine.rs      (pair/unpair/reconcile_once, reshare)
 *   - site/src-tauri/src/app/sync_daemon.rs (10s reconcile poller)
 *
 * Bug families covered:
 *   1. Roster CRDT last-writer-wins under per-device wall clocks (clock skew can
 *      make the merge disagree with causal order → device resurrection).
 *   2. Reconcile asymmetry: add-on-presence vs remove-only-on-tombstone; reconcile
 *      must never drop a device that is active in the merged roster.
 *   3. Mesh propagation: one pairing should fan out to a full mesh across the
 *      independent roster-sync + reconcile loops.
 *
 * KEY MODELING DEVICE — ground-truth causal order. Each membership operation is
 * stamped with BOTH a wall-clock `ts` (in 0..MaxClock, chosen freely to model
 * arbitrary cross-device skew; roster.rs:209 `now_ms`) AND a global sequence
 * number `seq` (monotone, ground truth). The merge (Roster::merge) compares ONLY
 * `ts`, exactly like the code. The invariants compare `seq` to know what the
 * causally-latest operation really was. When skew makes `ts` disagree with `seq`,
 * the merge can keep a removed device active — the open question behind brief M1/M3.
 *
 * Every node IS a device; the set of running app nodes and the set of device ids
 * coincide (Node). `engine[n]` is the set of PEER devices n has added to its
 * Syncthing config (excludes self); the folder is shared with `engine[n] \cup {n}`
 * (engine.rs:138-157 reshare_library), so two nodes replicate iff they are mutual.
 *)

EXTENDS Naturals, FiniteSets, TLC

CONSTANTS
    Node,        \* set of mesh devices (each is also a running app node)
    MaxClock,    \* upper bound on a wall-clock timestamp (skew domain: 0..MaxClock)
    Nil          \* sentinel model value for "absent from the active/tomb map"

\* ============================================================================
\* VARIABLES
\* ============================================================================

\* Each node's local roster file content, AFTER any conflict-sibling merges it has
\* performed. A roster has an `active` map (device -> add Entry) and a `tomb` map
\* (device -> removal Entry); Nil means "absent from that map" (roster.rs:34-42).
VARIABLE roster

\* Each node's Syncthing device config = the PEER ids it has added (engine.rs
\* add_device/remove_device via add_peer/remove_peer). Excludes self.
VARIABLE engine

\* Ground-truth global operation counter. Every membership op increments it; it is
\* the causal order the invariants trust (NOT visible to the merge).
VARIABLE gseq

\* Ground-truth latest operation per device: [seq, kind] with kind in
\* {"none","add","remove"}. Updated by every Pair/Unpair/Approve/EnsureSelf.
VARIABLE lastOp

vars == <<roster, engine, gseq, lastOp>>

\* ============================================================================
\* TYPES AND HELPERS
\* ============================================================================

Entry == [ts : 0..MaxClock, seq : Nat]
EntryOrNil == Entry \cup {Nil}

RosterType == [active : [Node -> EntryOrNil], tomb : [Node -> EntryOrNil]]

\* Strict-ish total order on entries: newer ts wins; ties broken by seq (ground
\* truth). seq is globally unique, so this is a strict total order over distinct
\* entries — CHOOSE-max is well defined. (Rust's max_by_key breaks ts-ties by
\* iteration order; we break by seq, which differs only in bookkeeping, never in
\* the `ts` that drives future merges. roster.rs:60-66.)
GeqEntry(e, f) == (e.ts > f.ts) \/ (e.ts = f.ts /\ e.seq >= f.seq)

MaxEntry(S) == IF S = {} THEN Nil
               ELSE CHOOSE e \in S : \A f \in S : GeqEntry(e, f)

Active(r, d)     == r.active[d] # Nil
Tombstoned(r, d) == r.active[d] = Nil /\ r.tomb[d] # Nil
ActiveSet(r)     == { d \in Node : r.active[d] # Nil }

\* --------------------------------------------------------------------------
\* Roster::merge (roster.rs:48-84). Per device: take the newest add and newest
\* tombstone across both inputs; a tombstone wins ONLY if STRICTLY newer than the
\* latest add (`rts > rec.added_at_ms`), so an add at least as new as any tombstone
\* keeps the device active (equality → active). Order-independent given the inputs.
\* --------------------------------------------------------------------------
ResolveDevice(r1, r2, d) ==
    LET adds == { e \in {r1.active[d], r2.active[d]} : e # Nil }   \* roster.rs:59-61
        rems == { e \in {r1.tomb[d],   r2.tomb[d]}   : e # Nil }   \* roster.rs:62-66
        maxAdd == MaxEntry(adds)
        maxRem == MaxEntry(rems)
    IN  \* roster.rs:68-81: tombstone strictly newer than the add → removed
        IF maxAdd # Nil /\ maxRem # Nil /\ maxRem.ts > maxAdd.ts
            THEN [active |-> Nil,    tomb |-> maxRem]
        \* an add at least as new as any tombstone → active (the (Some(rec), _) arm)
        ELSE IF maxAdd # Nil
            THEN [active |-> maxAdd, tomb |-> Nil]
        \* only a tombstone seen → removed
        ELSE IF maxRem # Nil
            THEN [active |-> Nil,    tomb |-> maxRem]
        \* nothing known about d
        ELSE [active |-> Nil, tomb |-> Nil]

Merge(r1, r2) ==
    [active |-> [d \in Node |-> ResolveDevice(r1, r2, d).active],
     tomb   |-> [d \in Node |-> ResolveDevice(r1, r2, d).tomb]]

\* add_device: clear any tombstone, insert into devices (roster.rs:142-154).
AddToRoster(r, d, e) == [r EXCEPT !.active[d] = e, !.tomb[d] = Nil]
\* remove_device: drop from devices, insert tombstone (roster.rs:157-163).
RemoveFromRoster(r, d, e) == [r EXCEPT !.active[d] = Nil, !.tomb[d] = e]

\* Two nodes replicate the library folder (and thus the roster file) iff each has
\* added the other (folder shared both ways — engine.rs:138-157).
Mutual(a, b) == b \in engine[a] /\ a \in engine[b]

\* --------------------------------------------------------------------------
\* reconcile (reconcile.rs:31-53): the pure plan diffing the merged roster against
\* this node's engine device set. Never touches self.
\*   to_add    = active roster devices missing from the engine
\*   to_remove = engine devices the roster TOMBSTONES and does not re-list
\* (a device merely absent from the roster is left alone — opt-in removal).
\* --------------------------------------------------------------------------
ReconcilePlan(n) ==
    [add    |-> { d \in Node : d # n /\ Active(roster[n], d) /\ d \notin engine[n] },
     remove |-> { d \in engine[n] : d # n /\ Tombstoned(roster[n], d) }]

\* ============================================================================
\* INIT
\* ============================================================================

\* Each node starts knowing only itself (ensure_self at startup, sync_daemon.rs:112;
\* roster.rs:167-177), with no peers configured yet.
SelfRoster(n) ==
    [active |-> [d \in Node |-> IF d = n THEN [ts |-> 0, seq |-> 0] ELSE Nil],
     tomb   |-> [d \in Node |-> Nil]]

Init ==
    /\ roster = [n \in Node |-> SelfRoster(n)]
    /\ engine = [n \in Node |-> {}]
    /\ gseq = 0
    /\ lastOp = [d \in Node |-> [seq |-> 0, kind |-> "none"]]

\* ============================================================================
\* ACTIONS
\* ============================================================================

\* --------------------------------------------------------------------------
\* PairOn: user pairs peer m on node n (sync.rs:127-141 sync_add_device →
\* engine.rs:162-165 pair_device = roster.add_device(m) + add_peer(m)+reshare).
\* `ts` is chosen freely to model this device's wall clock (skew).
\* --------------------------------------------------------------------------
PairOn(n, m, ts) ==
    /\ m # n
    /\ LET g == gseq + 1
           e == [ts |-> ts, seq |-> g]
       IN /\ gseq' = g
          /\ roster' = [roster EXCEPT ![n] = AddToRoster(roster[n], m, e)]
          /\ engine' = [engine EXCEPT ![n] = engine[n] \cup {m}]
          /\ lastOp' = [lastOp EXCEPT ![m] = [seq |-> g, kind |-> "add"]]

\* --------------------------------------------------------------------------
\* UnpairOn: user unpairs peer m on node n (sync.rs:168-181 →
\* engine.rs:169-172 unpair_device = roster.remove_device(m) + remove_peer(m)).
\* --------------------------------------------------------------------------
UnpairOn(n, m, ts) ==
    /\ m # n
    /\ LET g == gseq + 1
           e == [ts |-> ts, seq |-> g]
       IN /\ gseq' = g
          /\ roster' = [roster EXCEPT ![n] = RemoveFromRoster(roster[n], m, e)]
          /\ engine' = [engine EXCEPT ![n] = engine[n] \ {m}]
          /\ lastOp' = [lastOp EXCEPT ![m] = [seq |-> g, kind |-> "remove"]]

\* --------------------------------------------------------------------------
\* ApprovePending: the OTHER half of a first pairing. Peer p added n (n \in
\* engine[p]) and is connecting, so p shows up as a pending device on n; the user
\* on n accepts it, which is a pair_device of p (sync.rs:147-165 list/approve).
\* Distinct from PairOn only by the pending precondition.
\* --------------------------------------------------------------------------
ApprovePending(n, p, ts) ==
    /\ p # n
    /\ n \in engine[p]          \* p has us configured (p is connecting) — we are pending on... n
    /\ p \notin engine[n]       \* we have not added p yet
    /\ LET g == gseq + 1
           e == [ts |-> ts, seq |-> g]
       IN /\ gseq' = g
          /\ roster' = [roster EXCEPT ![n] = AddToRoster(roster[n], p, e)]
          /\ engine' = [engine EXCEPT ![n] = engine[n] \cup {p}]
          /\ lastOp' = [lastOp EXCEPT ![p] = [seq |-> g, kind |-> "add"]]

\* --------------------------------------------------------------------------
\* EnsureSelf: set_device_name re-adds self to the roster with a fresh timestamp
\* (engine.rs:178-181 → roster.rs:167-177). Models the C4 resurrection edge: if
\* another node tombstoned this device, ensure_self re-activates it. Touches only
\* the roster, not the engine peer set.
\* --------------------------------------------------------------------------
EnsureSelf(n, ts) ==
    /\ LET g == gseq + 1
           e == [ts |-> ts, seq |-> g]
       IN /\ gseq' = g
          /\ roster' = [roster EXCEPT ![n] = AddToRoster(roster[n], n, e)]
          /\ lastOp' = [lastOp EXCEPT ![n] = [seq |-> g, kind |-> "add"]]
          /\ UNCHANGED engine

\* --------------------------------------------------------------------------
\* RosterSync: Syncthing replicates src's roster file to dst, which union-merges
\* it (this also models load() merging a `.sync-conflict-*` sibling — same Merge,
\* roster.rs:106-126). Enabled only between mutual peers, and only when it
\* actually changes dst's roster (so a fixpoint is reachable).
\* --------------------------------------------------------------------------
RosterSync(src, dst) ==
    /\ src # dst
    /\ Mutual(src, dst)
    /\ LET merged == Merge(roster[dst], roster[src]) IN
       /\ merged # roster[dst]
       /\ roster' = [roster EXCEPT ![dst] = merged]
    /\ UNCHANGED <<engine, gseq, lastOp>>

\* --------------------------------------------------------------------------
\* ReconcileNode: one reconcile_once pass (engine.rs:187-212, driven every 10s by
\* sync_daemon.rs:122-133). Brings the engine peer set in line with the merged
\* roster: add active devices, remove tombstoned-and-absent ones; reshare is
\* implicit (share set == engine \cup self). Touches only the engine, not the
\* roster. Enabled only when the plan is non-empty.
\* --------------------------------------------------------------------------
ReconcileNode(n) ==
    /\ LET plan == ReconcilePlan(n)
           next == (engine[n] \cup plan.add) \ plan.remove
       IN /\ next # engine[n]
          /\ engine' = [engine EXCEPT ![n] = next]
    /\ UNCHANGED <<roster, gseq, lastOp>>

\* ============================================================================
\* NEXT
\* ============================================================================

Next ==
    \/ \E n, m \in Node : \E ts \in 0..MaxClock : PairOn(n, m, ts)
    \/ \E n, m \in Node : \E ts \in 0..MaxClock : UnpairOn(n, m, ts)
    \/ \E n, p \in Node : \E ts \in 0..MaxClock : ApprovePending(n, p, ts)
    \/ \E n \in Node    : \E ts \in 0..MaxClock : EnsureSelf(n, ts)
    \/ \E src, dst \in Node : RosterSync(src, dst)
    \/ \E n \in Node    : ReconcileNode(n)

Spec == Init /\ [][Next]_vars

\* ============================================================================
\* INVARIANTS
\* ============================================================================

TypeOK ==
    /\ roster \in [Node -> RosterType]
    /\ engine \in [Node -> SUBSET Node]
    /\ gseq \in Nat
    /\ lastOp \in [Node -> [seq : Nat, kind : {"none", "add", "remove"}]]

\* A device id is never simultaneously active and tombstoned in one roster
\* (add_device/remove_device keep the two maps disjoint; merge preserves it).
RosterDisjoint ==
    \A n \in Node, d \in Node :
        ~(roster[n].active[d] # Nil /\ roster[n].tomb[d] # Nil)

\* No node ever adds or removes itself as a peer.
NoSelfPeer == \A n \in Node : n \notin engine[n]

\* ---- Convergence gate ------------------------------------------------------
\* No mutual RosterSync would change anything (every connected node has merged).
FullyMerged ==
    \A s, d \in Node : (s # d /\ Mutual(s, d)) => Merge(roster[d], roster[s]) = roster[d]

\* The mesh is fully closed (every pair of nodes mutually shares). Used to isolate
\* Family 1 (skew) from Family 3 (propagation/partition): we only assert causal
\* correctness when there is no partition to blame.
AllConnected == \A a, b \in Node : a # b => Mutual(a, b)

Converged == FullyMerged /\ AllConnected

ActiveAnywhere(d) == \E n \in Node : Active(roster[n], d)

\* ---- Family 1: convergent + causally correct -------------------------------
\* At a closed, fully-merged mesh, every node holds an identical roster (merge is
\* commutative/idempotent). A violation means the CRDT does not converge.
ConvergenceAgreement ==
    Converged => \A a, b \in Node : roster[a] = roster[b]

\* At a closed, fully-merged mesh, a device whose globally-latest operation was a
\* removal must not be active anywhere. Clock skew (a remove stamped with a ts
\* older than an earlier add) can break this — the open question behind M1/M3.
NoSpuriousResurrection ==
    Converged =>
        \A d \in Node : (lastOp[d].kind = "remove") => ~ActiveAnywhere(d)

\* ---- Family 2: reconcile never tears down an active device -----------------
\* reconcile's remove set must contain only tombstoned (not active) devices, in
\* every reachable state (pure-function safety on reconcile.rs:42-50).
ReconcileNeverDropsActive ==
    \A n \in Node : \A d \in ReconcilePlan(n).remove : ~Active(roster[n], d)

\* ---- Family 3: mesh closure (safety surrogate for the liveness target) ------
\* A node is "in the mesh" once it has any peer edge in either direction.
InMesh(n) == \E m \in Node : m # n /\ (m \in engine[n] \/ n \in engine[m])

\* No reconcile would change any engine set (the 10s poller has reached fixpoint).
ReconcileSettled ==
    \A n \in Node : ReconcilePlan(n).add = {} /\ ReconcilePlan(n).remove = {}

\* Some first-pairing is still half-open: peer p is connecting to n (n \in engine[p])
\* but n has not approved it yet (p \notin engine[n]). The second side is a MANUAL
\* approval (sync.rs:147-165), so closure is only promised once these are resolved.
PendingApprovalExists ==
    \E n, p \in Node : n # p /\ n \in engine[p] /\ p \notin engine[n]

\* The whole system has quiesced: roster sync AND reconcile are at fixpoint and no
\* first-pairing approval is still outstanding.
Settled == FullyMerged /\ ReconcileSettled /\ ~PendingApprovalExists

\* When the system is fully settled, every pair of in-mesh nodes must be mutual —
\* i.e., a single hub pairing has fanned out to a full mesh via roster sync +
\* reconcile, with no node left half-connected. A settled-but-not-closed state is
\* the Family-3 bug. (Nodes never paired into the mesh are excluded by InMesh, so
\* this is not falsified merely because the scenario didn't pair everyone.)
MeshClosesWhenSettled ==
    Settled =>
        \A a, b \in Node : (a # b /\ InMesh(a) /\ InMesh(b)) => Mutual(a, b)

=============================================================================
