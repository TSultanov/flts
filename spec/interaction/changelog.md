# Interaction Spec — Verification Changelog

## Round 1 - Trace Validation
- [fix] ConfigChange: added missing `newLib` field to trace event (Trace.tla reads `Logline(a).newLib` to set `currentLib'`)
- [fix] BeginWorker/WorkerSave/BeginTauri/BeginWatcher: updated `lib` field from 1→2 after ConfigChange to maintain consistency (Trace: interaction-baseline.ndjson)
- [fix] DeliverEvent: added 4th DeliverEvent to baseline since ConfigChange also produces a pending event (Trace: interaction-baseline.ndjson)

**Result**: Both traces pass (baseline: 19 states depth 19, concurrent: 16 states depth 12 with branching).

## Round 1 - Model Checking
- [fix-inv] MCUIConsistency: added `maxDeliveredVersion > 0` guard — invariant too strong at initial state where UI hasn't received any events yet (Case A)
- [bug] TauriEmit→DeliverEvent: F2 Stale Snapshot Overwrites — stale v2 event delivered after fresh v3, UI regresses (Case C, 8-state counterexample)
- Structural invariants (PCConsistency, TaskLibraryValidity): no violations after 2.3B states generated, 496M distinct states, depth 126 (stopped by user)

## Bug Hunting
- [bug] F1 StaleLibrarySafety: ConfigChange creates new library but in-flight tasks retain stale Arc ref (MC_hunt_f1.cfg, 4-state counterexample)
- [bug] F2 EventMonotonicity: confirmed — stale snapshot overwrites fresher UI state (MC_hunt_f2.cfg, re-confirmed)
- [bug] F3 NoPersistenceLoss: app has no shutdown handler, in-memory state lost on close (MC_hunt_f3.cfg, 4-state counterexample, exhaustive BFS)
- [bug] F4 NoStaleTranslation: worker reads paragraph then stores translation after book reloaded by watcher (MC_hunt_f4.cfg, 31-state counterexample)

## Result
Converged in 1 round. Bug hunting: 4 bugs found (all 4 families confirmed).

## Spec Update — Post-Fix
Updated base.tla to reflect implemented fixes:

- **F1 FIXED**: ConfigChange now resets all worker tasks to idle (models JoinHandle::abort
  in TranslationQueue::Drop). StaleLibrarySafety and NoDataLoss should now pass.
- **F2 PARTIAL**: Added comments explaining emit_versioned fix. The fix targets invoke
  races and getterToReadableWithEvents (not modeled here). For the FIFO eventToReadable
  pattern modeled in this spec, the version check is a no-op — EventMonotonicity and
  UIConsistency can still be violated.
- **F3 FIXED**: AppClose now sets diskVersion = memVersion for all books (models
  save_all() in RunEvent::Exit handler). NoPersistenceLoss should now pass.
- **F4 UNFIXED**: No changes. NoStaleTranslation can still be violated.

MC.cfg updated: F1 invariants enabled. Hunt configs annotated with fix status.
