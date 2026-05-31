# Bug Report — FLTS roster mesh (`spec/roster/`)

## Summary

- Bug families tested: 3 (F1 roster CRDT LWW, F2 reconcile asymmetry, F3 mesh closure)
- Bugs found: 1 (F1)
- Configs run: `MC_hunt_f1.cfg`, `MC_hunt_f2.cfg`, `MC_hunt_f3.cfg`
- Spec status: converged in Round 1 (traces pass; `MC.cfg` structural invariants exhaustive-clean, 1.08M states). The spec is trusted.

## Bug 1: A removed device silently resurrects mesh-wide under clock skew / equal-ms

> **STATUS: FIXED.** Membership is now a vector-clock CRDT (remove-wins): merge
> orders add vs remove by causal context, not wall clock (`roster.rs`
> `is_present` = add context strictly dominates remove context). After the fix,
> `MC_hunt_f1.cfg` reports **no violation** (`NoSpuriousResurrection` re-stated
> causally; exhaustive, 10,463 states, `spec/roster/output/MC_hunt_f1.out`), and
> the Rust convergence proptest + causal unit tests pass. See `changelog.md`.

- **Bug Family**: F1 — roster CRDT last-writer-wins under per-device wall clocks
- **Severity**: High (a removed/unpaired device is silently re-admitted to the synced library on every node — an access-control/privacy consequence — though it requires a concurrent add+remove of the same device plus a clock-skew/equal-timestamp condition)
- **Invariant violated** (pre-fix): `NoSpuriousResurrection`
- **Config**: `MC_hunt_f1.cfg` (3 nodes, `MaxClock=2`)
- **Counterexample** (pre-fix): 11 states; 8,139,892 distinct states explored before the violation

### Trace Summary

Ground truth: device **n2**'s globally-latest operation is a *removal* (`seq=4`),
so a converged, fully-connected mesh must show n2 as removed. It does not.

| State | Action | Effect |
|---|---|---|
| 2 | `PairOn(n2, n1, ts=1)` | n2 adds n1 |
| 3 | `PairOn(n3, n1, ts=0)` | n3 adds n1 |
| 4 | `PairOn(n1, n2, ts=0)` | n1 adds **n2** → `active[n2] = {ts:0, seq:3}` |
| 5 | `UnpairOn(n3, n2, ts=0)` | n3 removes **n2** → tombstone `{ts:0, seq:4}` (causally LATER) |
| 6 | `ApprovePending(n1, n3, ts=0)` | second side of the n1–n3 pairing |
| 7–9 | `RosterSync ×3` | rosters replicate and union-merge |
| 10–11 | `ReconcileNode(n2), ReconcileNode(n3)` | engines converge → full mesh |

**Final state**: all three rosters are identical and `engine = (n1↦{n2,n3}, n2↦{n1,n3}, n3↦{n1,n2})` — a fully-closed mesh — with `active[n2] = {ts:0, seq:3}` and **`tomb[n2] = nil` on every node**. The removal at `seq=4` has vanished: n2 is fully, silently re-paired everywhere.

### Root Cause

`Roster::merge` resolves an add-vs-tombstone race by **wall-clock** timestamp, and
a tombstone wins **only if strictly newer** than the add:

- `library/src/sync/roster.rs:70` — `(Some(rec), Some(rts)) if rts > rec.added_at_ms => removed`. At equal timestamps (`rts == added_at_ms`) this arm is skipped and the next arm (`(Some(rec), _) => active`) keeps the device **active**.
- `library/src/sync/roster.rs:209` (`now_ms`) — both the add (`added_at_ms`) and the removal tombstone are stamped from each device's **independent wall clock**. There is no logical/causal clock, so a causally-later removal can carry a timestamp `≤` the add it is meant to supersede — via either an exact millisecond collision (this trace, `ts=0==0`) or ordinary cross-device clock skew (a removal on a behind-clock node).

The merge is convergent (all nodes agree) but **not causal**: agreement on the *wrong* value. Once the add wins, the tombstone is dropped entirely, so nothing on any node records that a removal ever happened — the device looks legitimately paired.

### Affected Code

- `library/src/sync/roster.rs:68-72` — the add-vs-tombstone resolution (`rts > added_at_ms`); strict-`>` tie-break + wall-clock basis is the defect.
- `library/src/sync/roster.rs:142-163` — `add_device` / `remove_device` stamp `now_ms()` with no causal ordering.
- `library/src/sync/reconcile.rs:31-53` — downstream: an "active" merged record makes `reconcile` re-add the device to the engine and reshare the folder, so the resurrection becomes real Syncthing access (not just a roster artifact).

### Recommendation

This is the open design question flagged as **C1** in `../modeling-brief.md`; the model
confirms it is reachable. Wall-clock LWW cannot express causal "remove-after-add".
Options, strongest first:

1. **Per-device version vector / monotonic op counter** on roster entries; merge by causal order, not wall clock. Eliminates both the skew and equal-ms cases. (Matches brief C1.)
2. **Hybrid logical clock** for `addedAtMs`/`removedAtMs` so timestamps are monotonic and causally consistent across devices.
3. **Partial mitigation only**: make a tombstone win on ties (`rts >= rec.added_at_ms`, `roster.rs:70`). Closes the equal-ms collision (this trace) but **not** clock skew, where a later removal still carries a smaller wall-clock value. Not sufficient alone.

A re-add must still be able to beat an older tombstone (the intended `add_device` clears-tombstone behavior), so any fix must preserve "newer add resurrects" while making "newer remove" win — which is exactly what a causal clock gives and wall-clock LWW cannot.

---

## Not Reproduced

| Bug Family | Config | States Explored | Result |
|---|---|---|---|
| F2 — reconcile never drops an active device | `MC_hunt_f2.cfg` | 20,876,727 distinct (depth 17, **exhaustive**, 0 left on queue) | No violation — `ReconcileNeverDropsActive` holds |
| F3 — mesh closure | `MC_hunt_f3.cfg` | 5,405 distinct (depth 11, **exhaustive**, 0 left on queue) | No violation — `MeshClosesWhenSettled` holds (a single hub pairing fans out to a full mesh) |

Both ran exhaustive BFS to completion (0 states left on queue), so simulation
follow-up is unnecessary (the depth-≤25 simulation rule applies to runs that hit
the time cap with states remaining; these finished).

## Spec fixes during hunting

None. The spec converged in Round 1 and was not modified during bug hunting.
