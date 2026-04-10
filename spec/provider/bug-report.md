# Bug Report — FLTS Provider Interaction

## Summary

- Bug families tested: 3
- Bugs found: 2 (`F1`, `F2`)
- No violation found: `F3` under the current retry-once abstraction
- Configs run: `MC_hunt_f1.cfg`, `MC_hunt_f2.cfg`, `MC_hunt_f3.cfg`

---

## Bug 1: Single-Worker Queue Stall (`F1`)

- **Bug family**: F1 — non-terminating provider stream stalls later queued work
- **Severity**: High
- **Property violated**: `MCTrackedQueuedRequestEventuallyLeavesQueue`
- **Config**: `MC_hunt_f1.cfg`
- **Counterexample**: 9-state liveness counterexample, `output/MC_hunt_f1_bfs.out`

### Trace summary

1. `r1` is queued and starts running on the single translation worker.
2. `r2` is queued behind `r1`.
3. `r1` opens a provider stream and then only receives keepalive/non-progress steps.
4. The worker remains stuck on `r1` while `r2` stays forever in `queued`.

### Root cause

The translation queue runs a single worker loop and awaits each request to completion before pulling the next queued request. Provider integration enforces only a request-setup timeout and a per-chunk idle timeout; once a stream keeps returning before the idle timeout without making terminal progress, there is no overall stream deadline to evict it.

That means a provider request can monopolize the only worker indefinitely and starve later queued translations.

### Affected code

- `site/src-tauri/src/app/translation_queue.rs:108-146` — single worker loop processes one request at a time
- `library/src/translator.rs:16-17` — only request timeout + stream idle timeout are defined
- `library/src/translator/openai.rs:209-220` — request timeout + per-iteration idle timeout, but no overall stream wall-clock bound
- `library/src/translator/gemini.rs:209-228` — same missing overall stream deadline on the Gemini path

### Recommendation

Add an overall stream deadline or a bounded keepalive budget in the provider request path. Once exceeded, fail the active request so the worker can advance to the next queued translation.

---

## Bug 2: Failure Status Collapse (`F2`)

- **Bug family**: F2 — provider failures are surfaced as generic completion
- **Severity**: High
- **Invariant violated**: `MCFailureStatusNotComplete`
- **Config**: `MC_hunt_f2.cfg`
- **Counterexample**: 5-state counterexample, `output/MC_hunt_f2_bfs.out`

### Trace summary

1. A request is queued and starts running.
2. Provider initialization times out.
3. The request transitions to `failed`, but the published UI status is still `is_complete = true`.

### Root cause

The failure path in the translation worker sends the same terminal `TranslationStatus` shape that the success path sends after saving. The status payload has progress counters and a single `is_complete` bit, but no error discriminator, so provider failure is collapsed into a generic "done" state.

From the frontend's perspective, terminal success and terminal provider failure are indistinguishable.

### Affected code

- `site/src-tauri/src/app/translation_queue.rs:127-139` — worker error handler sends terminal `TranslationStatus { is_complete: true, progress_chars: 0, expected_chars: 0 }`
- `site/src-tauri/src/app/translation_queue.rs:393-398` — immediate save success sends the same terminal shape
- `site/src-tauri/src/app/translation_queue.rs:414-419` — delayed save success sends the same terminal shape
- `site/src/lib/data/library.ts:29-34` — `TranslationStatus` contains no failure/error field
- `site/src/lib/data/library.ts:84-103` — polling only observes request id, progress, expected chars, and `is_complete`

### Recommendation

Add an explicit terminal outcome to `TranslationStatus` (for example `success | error` plus an error kind/message) and preserve that distinction through the frontend polling/store layer.

---

## Family 3 Result: Retry-Once Stream Error (`F3`)

- **Bug family**: F3 — first chunk-level stream error should be retried
- **Config**: `MC_hunt_f3.cfg`
- **BFS result**: No violation, `output/MC_hunt_f3_bfs.out`
- **Simulation result**: No violation observed, `output/MC_hunt_f3_sim.out`

### Notes

The F3 hunt is clean under the current spec because the provider model already encodes the user-requested retry-once abstraction.

The BFS run completed exhaustively at depth 9. A simulation follow-up sampled at least **700,407,585** states across **111,978,864** traces without finding a violation before it was stopped.

This does **not** mean the current implementation already behaves this way; it only means the converged provider spec is internally consistent with the intended retry-once model.

---

## Coverage summary

| Config | Mode | Result | Evidence |
|---|---|---|---|
| `MC.cfg` | BFS | No violation | `output/MC_round1.out` |
| `MC_hunt_f1.cfg` | BFS | **Bug found** (`F1`) | `output/MC_hunt_f1_bfs.out` |
| `MC_hunt_f2.cfg` | BFS | **Bug found** (`F2`) | `output/MC_hunt_f2_bfs.out` |
| `MC_hunt_f3.cfg` | BFS | No violation | `output/MC_hunt_f3_bfs.out` |
| `MC_hunt_f3.cfg` | Simulation | No violation observed | `output/MC_hunt_f3_sim.out` |
