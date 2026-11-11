# Brief-coverage self-audit — roster-mesh spec

Maps the roster modeling brief's §2 (families) / §5 (invariants) / §6.1
(model-checkable findings) onto the artifacts in this directory. This spec covers the
roster-mesh protocol — brief Families 1-3. The brief's Families 4 (Syncthing-fed file
merge) and 5 (save/watch echo-gate quiescence) described the file-per-book persistence,
which the library no longer uses: data is rows in a dbsync store, merged by the store
itself. They are not listed below.

Results are the Round 1 run, against the wall-clock LWW roster. F1 has since been fixed
(vector-clock CRDT, remove-wins) and `MC_hunt_f1.cfg` re-verified clean; see
`changelog.md` Round 2 and `bug-report.md`.

## §2 Families → hunting cfg

| Family                                     | Mechanism                                     | Hunt cfg         | Targeting invariant/property                     | Status                                    |
| ------------------------------------------ | --------------------------------------------- | ---------------- | ------------------------------------------------ | ----------------------------------------- |
| F1 Roster CRDT LWW under per-device clocks | `roster.rs:48-84` merge                       | `MC_hunt_f1.cfg` | `ConvergenceAgreement`, `NoSpuriousResurrection` | **Counterexample found** (see §6.1 M1/M3) |
| F2 Reconcile add/remove asymmetry          | `reconcile.rs:31-53`                          | `MC_hunt_f2.cfg` | `ReconcileNeverDropsActive`                      | Holds (20.9M states, no error)            |
| F3 Mesh propagation / closure              | `engine.rs:187-212`, `sync_daemon.rs:122-133` | `MC_hunt_f3.cfg` | `MeshClosesWhenSettled`                          | Holds (mesh closes from one hub)          |

## §5 Invariants → enabled in ≥1 cfg

| Brief invariant             | Spec operator                                                                        | Enabled in       | Result                                    |
| --------------------------- | ------------------------------------------------------------------------------------ | ---------------- | ----------------------------------------- |
| `RosterConvergence`         | `ConvergenceAgreement`                                                               | `MC_hunt_f1.cfg` | reachable, holds when causality respected |
| `NoSpuriousResurrection`    | `NoSpuriousResurrection`                                                             | `MC_hunt_f1.cfg` | **violated** under ts collision/skew      |
| `ReconcileNeverDropsActive` | `ReconcileNeverDropsActive`                                                          | `MC_hunt_f2.cfg` | holds                                     |
| `MeshClosure`               | `MeshClosesWhenSettled` (safety surrogate) + `MeshConverges` (temporal, in `MC.tla`) | `MC_hunt_f3.cfg` | holds                                     |

Structural (always on): `TypeOK`, `RosterDisjoint`, `NoSelfPeer` — in `MC.cfg`
convergence run (clean, ~1.08M distinct states).

## §6.1 Model-checkable findings → reachability

| ID  | Description                                                                         | Hunt cfg makes it reachable?                           | Outcome                                                                |
| --- | ----------------------------------------------------------------------------------- | ------------------------------------------------------ | ---------------------------------------------------------------------- |
| M1  | Remove on a behind-clock node loses to a causally-earlier add (skew) → resurrection | `MC_hunt_f1.cfg` (`MaxClock=2` allows ts inversion)    | **Confirmed reachable** — `NoSpuriousResurrection` counterexample      |
| M2  | Third node isolated despite hub pairing?                                            | `MC_hunt_f3.cfg`                                       | Refuted in scope — mesh always closes when settled; no isolation found |
| M3  | Equal-`now_ms()` add+remove resolve to "active" (`rts > added` false at equality)   | `MC_hunt_f1.cfg` (the found trace uses `ts=0` on both) | **Confirmed reachable** — same counterexample, equal-ts form           |

### F1 counterexample (M1/M3) — what TLC found

`MC_hunt_f1.cfg`, 11-step trace: n1 adds n2 (`ts=0, seq=3`) while n3 removes n2
(`ts=0, seq=4`); after the mesh fully merges and connects, n2 is still active
everywhere although its globally-latest operation (`seq=4`) was the removal. Faithful
to `roster.rs:70` (`rts > rec.added_at_ms` — a tombstone equal-or-older in wall-clock
loses). This is the open question, not a coding slip: the resolution is the brief **C1**
design decision (wall clock vs. logical clock / version vector). The hunt cfg's job is
to make the open question concrete; it did.

## Gaps (honest)

- **`MeshConverges` (temporal liveness) is defined but not the checked form.** The
  `<>AllConnected` property stutters on behaviors that never perform the setup pairings;
  the safety surrogate `MeshClosesWhenSettled` checks the same closure guarantee without
  forced-pairing fairness. Re-enabling the temporal form (with WF on the pairing actions)
  is a validation-workflow tuning task.
