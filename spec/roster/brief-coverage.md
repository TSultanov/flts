# Brief-coverage self-audit — roster-mesh spec

Maps `../modeling-brief.md` §2 (families) / §5 (invariants) / §6.1 (model-checkable
findings) onto the artifacts in this directory. This spec covers the **new
distributed protocol** — brief Families 1-3. Family 4 (file-merge) is the existing
`../base.tla`; Family 5 (echo-gate quiescence) is **not modeled** (see gaps below).

## §2 Families → hunting cfg

| Family | Mechanism | Hunt cfg | Targeting invariant/property | Status |
|---|---|---|---|---|
| F1 Roster CRDT LWW under per-device clocks | `roster.rs:48-84` merge | `MC_hunt_f1.cfg` | `ConvergenceAgreement`, `NoSpuriousResurrection` | **Counterexample found** (see §6.1 M1/M3) |
| F2 Reconcile add/remove asymmetry | `reconcile.rs:31-53` | `MC_hunt_f2.cfg` | `ReconcileNeverDropsActive` | Holds (20.9M states, no error) |
| F3 Mesh propagation / closure | `engine.rs:187-212`, `sync_daemon.rs:122-133` | `MC_hunt_f3.cfg` | `MeshClosesWhenSettled` | Holds (mesh closes from one hub) |
| F4 Syncthing-fed file merge | `library_book/`, `translation.rs` | `../MC_hunt_*.cfg` (existing) | `TranslationDistinctVersionsPreserved`, `DictionaryEntriesMonotonic` | Existing spec (`../base.tla`) |
| F5 Save/watch echo-gate quiescence | `6e5e4f4`, `library_book/mod.rs:441-459` | **none** | `EchoQuiescence` | **Gap — not modeled (see below)** |

## §5 Invariants → enabled in ≥1 cfg

| Brief invariant | Spec operator | Enabled in | Result |
|---|---|---|---|
| `RosterConvergence` | `ConvergenceAgreement` | `MC_hunt_f1.cfg` | reachable, holds when causality respected |
| `NoSpuriousResurrection` | `NoSpuriousResurrection` | `MC_hunt_f1.cfg` | **violated** under ts collision/skew |
| `ReconcileNeverDropsActive` | `ReconcileNeverDropsActive` | `MC_hunt_f2.cfg` | holds |
| `MeshClosure` | `MeshClosesWhenSettled` (safety surrogate) + `MeshConverges` (temporal, in `MC.tla`) | `MC_hunt_f3.cfg` | holds |
| `TranslationDistinctVersionsPreserved` | — | `../MC.cfg` (existing) | existing spec |
| `DictionaryEntriesMonotonic` | — | `../MC.cfg` (existing) | existing spec |
| `EchoQuiescence` | — | none | **gap** |

Structural (always on): `TypeOK`, `RosterDisjoint`, `NoSelfPeer` — in `MC.cfg`
convergence run (clean, ~1.08M distinct states).

## §6.1 Model-checkable findings → reachability

| ID | Description | Hunt cfg makes it reachable? | Outcome |
|---|---|---|---|
| M1 | Remove on a behind-clock node loses to a causally-earlier add (skew) → resurrection | `MC_hunt_f1.cfg` (`MaxClock=2` allows ts inversion) | **Confirmed reachable** — `NoSpuriousResurrection` counterexample |
| M2 | Third node isolated despite hub pairing? | `MC_hunt_f3.cfg` | Refuted in scope — mesh always closes when settled; no isolation found |
| M3 | Equal-`now_ms()` add+remove resolve to "active" (`rts > added` false at equality) | `MC_hunt_f1.cfg` (the found trace uses `ts=0` on both) | **Confirmed reachable** — same counterexample, equal-ts form |
| M4 | Echo-gate quiescence under concurrent divergent edits | none (Family 5 not modeled) | **gap** |
| M5 | Translation siblings, distinct ids, equal timestamps collapse | `../MC.cfg` (existing) | existing spec |

### F1 counterexample (M1/M3) — what TLC found

`MC_hunt_f1.cfg`, 11-step trace: n1 adds n2 (`ts=0, seq=3`) while n3 removes n2
(`ts=0, seq=4`); after the mesh fully merges and connects, n2 is still active
everywhere although its globally-latest operation (`seq=4`) was the removal. Faithful
to `roster.rs:70` (`rts > rec.added_at_ms` — a tombstone equal-or-older in wall-clock
loses). This is the open question, not a coding slip: the resolution is the brief **C1**
design decision (wall clock vs. logical clock / version vector). The hunt cfg's job is
to make the open question concrete; it did.

## Gaps (honest)

- **Family 5 (echo-gate quiescence) is out of scope of this module.** It is a
  termination property on the data-file save/watch/reload loop (content hash + mtime),
  a different state machine from membership. It belongs with the file-merge spec
  (`../base.tla`) as a `mem/disk/hash` extension, or its own module. Not modeled here;
  `EchoQuiescence` / brief M4 remain open. Recommended next: add a small 2-node
  `content × hash × mtime` spec and check `<>[]` no-further-save under fairness.
- **`MeshConverges` (temporal liveness) is defined but not the checked form.** The
  `<>AllConnected` property stutters on behaviors that never perform the setup pairings;
  the safety surrogate `MeshClosesWhenSettled` checks the same closure guarantee without
  forced-pairing fairness. Re-enabling the temporal form (with WF on the pairing actions)
  is a validation-workflow tuning task.
