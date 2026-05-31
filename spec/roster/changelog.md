# spec/roster verification changelog

## Round 1 - Trace Validation
- [pass] roster_mesh_forms.ndjson (11 events): EnsureSelf×3, PairOn×4, RosterSync×2, ReconcileNode×2 — TraceMatched, 13 states. No spec change needed.
- [pass] roster_unpair.ndjson (14 events): adds UnpairOn + tombstone RosterSync + reconcile removal — TraceMatched, 16 states. No spec change needed.

## Round 1 - Model Checking
- [pass] MC.cfg (TypeOK, RosterDisjoint, NoSelfPeer): exhaustive, no violation. ~1.08M distinct states, no spec change.

## Round 1 - Bug Hunting
- [bug] Roster::merge / NoSpuriousResurrection: a removed device resurrects mesh-wide when a concurrent add and its causally-later removal carry equal or skewed wall-clock timestamps (`rts > added_at_ms` strict tie-break, roster.rs:70). Config MC_hunt_f1.cfg, 11-state counterexample, output/MC_hunt_f1.out. See bug-report.md Bug 1.
- [pass] ReconcileNeverDropsActive (F2): exhaustive, 20.9M states, depth 17, no violation.
- [pass] MeshClosesWhenSettled (F3): exhaustive, 5,405 states, depth 11, no violation.

## Result
Converged in 1 round (no spec modifications in Phase 2 → Phase 3 converged immediately).
The base spec faithfully models the implementation (traces ⊆ spec) and admits no
illegal structural states (spec ⊆ legal).

Bug hunting: 1 bug found (F1 — clock-skew/equal-ms device resurrection; the brief's
C1 design question, now confirmed reachable). F2 and F3 exhaustively clean. Full
write-up in bug-report.md.

## Round 2 - F1 fix (vector-clock CRDT)

- [fix-impl] roster.rs: replaced wall-clock LWW with a vector-clock CRDT
  (remove-wins) — `is_present` = add context strictly dominates remove context.
  Convergence proven by a proptest (commutative/associative/idempotent over mixed
  legacy/new inputs); causal unit tests (dominant-remove-wins under skew,
  concurrent-remove-wins, re-add) + upgrade tests (legacy deserialize, legacy⊔new
  remove-wins, self-heal) pass. Additive schema keeps old `devices`/`removed`
  fields for not-yet-upgraded nodes.
- [fix-spec] base.tla: re-grounded to the vector-clock model — entries are
  `[add: VC, rem: VC]`, ops stamp the node's advanced context, `RosterSync` joins
  pointwise, `Present` = add strictly dominates rem. `gAdd`/`gRem` ground truth
  replace `gseq`/`lastOp`. `NoSpuriousResurrection` re-stated causally.
- [re-verify] MC.cfg convergence clean (2,539 states). **MC_hunt_f1 now clean**
  (10,463 states, depth 18 — the resurrection is gone). f2 clean (9,921), f3 clean
  (179, hub mesh closes). Both traces re-validate against the updated Trace.tla.

## Result (final)

Converged; F1 bug found in Round 1 and **fixed + re-verified in Round 2**. The
roster CRDT is now causally correct (remove-wins) and convergent. F2/F3 hold.
