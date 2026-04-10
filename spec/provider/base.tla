---------------------------- MODULE base ----------------------------
\* TLA+ specification of the FLTS translation queue <-> LLM provider boundary.
\*
\* Scope:
\*   - queueing a translation request into the single translation worker
\*   - provider request setup and streaming response handling
\*   - malformed/empty/transport/timeout outcomes
\*   - terminal status publication semantics
\*
\* Bug families:
\*   F1 - non-terminating slow streams block later queued requests
\*   F2 - provider failures collapse into generic complete statuses
\*   F3 - a single chunk error is tolerated and retried

EXTENDS Naturals, Sequences, FiniteSets, TLC

\* ========================================================================
\* Constants
\* ========================================================================

CONSTANTS
    Request,
    OpenAI,
    Gemini,
    Nil,
    TrackedQueuedReq

ASSUME Request /= {}
ASSUME OpenAI /= Gemini
ASSUME TrackedQueuedReq \in Request

ProviderSet == {OpenAI, Gemini}
FailureOutcome == {"request_timeout", "idle_timeout",
                   "transport_error", "parse_error", "empty_response"}
TerminalOutcome == FailureOutcome \union {"success"}

\* ========================================================================
\* Variables
\* ========================================================================

VARIABLE queue              \* Seq(Request) queued for the single worker
VARIABLE activeReq          \* Request \cup {Nil}
VARIABLE workerPc           \* "idle" | "read" | "provider_init" | "provider_stream" | "save"

VARIABLE reqState           \* [Request -> {"new","queued","active","succeeded","failed"}]
VARIABLE reqProvider        \* [Request -> ProviderSet \cup {Nil}]
VARIABLE bufferKind         \* [Request -> {"none","partial","valid","malformed"}]
VARIABLE progressChars      \* [Request -> Nat]
VARIABLE keepAliveCount     \* [Request -> Nat]
VARIABLE sawChunkError      \* [Request -> BOOLEAN]

VARIABLE reqOutcome         \* [Request -> TerminalOutcome \cup {"none"}]
VARIABLE statusComplete     \* [Request -> BOOLEAN]

queueVars == <<queue, activeReq, workerPc>>
requestVars == <<reqState, reqProvider, bufferKind,
                 progressChars, keepAliveCount, sawChunkError,
                 reqOutcome, statusComplete>>
vars == <<queueVars, requestVars>>

\* ========================================================================
\* Helpers
\* ========================================================================

Elems(s) == {s[i] : i \in 1..Len(s)}

FailRequest(r, outcomeVal) ==
    /\ reqState' = [reqState EXCEPT ![r] = "failed"]
    /\ reqOutcome' = [reqOutcome EXCEPT ![r] = outcomeVal]
    /\ statusComplete' = [statusComplete EXCEPT ![r] = TRUE]
    /\ activeReq' = Nil
    /\ workerPc' = "idle"
    /\ UNCHANGED <<queue, reqProvider, bufferKind,
                   progressChars, keepAliveCount, sawChunkError>>

SucceedRequest(r) ==
    /\ reqState' = [reqState EXCEPT ![r] = "succeeded"]
    /\ reqOutcome' = [reqOutcome EXCEPT ![r] = "success"]
    /\ statusComplete' = [statusComplete EXCEPT ![r] = TRUE]
    /\ activeReq' = Nil
    /\ workerPc' = "idle"
    /\ UNCHANGED <<queue, reqProvider, bufferKind,
                   progressChars, keepAliveCount, sawChunkError>>

\* ========================================================================
\* Init
\* ========================================================================

Init ==
    /\ queue = << >>
    /\ activeReq = Nil
    /\ workerPc = "idle"
    /\ reqState = [r \in Request |-> "new"]
    /\ reqProvider = [r \in Request |-> Nil]
    /\ bufferKind = [r \in Request |-> "none"]
    /\ progressChars = [r \in Request |-> 0]
    /\ keepAliveCount = [r \in Request |-> 0]
    /\ sawChunkError = [r \in Request |-> FALSE]
    /\ reqOutcome = [r \in Request |-> "none"]
    /\ statusComplete = [r \in Request |-> FALSE]

\* ========================================================================
\* Actions
\* ========================================================================

\* ------------------------------------------------------------------------
\* QueueRequest
\* Models AppState::translate_paragraph() -> TranslationQueue::translate().
\* References:
\*   - site/src-tauri/src/app.rs:263-274
\*   - site/src-tauri/src/app/translation_queue.rs:160-199
\* ------------------------------------------------------------------------
QueueRequest(r, p) ==
    /\ r \in Request
    /\ p \in ProviderSet
    /\ reqState[r] = "new"
    /\ r \notin Elems(queue)
    /\ r /= activeReq
    /\ queue' = Append(queue, r)
    /\ reqState' = [reqState EXCEPT ![r] = "queued"]
    /\ reqProvider' = [reqProvider EXCEPT ![r] = p]
    /\ UNCHANGED <<activeReq, workerPc, bufferKind,
                   progressChars, keepAliveCount, sawChunkError,
                   reqOutcome, statusComplete>>

\* ------------------------------------------------------------------------
\* StartHandleRequest
\* Models the single worker loop pulling one request and awaiting it to
\* completion before reading the next queued request.
\* Reference: site/src-tauri/src/app/translation_queue.rs:108-146
\* ------------------------------------------------------------------------
StartHandleRequest ==
    /\ workerPc = "idle"
    /\ activeReq = Nil
    /\ Len(queue) > 0
    /\ LET r == Head(queue) IN
       /\ queue' = Tail(queue)
       /\ activeReq' = r
       /\ workerPc' = "read"
       /\ reqState' = [reqState EXCEPT ![r] = "active"]
    /\ UNCHANGED <<reqProvider, bufferKind, progressChars,
                   keepAliveCount, sawChunkError, reqOutcome, statusComplete>>

\* ------------------------------------------------------------------------
\* HandleRequestReadParagraph
\* Models the pre-provider setup inside handle_request(), up to calling the
\* provider-specific translator.
\* References:
\*   - site/src-tauri/src/app/translation_queue.rs:232-272
\* ------------------------------------------------------------------------
HandleRequestReadParagraph ==
    /\ activeReq /= Nil
    /\ workerPc = "read"
    /\ LET r == activeReq IN
       /\ workerPc' = "provider_init"
       /\ bufferKind' = [bufferKind EXCEPT ![r] = "none"]
       /\ progressChars' = [progressChars EXCEPT ![r] = 0]
       /\ keepAliveCount' = [keepAliveCount EXCEPT ![r] = 0]
       /\ sawChunkError' = [sawChunkError EXCEPT ![r] = FALSE]
    /\ UNCHANGED <<queue, activeReq, reqState, reqProvider,
                   reqOutcome, statusComplete>>

\* ------------------------------------------------------------------------
\* GetTranslationRequestOpen
\* Models successful provider stream creation after request setup timeout.
\* References:
\*   - library/src/translator/openai.rs:184-214
\*   - library/src/translator/gemini.rs:209-221
\* ------------------------------------------------------------------------
GetTranslationRequestOpen ==
    /\ activeReq /= Nil
    /\ workerPc = "provider_init"
    /\ workerPc' = "provider_stream"
    /\ UNCHANGED <<queue, activeReq, reqState, reqProvider, bufferKind,
                   progressChars, keepAliveCount, sawChunkError,
                   reqOutcome, statusComplete>>

\* ------------------------------------------------------------------------
\* GetTranslationRequestTimeout
\* Models request setup timing out before a stream object is returned.
\* References:
\*   - library/src/translator.rs:16-17
\*   - library/src/translator/openai.rs:209-214
\*   - library/src/translator/gemini.rs:209-221
\* ------------------------------------------------------------------------
GetTranslationRequestTimeout ==
    /\ activeReq /= Nil
    /\ workerPc = "provider_init"
    /\ FailRequest(activeReq, "request_timeout")

\* ------------------------------------------------------------------------
\* GetTranslationStreamChunk
\* Models a stream step that appends response content and reports progress.
\* References:
\*   - library/src/translator/openai.rs:217-230
\*   - library/src/translator/gemini.rs:225-237
\* ------------------------------------------------------------------------
GetTranslationStreamChunk(kind) ==
    /\ kind \in {"partial", "valid"}
    /\ activeReq /= Nil
    /\ workerPc = "provider_stream"
    /\ LET r == activeReq IN
       /\ bufferKind' =
            [bufferKind EXCEPT ![r] =
                IF kind = "valid"
                THEN "valid"
                ELSE IF bufferKind[r] = "valid" THEN "valid" ELSE "partial"]
       /\ progressChars' = [progressChars EXCEPT ![r] = @ + 1]
    /\ UNCHANGED <<queue, activeReq, workerPc, reqState, reqProvider,
                   keepAliveCount, sawChunkError, reqOutcome, statusComplete>>

\* ------------------------------------------------------------------------
\* ProviderKeepAlive
\* Models a slow stream that remains active without producing enough terminal
\* information to resolve the request.
\* References:
\*   - library/src/translator/openai.rs:218-221
\*   - library/src/translator/gemini.rs:226-229
\* ------------------------------------------------------------------------
ProviderKeepAlive ==
    /\ activeReq /= Nil
    /\ workerPc = "provider_stream"
    /\ keepAliveCount' = [keepAliveCount EXCEPT ![activeReq] = @ + 1]
    /\ UNCHANGED <<queue, activeReq, workerPc, reqState, reqProvider,
                   bufferKind, progressChars, sawChunkError,
                   reqOutcome, statusComplete>>

\* ------------------------------------------------------------------------
\* StreamChunkErrorRetry
\* Models the desired policy: the first chunk-level stream error is tolerated
\* and the request remains active for retry / later stream steps.
\* References:
\*   - library/src/translator/openai.rs:222-236
\*   - library/src/translator/gemini.rs:225-228 (desired future behavior)
\* ------------------------------------------------------------------------
StreamChunkErrorRetry ==
    /\ activeReq /= Nil
    /\ workerPc = "provider_stream"
    /\ ~sawChunkError[activeReq]
    /\ sawChunkError' = [sawChunkError EXCEPT ![activeReq] = TRUE]
    /\ UNCHANGED <<queue, activeReq, workerPc, reqState, reqProvider,
                    bufferKind, progressChars, keepAliveCount,
                    reqOutcome, statusComplete>>

\* ------------------------------------------------------------------------
\* StreamChunkErrorFail
\* Models transport failure after the one tolerated chunk error has already
\* been spent.
\* ------------------------------------------------------------------------
StreamChunkErrorFail ==
    /\ activeReq /= Nil
    /\ workerPc = "provider_stream"
    /\ sawChunkError[activeReq]
    /\ FailRequest(activeReq, "transport_error")

\* ------------------------------------------------------------------------
\* GetTranslationIdleTimeout
\* Models the per-chunk idle timeout firing while the request is streaming.
\* References:
\*   - library/src/translator.rs:16-17
\*   - library/src/translator/openai.rs:218-220
\*   - library/src/translator/gemini.rs:226-228
\* ------------------------------------------------------------------------
GetTranslationIdleTimeout ==
    /\ activeReq /= Nil
    /\ workerPc = "provider_stream"
    /\ FailRequest(activeReq, "idle_timeout")

\* ------------------------------------------------------------------------
\* GetTranslationEmptyResponse
\* Models EOF with no accumulated content.
\* References:
\*   - library/src/translator/openai.rs:239-241
\*   - library/src/translator/gemini.rs:239-241
\* ------------------------------------------------------------------------
GetTranslationEmptyResponse ==
    /\ activeReq /= Nil
    /\ workerPc = "provider_stream"
    /\ progressChars[activeReq] = 0
    /\ FailRequest(activeReq, "empty_response")

\* ------------------------------------------------------------------------
\* GetTranslationParseFailure
\* Models malformed or incomplete buffered content failing serde parsing.
\* References:
\*   - library/src/translator/openai.rs:239-243
\*   - library/src/translator/gemini.rs:239-243
\* ------------------------------------------------------------------------
GetTranslationParseFailure ==
    /\ activeReq /= Nil
    /\ workerPc = "provider_stream"
    /\ progressChars[activeReq] > 0
    /\ LET r == activeReq IN
       /\ bufferKind' = [bufferKind EXCEPT ![r] = "malformed"]
       /\ reqState' = [reqState EXCEPT ![r] = "failed"]
       /\ reqOutcome' = [reqOutcome EXCEPT ![r] = "parse_error"]
       /\ statusComplete' = [statusComplete EXCEPT ![r] = TRUE]
       /\ activeReq' = Nil
       /\ workerPc' = "idle"
    /\ UNCHANGED <<queue, reqProvider, progressChars,
                   keepAliveCount, sawChunkError>>

\* ------------------------------------------------------------------------
\* GetTranslationParseSuccess
\* Models buffered JSON successfully parsing into ParagraphTranslation.
\* References:
\*   - library/src/translator/openai.rs:243-259
\*   - library/src/translator/gemini.rs:243-255
\*   - site/src-tauri/src/app/translation_queue.rs:316-368
\* ------------------------------------------------------------------------
GetTranslationParseSuccess ==
    /\ activeReq /= Nil
    /\ workerPc = "provider_stream"
    /\ bufferKind[activeReq] = "valid"
    /\ workerPc' = "save"
    /\ UNCHANGED <<queue, activeReq, reqState, reqProvider, bufferKind,
                   progressChars, keepAliveCount, sawChunkError,
                   reqOutcome, statusComplete>>

\* ------------------------------------------------------------------------
\* RunSaverComplete
\* Models the saver path publishing terminal completion on success.
\* References:
\*   - site/src-tauri/src/app/translation_queue.rs:363-398
\*   - site/src-tauri/src/app/translation_queue.rs:414-419
\* ------------------------------------------------------------------------
RunSaverComplete ==
    /\ activeReq /= Nil
    /\ workerPc = "save"
    /\ SucceedRequest(activeReq)

Next ==
    \/ \E r \in Request, p \in ProviderSet : QueueRequest(r, p)
    \/ StartHandleRequest
    \/ HandleRequestReadParagraph
    \/ GetTranslationRequestOpen
    \/ GetTranslationRequestTimeout
    \/ \E kind \in {"partial", "valid"} : GetTranslationStreamChunk(kind)
    \/ ProviderKeepAlive
    \/ StreamChunkErrorRetry
    \/ StreamChunkErrorFail
    \/ GetTranslationIdleTimeout
    \/ GetTranslationEmptyResponse
    \/ GetTranslationParseFailure
    \/ GetTranslationParseSuccess
    \/ RunSaverComplete

\* ========================================================================
\* Structural invariants
\* ========================================================================

TypeOK ==
    /\ queue \in Seq(Request)
    /\ activeReq \in Request \union {Nil}
    /\ workerPc \in {"idle", "read", "provider_init", "provider_stream", "save"}
    /\ reqState \in [Request -> {"new", "queued", "active", "succeeded", "failed"}]
    /\ reqProvider \in [Request -> ProviderSet \union {Nil}]
    /\ bufferKind \in [Request -> {"none", "partial", "valid", "malformed"}]
    /\ progressChars \in [Request -> Nat]
    /\ keepAliveCount \in [Request -> Nat]
    /\ sawChunkError \in [Request -> BOOLEAN]
    /\ reqOutcome \in [Request -> (TerminalOutcome \union {"none"})]
    /\ statusComplete \in [Request -> BOOLEAN]

WorkerDiscipline ==
    /\ (activeReq = Nil) = (workerPc = "idle")
    /\ activeReq /= Nil => reqState[activeReq] = "active"

QueueMatchesState ==
    /\ \A r \in Request : (r \in Elems(queue)) => reqState[r] = "queued"
    /\ \A r \in Request : (reqState[r] = "queued") => r \in Elems(queue)
    /\ activeReq = Nil \/ activeReq \notin Elems(queue)

SuccessRequiresValidBuffer ==
    \A r \in Request :
        reqOutcome[r] = "success" => bufferKind[r] = "valid"

\* ========================================================================
\* Bug-family properties
\* ========================================================================

FailureStatusNotComplete ==
    \A r \in Request :
        reqOutcome[r] \in FailureOutcome => ~statusComplete[r]

FirstChunkErrorIsRetried ==
    \A r \in Request :
        sawChunkError[r] /\ reqOutcome[r] = "none" => reqState[r] = "active"

TrackedQueuedRequestEventuallyLeavesQueue ==
    []((reqState[TrackedQueuedReq] = "queued") =>
        <>(reqState[TrackedQueuedReq] # "queued"))

\* ========================================================================
\* Specification
\* ========================================================================

Spec ==
    /\ Init
    /\ [][Next]_vars
    /\ WF_vars(StartHandleRequest)
    /\ WF_vars(HandleRequestReadParagraph)
    /\ WF_vars(GetTranslationParseSuccess)
    /\ WF_vars(GetTranslationParseFailure)
    /\ WF_vars(RunSaverComplete)

=============================================================================
