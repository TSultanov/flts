# Instrumentation spec — roster-mesh trace collection

Action → code mapping for `harness-generation`. Each running FLTS node emits one
NDJSON event per membership transition; trace validation (`Trace.tla`) replays them
against `base.tla`. Events are written to `../../traces/roster_*.ndjson`.

## Common event envelope

```json
{ "action": "<ActionName>", "node": "<this device id>", "ts": <wall-clock ms>,
  "roster": { "active": { "<dev>": <addedAtMs>, ... },
              "tomb":   { "<dev>": <removedAtMs>, ... } },
  "engine": [ "<peer dev id>", ... ] }
```

`roster` is this node's `RosterStore` content **after** the operation
(`active` = `Roster::devices` as id→`addedAtMs`; `tomb` = `Roster::removed`).
`engine` is the Syncthing peer device set **after** the operation
(`list_devices()` minus self).

> **Ground-truth caveat.** `gseq` / `lastOp` (and therefore `NoSpuriousResurrection`,
> `ConvergenceAgreement`) are modeling-only ground truth with no code counterpart;
> they are **not** trace-validatable. Trace validation checks operational faithfulness
> of `merge` / `reconcile` / pairing — the causal invariants stay MC-only.

## Per-action instrumentation

| Spec action | Code location | Trigger point | Captured fields (beyond envelope) |
|---|---|---|---|
| `PairOn(n,m,ts)` | `engine.rs:162-165` `pair_device` → `roster.rs:142-154` `add_device` | after `save(&roster)` | `target`=m, `ts`=`addedAtMs` written |
| `UnpairOn(n,m,ts)` | `engine.rs:169-172` `unpair_device` → `roster.rs:157-163` `remove_device` | after `save(&roster)` | `target`=m, `ts`=`removedAtMs` |
| `ApprovePending(n,p,ts)` | `sync.rs:147-165` approve → `engine.rs:162-165` `pair_device` | after `save(&roster)` | `target`=p, `ts`=`addedAtMs`; mark `pending=true` |
| `EnsureSelf(n,ts)` | `engine.rs:178-181` `set_device_name` → `roster.rs:167-177` `ensure_self` | after `save` when a write occurred | `target`=n, `ts`=`addedAtMs` |
| `RosterSync(src,dst)` | `roster.rs:106-126` `load` (conflict-sibling union-merge) | after `save(&roster)` + sibling delete | `src`=modifier id parsed from the `.sync-conflict-<…>-<modifiedBy>` filename; emit on `dst` |
| `ReconcileNode(n)` | `engine.rs:187-212` `reconcile_once` | after the add/remove loop | `toAdd`=[ids], `toRemove`=[ids] (the applied `ReconcilePlan`) |

## Notes for the harness

- **One folder, many nodes.** The Docker multi-node harness (`c36544f`,
  `sync-harness/`) already runs ≥2 engines sharing a folder; tap each node's
  `RosterStore`/engine to emit the envelope. Use `FLTS_MOCK_TRANSLATORS=1` and a
  temp `FLTS_CONFIG_DIR` per node (see project memory) so the harness stays hermetic.
- **`RosterSync` is the merge observable.** The spec's `RosterSync(src,dst)` collapses
  "Syncthing delivered src's file / a conflict sibling, dst merged it." Emit it from
  `RosterStore::load` whenever `conflict_siblings()` was non-empty, recording the
  merged result; `src` comes from the sibling's `modifiedBy` field.
- **Clocks.** Record real `now_ms()` in `ts`. Cross-node skew in the captured `ts`
  values is exactly the F1 input; do not normalize it.
- **Silent steps.** Syncthing re-touches and folder reshares that change neither
  `roster` nor `engine` should NOT emit events (they map to spec stutter). Keep the
  emit guarded on an actual roster/engine change to keep `Trace.tla` silent-action
  count low.
