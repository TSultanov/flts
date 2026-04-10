# Modeling Brief: FLTS Translation Queue <-> LLM Provider Interaction

## 1. System overview

- **System**: FLTS desktop/backend translation pipeline from `translate_paragraph()` through `TranslationQueue::handle_request()` into provider-specific streaming translators for OpenAI and Gemini.
- **Category**: **Category A (distributed / message-passing)**. The key boundary is a remote provider API with request setup, streaming response delivery, transport errors, and timeout behavior rather than shared-memory synchronization (`site/src-tauri/src/app.rs:263-274`, `site/src-tauri/src/app/translation_queue.rs:221-370`, `library/src/translator/openai.rs:184-259`, `library/src/translator/gemini.rs:209-255`).
- **Concurrency model**:
  - `translate_paragraph()` enqueues work into a single translation worker (`site/src-tauri/src/app/translation_queue.rs:103-147`).
  - The worker processes one request at a time and awaits `handle_request()` to completion before reading the next queued request (`site/src-tauri/src/app/translation_queue.rs:108-146`).
  - Provider interaction is streaming. The implementation currently differs by provider on chunk errors, but this spec intentionally abstracts that away and assumes the desired future behavior: one chunk-level failure is tolerated and the request continues with a retry/continued stream (`library/src/translator/openai.rs:217-237`, `library/src/translator/gemini.rs:225-237`).

## 2. Bug families

### Family 1: Non-terminating slow streams can block the single worker indefinitely

**Mechanism:** the provider layer enforces only a request-start timeout and a per-chunk idle timeout. A stream that keeps producing occasional chunks or non-progress keepalive events before the idle timeout can remain active forever because there is no overall stream deadline. Since the queue has a single worker, later requests cannot start while the active request remains in the streaming state.

**Evidence:**
- `library/src/translator.rs:16-17` defines only `TRANSLATION_REQUEST_TIMEOUT` and `TRANSLATION_STREAM_IDLE_TIMEOUT`.
- `library/src/translator/openai.rs:209-220` wraps stream creation and each `stream.next()` call with timeouts, but not the total stream duration.
- `library/src/translator/gemini.rs:209-228` does the same for `execute_stream()` and `stream.try_next()`.
- `site/src-tauri/src/app/translation_queue.rs:108-146` shows a single `while let Ok(request)` worker loop awaiting one request at a time.

**Affected code paths:** `TranslationQueue::init`, `handle_request`, `OpenAITranslator::get_translation`, `GeminiTranslator::get_translation`.

**Suggested modeling approach:**
- Variables: queued requests, single `activeReq`, worker program counter, per-request progress/keepalive counters.
- Actions: enqueue request, start worker, provider request open, provider chunk, provider keepalive, request/idle timeout, parse success, parse failure, save complete.
- Property: a tracked queued request should eventually leave the queued state once it has been enqueued.

**Priority:** High

---

### Family 2: Provider failures collapse into the same terminal status shape as success

**Mechanism:** all failure exits from `handle_request()` are caught by the translation worker loop, which logs the error and publishes `TranslationStatus { is_complete: true, progress_chars: 0, expected_chars: 0 }`. Successful saves also publish `is_complete: true`. The frontend treats `is_complete` as success and refreshes the paragraph without any error discriminator.

**Evidence:**
- Worker error handler publishes terminal complete status on failure: `site/src-tauri/src/app/translation_queue.rs:127-139`.
- Successful saver path publishes the same terminal shape: `site/src-tauri/src/app/translation_queue.rs:393-398`, `414-419`.
- `TranslationStatus` contains only `request_id`, `progress_chars`, `expected_chars`, and `is_complete`: `site/src-tauri/src/app/translation_queue.rs:41-47`, `site/src/lib/data/library.ts:29-34`.
- Frontend treats `is_complete` as done and refreshes rather than surfacing an error: `site/src/lib/bookView/ParagraphView.svelte:50-59`.

**Affected code paths:** translation worker error handler, saver completion path, frontend status polling/rendering.

**Suggested modeling approach:**
- Variables: actual request outcome vs frontend-visible terminal status.
- Actions: provider/parse failure, success save completion, terminal status publication.
- Invariant: failed requests should not be represented only as generic `is_complete = true`.

**Priority:** High

---

### Family 3: A single chunk-level stream failure should be tolerated and retried

**Mechanism:** the spec assumes the desired transport policy rather than the current implementation split: one chunk-level stream failure should be tolerated and the request should remain active for a retry / subsequent stream step. This removes provider-specific semantics from the model and treats chunk failure handling as a single logical policy.

**Evidence:**
- OpenAI currently logs and continues on a chunk error: `library/src/translator/openai.rs:222-236`.
- Gemini currently propagates the chunk error immediately: `library/src/translator/gemini.rs:225-228`.
- The code should be normalized later; for now the spec models the intended common behavior instead of the existing divergence.

**Affected code paths:** `OpenAITranslator::get_translation`, `GeminiTranslator::get_translation`.

**Suggested modeling approach:**
- Variables: provider kind, whether the one tolerated chunk error has already been spent, current streaming/request state.
- Actions: first chunk error stays active and records retry budget spent; second chunk error fails; subsequent parse/save transitions remain unchanged.
- Invariant: the first chunk-level error must not force terminal failure.

**Priority:** Medium

## 3. Modeling recommendations

### 3.1 Model

| What | Why | How |
|---|---|---|
| Single translation worker + queue | Family 1 depends on one stalled request blocking later requests | Model `queue`, `activeReq`, and a worker program counter |
| Provider kind | Keeps traces aligned with implementation and leaves room for later code fixes | Record `reqProvider[r]` as OpenAI or Gemini |
| Stream progress and keepalive | Family 1 depends on non-progress stream steps | Track `progressChars` and `keepAliveCount` |
| Actual request outcome vs published status | Family 2 is about observability collapse | Keep `reqOutcome` separate from `statusComplete` |
| Malformed parse result | User-requested scenario and normal terminal branch | Model parse success vs parse failure branches |

### 3.2 Do not model

| What | Why |
|---|---|
| Cache-hit fast path | It bypasses the provider API entirely; this brief is scoped to provider interaction |
| Full paragraph/dictionary content structure | The provider protocol risks are about transport and terminal outcomes, not NLP semantics |
| Book/file watcher races | Already covered by the frontend/backend interaction and sync specs; not needed for provider outcome modeling |
| Frontend polling intervals | They do not change backend provider semantics |

## 4. Proposed extensions

| Extension | Variables | Purpose | Bug family |
|---|---|---|---|
| Queue + active request | `queue`, `activeReq`, `workerPc` | Expose single-worker blocking during provider streaming | Family 1 |
| Stream progress accounting | `bufferKind`, `progressChars`, `keepAliveCount` | Represent chunk delivery, malformed output, and slow keepalives | Family 1 |
| Retry-once chunk error semantics | `reqProvider`, `sawChunkError` | Model a single tolerated chunk failure before terminal transport failure | Family 3 |
| Outcome/status split | `reqOutcome`, `statusComplete` | Check whether failures remain distinguishable at the app boundary | Family 2 |

## 5. Proposed invariants and properties

| Name | Type | Description | Targets |
|---|---|---|---|
| `TypeOK` | Structural | All queue/request variables remain well-typed | All |
| `WorkerDiscipline` | Structural | `activeReq = Nil` iff `workerPc = "idle"` | All |
| `QueueMatchesState` | Structural | Queued requests and `reqState = "queued"` stay in sync | All |
| `SuccessRequiresValidBuffer` | Safety | A request can succeed only after a valid buffered response | General correctness |
| `FailureStatusNotComplete` | Safety | Failed requests are not represented only as generic complete statuses | Family 2 |
| `FirstChunkErrorIsRetried` | Safety | A request remains active after its first chunk-level transport error | Family 3 |
| `TrackedQueuedRequestEventuallyLeavesQueue` | Liveness | Once the tracked queued request is enqueued, it eventually stops being queued | Family 1 |

## 6. Findings pending verification

### 6.1 Model-checkable

| ID | Description | Expected invariant/property violation | Bug family |
|---|---|---|---|
| MC-P1 | Request `r1` keeps receiving keepalive/non-progress stream steps while `r2` is queued behind it | `TrackedQueuedRequestEventuallyLeavesQueue` | Family 1 |
| MC-P2 | Provider request times out / parse fails and the worker still publishes `is_complete = true` | `FailureStatusNotComplete` | Family 2 |
| MC-P3 | A request sees one chunk-level stream error and should remain active for retry rather than fail immediately | `FirstChunkErrorIsRetried` | Family 3 |

### 6.2 Code-review-only

| ID | Description | Suggested action |
|---|---|---|
| CR-P1 | Add an overall wall-clock deadline for the full streaming request, not only per-chunk idle timeout | Introduce a total stream timeout in `get_translation()` |
| CR-P2 | Distinguish terminal success from terminal failure in `TranslationStatus` | Add an explicit status/result field or error code |
| CR-P3 | Align OpenAI and Gemini stream-error semantics with the retry-once model | Implement one tolerated chunk failure before terminal failure |

## 7. Reference pointers

- `site/src-tauri/src/app.rs:241-274`
- `site/src-tauri/src/app/translation_queue.rs:75-218`
- `site/src-tauri/src/app/translation_queue.rs:221-370`
- `site/src-tauri/src/app/translation_queue.rs:381-449`
- `site/src/lib/data/library.ts:29-112`
- `site/src/lib/bookView/ParagraphView.svelte:46-64`
- `library/src/translator.rs:16-17`
- `library/src/translator/openai.rs:184-259`
- `library/src/translator/gemini.rs:198-255`
