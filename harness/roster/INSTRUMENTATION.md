# Roster-mesh Trace — Instrumentation Guide

Brief guide for the Phase-3 (validation) agent to adjust instrumentation when
trace validation of `spec/roster/Trace.tla` reveals issues.

## Overview

The roster harness exercises the **real** roster CRDT, reconcile, and engine glue
(`SyncEngine` over a `MockSyncthing` — no live Go engine) across a small simulated
mesh, emitting one NDJSON event per membership transition for `spec/roster/`.
Syncthing's file delivery is stood in by copying a node's `devices.json` into
another node's `.flts/` as a `.sync-conflict-*` sibling — exactly the input
`RosterStore::load` union-merges in production.

Instrumentation is **compiled into production code** behind the `tla_trace` Cargo
feature (zero-cost no-op when off). There is no patch/apply step.

## Files

| File | Purpose |
|------|---------|
| `library/src/tla_trace/trace.rs` | `emit_roster_event(...)` — writes the NDJSON envelope (real impl, `tla_trace` on) |
| `library/src/tla_trace/noop.rs` | zero-cost stand-in (`tla_trace` off) |
| `library/src/sync/engine.rs` | emit call sites + `trace_emit` helper (gathers post-state) |
| `library/src/sync/roster.rs` | `pending_sibling_sources` / `snapshot_for_trace` (trace-only peeks) |
| `library/src/sync/trace_harness.rs` | the two scenarios (`#[cfg(all(test, tla_trace, sync-engine))]`) |
| `harness/roster/run.sh` | build with the feature, run scenarios, check coverage |

## Event → emit site (post-`apply` = current source)

| Event | File:fn | Trigger | Notes |
|---|---|---|---|
| `EnsureSelf` | `engine.rs` `set_device_name` | after `ensure_self` + `rename_device` | `ts` = self `addedAtMs` |
| `PairOn` | `engine.rs` `pair_device` | after `add_device` + `add_peer` | `ts` = target `addedAtMs`. **ApprovePending is folded in here** (identical effect) |
| `UnpairOn` | `engine.rs` `unpair_device` | after `remove_device` + `remove_peer` | `ts` = target `removedAtMs` |
| `RosterSync` | `engine.rs` `reconcile_once` | after `load`, IFF it changed the roster | one per merged sibling; `src` = sibling `modifiedBy`; emitted only when `snapshot_for_trace() != load()` |
| `ReconcileNode` | `engine.rs` `reconcile_once` | after the add/remove apply loop | only when the plan was non-empty (engine changed) |

Every event carries the emitting node's POST-state: `node`, `roster.active`
(`id->addedAtMs`), `roster.tomb` (`id->removedAtMs`), `engine` (peer ids, self
excluded). `gseq`/`lastOp` are spec-only ground truth and are NOT captured (the
causal invariants stay MC-only).

## Common adjustments

- **Add a field to an event**: extend the `serde_json::Map` in
  `emit_roster_event` (`trace.rs`), pass it from `trace_emit` in `engine.rs`, and
  read it in `Trace.tla`'s `ValidateNode` / a wrapper.
- **Add a new event type**: add an emit call at the new site (mirror an existing
  one), add a wrapper + `TraceNext` disjunct in `Trace.tla`, and a name in the
  coverage set in `run.sh`.
- **Move a capture point** (before↔after an op): move the `self.trace_emit(...)`
  call; `trace_emit` re-reads the roster/engine, so it always reflects the state
  at the call site.
- **Timestamps**: `ts` and `addedAtMs`/`removedAtMs` are real `now_ms()` values.
  `Trace.tla` maps them through `DenseTs` (order-preserving), so set
  `Trace.cfg`'s `MaxClock >= ` the number of distinct ms in the trace.
- **Node ids**: the harness uses literal `"n1"/"n2"/"n3"` device ids; `Trace.cfg`
  sets `Node = {"n1","n2","n3"}` (strings, to match the JSON keys).

## Rebuild + re-run

```
bash harness/roster/run.sh           # regenerate traces/roster_*.ndjson
# then validate (Phase 3):
cd spec/roster && JSON=../../traces/roster_mesh_forms.ndjson \
  java -cp <tla2tools.jar>:<CommunityModules-deps.jar> tlc2.TLC -config Trace.cfg Trace.tla
```

## Scenarios + coverage

- `roster_mesh_forms.ndjson` (11 events): EnsureSelf×3, PairOn×4, RosterSync×2,
  ReconcileNode×2 — hub pairing fans out to a full 3-node mesh.
- `roster_unpair.ndjson` (14 events): the above plus UnpairOn, then tombstone
  RosterSync + reconcile removal on a third node.

Covers all 6 spec actions (ApprovePending via PairOn). Both traces pass
`Trace.tla` validation.
