# Instrumentation Spec — FLTS Provider Interaction

This document maps `spec/provider/base.tla` actions to backend code locations so
the harness can emit traces compatible with `spec/provider/Trace.tla`.

## 1. Trace event schema

Each trace line is NDJSON:

```json
{
  "event": "QueueRequest",
  "req": "r1",
  "provider": "openai",
  "state": {
    "activeReq": "",
    "workerPc": "idle",
    "reqState": "queued",
    "reqOutcome": "none",
    "statusComplete": false,
    "progressChars": 0,
    "bufferKind": "none",
    "keepAliveCount": 0
  }
}
```

### Common fields

| Field | Meaning | Spec variable |
|---|---|---|
| `req` | modeled request id (`r1`, `r2`, ...) | request parameter |
| `provider` | `"openai"` or `"gemini"` | `reqProvider[req]` |
| `state.activeReq` | active worker request or `""` | `activeReq` |
| `state.workerPc` | worker phase | `workerPc` |
| `state.reqState` | per-request lifecycle state | `reqState[req]` |
| `state.reqOutcome` | actual terminal outcome or `"none"` | `reqOutcome[req]` |
| `state.statusComplete` | frontend-visible terminal bit | `statusComplete[req]` |
| `state.progressChars` | accumulated streamed characters | `progressChars[req]` |
| `state.bufferKind` | `"none"`, `"partial"`, `"valid"`, `"malformed"` | `bufferKind[req]` |
| `state.keepAliveCount` | count of non-progress keepalive steps | `keepAliveCount[req]` |

## 2. Action-to-code mapping

### QueueRequest
- **Code location:** `site/src-tauri/src/app.rs:263-274`, `site/src-tauri/src/app/translation_queue.rs:160-199`
- **Trigger point:** after `translate_tx.send_async(...)` succeeds
- **Trace event name:** `QueueRequest`
- **Fields:** `req`, `provider`, full `state`
- **Notes:** request ids must be normalized by the preprocessor into `r1`, `r2`, ...

### StartHandleRequest
- **Code location:** `site/src-tauri/src/app/translation_queue.rs:108-125`
- **Trigger point:** immediately before calling `handle_request(...)`
- **Trace event name:** `StartHandleRequest`
- **Fields:** `req`, full `state`
- **Notes:** this marks the single worker selecting the next queued request

### HandleRequestReadParagraph
- **Code location:** `site/src-tauri/src/app/translation_queue.rs:232-272`
- **Trigger point:** after provider selection / translator construction succeeds
- **Trace event name:** `HandleRequestReadParagraph`
- **Fields:** `req`, `provider`, full `state`

### GetTranslationRequestOpen
- **Code location:** `library/src/translator/openai.rs:209-214`, `library/src/translator/gemini.rs:209-221`
- **Trigger point:** immediately after the timeout-wrapped stream object is returned
- **Trace event name:** `GetTranslationRequestOpen`
- **Fields:** `req`, `provider`, full `state`

### GetTranslationRequestTimeout
- **Code location:** `library/src/translator/openai.rs:209-214`, `library/src/translator/gemini.rs:209-221`
- **Trigger point:** in the timeout error branch before propagating the error
- **Trace event name:** `GetTranslationRequestTimeout`
- **Fields:** `req`, `provider`, full `state`

### GetTranslationStreamChunk
- **Code location:** `library/src/translator/openai.rs:217-230`, `library/src/translator/gemini.rs:225-237`
- **Trigger point:** after appending non-empty text and invoking the progress callback
- **Trace event name:** `GetTranslationStreamChunk`
- **Fields:** `req`, `provider`, `kind` (`"partial"` or `"valid"`), full `state`
- **Notes:** `kind` is a harness-level classification of the buffered stream after the append

### ProviderKeepAlive
- **Code location:** `library/src/translator/openai.rs:218-221`, `library/src/translator/gemini.rs:226-229`
- **Trigger point:** when the stream step returns but produces no useful progress while remaining open
- **Trace event name:** `ProviderKeepAlive`
- **Fields:** `req`, `provider`, full `state`
- **Notes:** this is the key event for the no-overall-deadline bug family

### StreamChunkErrorRetry
- **Code location:** `library/src/translator/openai.rs:222-236`, `library/src/translator/gemini.rs:225-228`
- **Trigger point:** on the first chunk-level stream failure, at the point where the harness chooses to model a tolerated retry step
- **Trace event name:** `StreamChunkErrorRetry`
- **Fields:** `req`, `provider`, full `state`
- **Notes:** this intentionally abstracts over the current implementation split and records the desired future behavior

### StreamChunkErrorFail
- **Code location:** `library/src/translator/openai.rs:222-236`, `library/src/translator/gemini.rs:225-228`
- **Trigger point:** on a subsequent chunk-level failure after the one tolerated retry has already been spent
- **Trace event name:** `StreamChunkErrorFail`
- **Fields:** `req`, `provider`, full `state`

### GetTranslationIdleTimeout
- **Code location:** `library/src/translator/openai.rs:218-220`, `library/src/translator/gemini.rs:226-228`
- **Trigger point:** in the idle-timeout branch before propagating the error
- **Trace event name:** `GetTranslationIdleTimeout`
- **Fields:** `req`, `provider`, full `state`

### GetTranslationEmptyResponse
- **Code location:** `library/src/translator/openai.rs:239-241`, `library/src/translator/gemini.rs:239-241`
- **Trigger point:** just before returning the `"returned empty content"` error
- **Trace event name:** `GetTranslationEmptyResponse`
- **Fields:** `req`, `provider`, full `state`

### GetTranslationParseFailure
- **Code location:** `library/src/translator/openai.rs:243`, `library/src/translator/gemini.rs:243`
- **Trigger point:** wrap `serde_json::from_str(&full_content)` so the failure branch emits before propagating
- **Trace event name:** `GetTranslationParseFailure`
- **Fields:** `req`, `provider`, full `state`
- **Notes:** set `state.bufferKind = "malformed"` if the parser rejects the final payload

### GetTranslationParseSuccess
- **Code location:** `library/src/translator/openai.rs:243-259`, `library/src/translator/gemini.rs:243-255`
- **Trigger point:** immediately after parse success and before returning to `handle_request`
- **Trace event name:** `GetTranslationParseSuccess`
- **Fields:** `req`, `provider`, full `state`

### RunSaverComplete
- **Code location:** `site/src-tauri/src/app/translation_queue.rs:363-398`, `414-419`
- **Trigger point:** immediately before sending the terminal `TranslationStatus { is_complete: true, ... }`
- **Trace event name:** `RunSaverComplete`
- **Fields:** `req`, full `state`
- **Notes:** this is the success-side terminal publication; failure-side terminal publications should already have been emitted by the failure action wrappers above

## 3. Special considerations

1. **Request id normalization:** runtime request ids are `usize` values. The harness preprocessor should map them deterministically to `r1`, `r2`, ... so `Trace.tla` can resolve them to model values.
2. **Provider names:** emit lowercase strings `"openai"` and `"gemini"` so the trace spec can map them directly to `OpenAI` and `Gemini`.
3. **Failure publication timing:** the implementation currently publishes failure completion in the outer worker error handler (`site/src-tauri/src/app/translation_queue.rs:127-139`), while success completion is published in the saver (`:393-398`, `:414-419`). If dedicated failure events are easier to emit at the error-handler boundary, keep the event names aligned with the corresponding spec actions and capture post-state after the terminal status send.
4. **Keepalive classification:** provider SDKs may not expose an explicit keepalive event. The harness may synthesize `ProviderKeepAlive` when a stream step returns without changing the buffered content but does not terminate the request.
5. **Retry abstraction:** `StreamChunkErrorRetry` is a spec-level abstraction for the intended code behavior, not a literal event that exists in both implementations today.
