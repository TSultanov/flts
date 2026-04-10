--------------------------- MODULE Trace ---------------------------
\* Trace validation spec for the FLTS provider interaction model.

EXTENDS base, Json, IOUtils, Sequences, TLC

\* ========================================================================
\* Trace loading
\* ========================================================================

JsonFile ==
    IF "JSON" \in DOMAIN IOEnv THEN IOEnv.JSON
    ELSE "../../traces/provider.ndjson"

TraceLog == TLCEval(
    LET all == ndJsonDeserialize(JsonFile)
    IN SelectSeq(all, LAMBDA x :
        /\ "event" \in DOMAIN x
        /\ x.event /= ""))

ASSUME Len(TraceLog) > 0

\* ========================================================================
\* Trace cursor
\* ========================================================================

VARIABLE l

traceVars == <<l>>
logline == TraceLog[l]

\* ========================================================================
\* Helpers
\* ========================================================================

RequestOf(name) ==
    CHOOSE r \in Request : ToString(r) = name

ProviderOf(name) ==
    IF name = "openai" THEN OpenAI ELSE Gemini

TraceActiveReq ==
    IF logline.state.activeReq = ""
    THEN Nil
    ELSE RequestOf(logline.state.activeReq)

ValidateRequestState(r) ==
    /\ activeReq' = TraceActiveReq
    /\ workerPc' = logline.state.workerPc
    /\ reqState'[r] = logline.state.reqState
    /\ reqOutcome'[r] = logline.state.reqOutcome
    /\ statusOutcome'[r] = logline.state.statusOutcome
    /\ progressChars'[r] = logline.state.progressChars
    /\ bufferKind'[r] = logline.state.bufferKind

StepTrace == l' = l + 1

IsEvent(name) ==
    /\ l <= Len(TraceLog)
    /\ logline.event = name

\* ========================================================================
\* Trace init
\* ========================================================================

TraceInit ==
    /\ Init
    /\ l = 1

\* ========================================================================
\* Event wrappers
\* ========================================================================

TraceQueueRequest ==
    /\ IsEvent("QueueRequest")
    /\ LET r == RequestOf(logline.req)
           p == ProviderOf(logline.provider) IN
       /\ QueueRequest(r, p)
       /\ ValidateRequestState(r)
       /\ StepTrace

TraceStartHandleRequest ==
    /\ IsEvent("StartHandleRequest")
    /\ StartHandleRequest
    /\ LET r == RequestOf(logline.req) IN
       /\ ValidateRequestState(r)
       /\ StepTrace

TraceHandleRequestReadParagraph ==
    /\ IsEvent("HandleRequestReadParagraph")
    /\ HandleRequestReadParagraph
    /\ LET r == RequestOf(logline.req) IN
       /\ ValidateRequestState(r)
       /\ StepTrace

TraceGetTranslationRequestOpen ==
    /\ IsEvent("GetTranslationRequestOpen")
    /\ GetTranslationRequestOpen
    /\ LET r == RequestOf(logline.req) IN
       /\ ValidateRequestState(r)
       /\ StepTrace

TraceGetTranslationRequestTimeout ==
    /\ IsEvent("GetTranslationRequestTimeout")
    /\ GetTranslationRequestTimeout
    /\ LET r == RequestOf(logline.req) IN
       /\ ValidateRequestState(r)
       /\ StepTrace

TraceGetTranslationStreamChunk ==
    /\ IsEvent("GetTranslationStreamChunk")
    /\ LET r == RequestOf(logline.req)
           kind == logline.kind IN
       /\ GetTranslationStreamChunk(kind)
       /\ ValidateRequestState(r)
       /\ StepTrace

TraceProviderKeepAlive ==
    /\ IsEvent("ProviderKeepAlive")
    /\ ProviderKeepAlive
    /\ LET r == RequestOf(logline.req) IN
       /\ ValidateRequestState(r)
       /\ keepAliveCount'[r] = logline.state.keepAliveCount
       /\ StepTrace

TraceStreamChunkErrorRetry ==
    /\ IsEvent("StreamChunkErrorRetry")
    /\ StreamChunkErrorRetry
    /\ LET r == RequestOf(logline.req) IN
        /\ ValidateRequestState(r)
        /\ sawChunkError'[r]
        /\ StepTrace

TraceStreamChunkErrorFail ==
    /\ IsEvent("StreamChunkErrorFail")
    /\ StreamChunkErrorFail
    /\ LET r == RequestOf(logline.req) IN
        /\ ValidateRequestState(r)
        /\ StepTrace

TraceGetTranslationIdleTimeout ==
    /\ IsEvent("GetTranslationIdleTimeout")
    /\ GetTranslationIdleTimeout
    /\ LET r == RequestOf(logline.req) IN
       /\ ValidateRequestState(r)
       /\ StepTrace

TraceGetTranslationEmptyResponse ==
    /\ IsEvent("GetTranslationEmptyResponse")
    /\ GetTranslationEmptyResponse
    /\ LET r == RequestOf(logline.req) IN
       /\ ValidateRequestState(r)
       /\ StepTrace

TraceGetTranslationParseFailure ==
    /\ IsEvent("GetTranslationParseFailure")
    /\ GetTranslationParseFailure
    /\ LET r == RequestOf(logline.req) IN
       /\ ValidateRequestState(r)
       /\ StepTrace

TraceGetTranslationParseSuccess ==
    /\ IsEvent("GetTranslationParseSuccess")
    /\ GetTranslationParseSuccess
    /\ LET r == RequestOf(logline.req) IN
       /\ ValidateRequestState(r)
       /\ StepTrace

TraceRunSaverComplete ==
    /\ IsEvent("RunSaverComplete")
    /\ RunSaverComplete
    /\ LET r == RequestOf(logline.req) IN
       /\ ValidateRequestState(r)
       /\ StepTrace

TraceNext ==
    \/ /\ l <= Len(TraceLog)
       /\ \/ TraceQueueRequest
          \/ TraceStartHandleRequest
          \/ TraceHandleRequestReadParagraph
          \/ TraceGetTranslationRequestOpen
          \/ TraceGetTranslationRequestTimeout
          \/ TraceGetTranslationStreamChunk
          \/ TraceProviderKeepAlive
          \/ TraceStreamChunkErrorRetry
          \/ TraceStreamChunkErrorFail
          \/ TraceGetTranslationIdleTimeout
          \/ TraceGetTranslationEmptyResponse
          \/ TraceGetTranslationParseFailure
          \/ TraceGetTranslationParseSuccess
          \/ TraceRunSaverComplete
    \/ /\ l > Len(TraceLog)
       /\ UNCHANGED <<vars, l>>

TraceSpec ==
    /\ TraceInit
    /\ [][TraceNext]_<<vars, l>>
    /\ WF_<<vars, l>>(TraceNext)

TraceMatched == <>(l > Len(TraceLog))

TraceTypeOK == TypeOK
TraceWorkerDiscipline == WorkerDiscipline
TraceSuccessRequiresValidBuffer == SuccessRequiresValidBuffer

=============================================================================
