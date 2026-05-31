# Instrumentation spec — roster-mesh trace collection

Action → code mapping for `harness-generation`. Each running FLTS node emits one
NDJSON event per membership transition; trace validation (`Trace.tla`) replays them
against `base.tla`. Events are written to `../../traces/roster_*.ndjson`.

## Common event envelope

```json
{ "name": "<ActionName>", "node": "<this device id>",
  "roster": { "<dev>": { "add": { "<dev>": <counter>, ... },
                         "rem": { "<dev>": <counter>, ... } }, ... },
  "engine": [ "<peer dev id>", ... ] }
```

`roster` is this node's `RosterStore` content **after** the operation: per device,
its add and remove **vector clocks** (`AddStamp.vc` / `RemStamp.vc`; a vclock is
`{deviceId: counter}`, canonical/sparse). `engine` is the Syncthing peer device set
**after** the operation (`list_devices()` minus self).

> **Ground-truth caveat.** `gAdd` / `gRem` (and therefore `NoSpuriousResurrection`,
> `ConvergenceAgreement`) are modeling-only ground truth with no code counterpart;
> they are **not** trace-validatable. Trace validation checks operational faithfulness
> of the vector-clock `merge` / `reconcile` / pairing — the causal invariants stay
> MC-only.

## Per-action instrumentation

| Spec action | Code location | Trigger point | Captured fields (beyond envelope) |
|---|---|---|---|
| `PairOn(n,m)` | `engine.rs` `pair_device` → `roster.rs` `add_device` | after `save(&roster)` | `target`=m; roster carries each device's add/rem vc |
| `UnpairOn(n,m)` | `engine.rs` `unpair_device` → `roster.rs` `remove_device` | after `save(&roster)` | `target`=m |
| `ApprovePending(n,p)` | `sync.rs` approve → `engine.rs` `pair_device` | after `save(&roster)` | `target`=p (folded into `PairOn` in the harness) |
| `EnsureSelf(n)` | `engine.rs` `set_device_name` → `roster.rs` `ensure_self` | after `save` when a write occurred | `target`=n |
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
- **Clocks.** The causal state is the per-device add/remove **vector clocks**
  (`AddStamp.vc` / `RemStamp.vc`); emit them verbatim. (`addedAtMs`/`removedAtMs`
  remain in the file as advisory display only, post-F1-fix; they no longer order
  the merge.)
- **Silent steps.** Syncthing re-touches and folder reshares that change neither
  `roster` nor `engine` should NOT emit events (they map to spec stutter). Keep the
  emit guarded on an actual roster/engine change to keep `Trace.tla` silent-action
  count low.
