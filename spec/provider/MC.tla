----------------------------- MODULE MC -----------------------------
\* Model-checking wrapper for the FLTS provider interaction spec.
\*
\* Counter-bounds non-deterministic environment actions:
\*   - request injection
\*   - streaming chunk delivery
\*   - keepalive steps
\*   - timeout/error branches

EXTENDS base

B == INSTANCE base

CONSTANTS
    RequestLimit,
    ChunkLimit,
    KeepAliveLimit,
    TimeoutLimit,
    TotalTimeoutLimit,
    StreamErrorLimit

VARIABLE constraintCounters

faultVars == <<constraintCounters>>

\* ========================================================================
\* Bounded environment actions
\* ========================================================================

MCQueueRequest(r, p) ==
    /\ constraintCounters.request < RequestLimit
    /\ B!QueueRequest(r, p)
    /\ constraintCounters' = [constraintCounters EXCEPT !.request = @ + 1]

MCGetTranslationStreamChunk(kind) ==
    /\ constraintCounters.chunk < ChunkLimit
    /\ B!GetTranslationStreamChunk(kind)
    /\ constraintCounters' = [constraintCounters EXCEPT !.chunk = @ + 1]

MCProviderKeepAlive ==
    /\ constraintCounters.keepAlive < KeepAliveLimit
    /\ B!ProviderKeepAlive
    /\ constraintCounters' = [constraintCounters EXCEPT !.keepAlive = @ + 1]

MCGetTranslationRequestTimeout ==
    /\ constraintCounters.timeout < TimeoutLimit
    /\ B!GetTranslationRequestTimeout
    /\ constraintCounters' = [constraintCounters EXCEPT !.timeout = @ + 1]

MCGetTranslationIdleTimeout ==
    /\ constraintCounters.timeout < TimeoutLimit
    /\ B!GetTranslationIdleTimeout
    /\ constraintCounters' = [constraintCounters EXCEPT !.timeout = @ + 1]

MCGetTranslationTotalStreamTimeout ==
    /\ constraintCounters.totalTimeout < TotalTimeoutLimit
    /\ B!GetTranslationTotalStreamTimeout
    /\ constraintCounters' = [constraintCounters EXCEPT !.totalTimeout = @ + 1]

MCStreamChunkErrorRetry ==
    /\ constraintCounters.streamError < StreamErrorLimit
    /\ B!StreamChunkErrorRetry
    /\ constraintCounters' = [constraintCounters EXCEPT !.streamError = @ + 1]

MCStreamChunkErrorFail ==
    /\ constraintCounters.streamError < StreamErrorLimit
    /\ B!StreamChunkErrorFail
    /\ constraintCounters' = [constraintCounters EXCEPT !.streamError = @ + 1]

\* ========================================================================
\* Reactive actions
\* ========================================================================

MCStartHandleRequest ==
    /\ B!StartHandleRequest
    /\ UNCHANGED faultVars

MCHandleRequestReadParagraph ==
    /\ B!HandleRequestReadParagraph
    /\ UNCHANGED faultVars

MCGetTranslationRequestOpen ==
    /\ B!GetTranslationRequestOpen
    /\ UNCHANGED faultVars

MCGetTranslationEmptyResponse ==
    /\ B!GetTranslationEmptyResponse
    /\ UNCHANGED faultVars

MCGetTranslationParseFailure ==
    /\ B!GetTranslationParseFailure
    /\ UNCHANGED faultVars

MCGetTranslationParseSuccess ==
    /\ B!GetTranslationParseSuccess
    /\ UNCHANGED faultVars

MCRunSaverComplete ==
    /\ B!RunSaverComplete
    /\ UNCHANGED faultVars

\* ========================================================================
\* Init and Next
\* ========================================================================

MCInit ==
    /\ Init
    /\ constraintCounters = [
        request      |-> 0,
        chunk        |-> 0,
        keepAlive    |-> 0,
        timeout      |-> 0,
        totalTimeout |-> 0,
        streamError  |-> 0]

MCNext ==
    \/ \E r \in Request, p \in {OpenAI, Gemini} : MCQueueRequest(r, p)
    \/ MCStartHandleRequest
    \/ MCHandleRequestReadParagraph
    \/ MCGetTranslationRequestOpen
    \/ MCGetTranslationRequestTimeout
    \/ \E kind \in {"partial", "valid"} : MCGetTranslationStreamChunk(kind)
    \/ MCProviderKeepAlive
    \/ MCStreamChunkErrorRetry
    \/ MCStreamChunkErrorFail
    \/ MCGetTranslationIdleTimeout
    \/ MCGetTranslationTotalStreamTimeout
    \/ MCGetTranslationEmptyResponse
    \/ MCGetTranslationParseFailure
    \/ MCGetTranslationParseSuccess
    \/ MCRunSaverComplete

MCSpec ==
    /\ MCInit
    /\ [][MCNext]_<<vars, faultVars>>
    /\ WF_<<vars, faultVars>>(MCStartHandleRequest)
    /\ WF_<<vars, faultVars>>(MCHandleRequestReadParagraph)
    /\ WF_<<vars, faultVars>>(MCGetTranslationRequestOpen)
    /\ WF_<<vars, faultVars>>(MCGetTranslationParseSuccess)
    /\ WF_<<vars, faultVars>>(MCGetTranslationParseFailure)
    /\ WF_<<vars, faultVars>>(MCRunSaverComplete)
    /\ WF_<<vars, faultVars>>(MCGetTranslationIdleTimeout)
    /\ WF_<<vars, faultVars>>(MCGetTranslationTotalStreamTimeout)

\* ========================================================================
\* State constraint
\* ========================================================================

StateConstraint ==
    /\ \A r \in Request : progressChars[r] <= ChunkLimit
    /\ \A r \in Request : keepAliveCount[r] <= KeepAliveLimit

\* ========================================================================
\* Re-exported properties
\* ========================================================================

MCTypeOK == TypeOK
MCWorkerDiscipline == WorkerDiscipline
MCQueueMatchesState == QueueMatchesState
MCSuccessRequiresValidBuffer == SuccessRequiresValidBuffer

MCFailureStatusNotComplete == FailureStatusNotComplete
MCFirstChunkErrorIsRetried == FirstChunkErrorIsRetried
MCTrackedQueuedRequestEventuallyLeavesQueue ==
    TrackedQueuedRequestEventuallyLeavesQueue

=============================================================================
