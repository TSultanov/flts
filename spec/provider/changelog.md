# Provider Spec — Verification Changelog

## Round 1 - Trace Validation
- [fix] Trace.cfg: widened `Request` from `{r1, r2}` to `{r1..r6}` so the baseline provider harness trace can replay all six modeled requests.
- [fix] GetTranslationParseFailure: updated the provider harness to emit post-state `bufferKind = "malformed"` on JSON parse failure, matching the instrumentation contract and base spec.
- [fix] TraceSpec: added `WF_<<vars, l>>(TraceNext)` so TLC cannot stutter before the final trace event; this matches the repo's other trace specs and removes false `TraceMatched` failures.

## Round 1 - Model Checking
- [fix-spec] MC.cfg: added `CHECK_DEADLOCK FALSE` because the provider model has legitimate quiescent terminal states after all requests have completed; TLC's default deadlock check was reporting a false positive instead of the configured safety invariants.

## Bug Hunting
- [bug] MCTrackedQueuedRequestEventuallyLeavesQueue: confirmed queue-stall bug — one active request can keep the single worker occupied indefinitely while a later queued request never leaves `queued` (`MC_hunt_f1.cfg`, `output/MC_hunt_f1_bfs.out`).
- [bug] MCFailureStatusNotComplete: confirmed failure-status-collapse bug — provider failures still publish the same terminal `is_complete = true` status shape used for successful saves (`MC_hunt_f2.cfg`, `output/MC_hunt_f2_bfs.out`).
- MC_hunt_f3.cfg: added `CHECK_DEADLOCK FALSE` to remove terminal-state false positives. BFS completed with no violation; simulation follow-up also found no violation under the current retry-once abstraction (`output/MC_hunt_f3_bfs.out`, `output/MC_hunt_f3_sim.out`).

## Result
Converged in 1 round. Bug hunting: 2 bugs found (F1, F2); no F3 violation found under the current retry-once model.
