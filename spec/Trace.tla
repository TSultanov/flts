--------------------------- MODULE Trace ---------------------------
(*
 * Trace validation wrapper for the FLTS sync spec.
 *
 * Trace format: NDJSON with tag="trace" and event records containing:
 *   - event.name: spec action name
 *   - event.arg: optional action arguments
 *   - event.state: post-action snapshot
 *)

EXTENDS base, Json, IOUtils, Sequences, TLC

\* ============================================================================
\* TRACE LOADING
\* ============================================================================

JsonFile ==
    IF "JSON" \in DOMAIN IOEnv THEN IOEnv.JSON
    ELSE "../traces/trace.ndjson"

TraceLog ==
    TLCEval(
        LET all == ndJsonDeserialize(JsonFile)
        IN SelectSeq(all, LAMBDA x :
            /\ "tag" \in DOMAIN x
            /\ x.tag = "trace"
            /\ "event" \in DOMAIN x))

ASSUME Len(TraceLog) > 0

\* ============================================================================
\* TRACE CURSOR
\* ============================================================================

VARIABLE l

traceVars == <<l>>

logline == TraceLog[l]

\* ============================================================================
\* HELPERS
\* ============================================================================

TraceMaybe(v) ==
    IF v = "nil" \/ v = "" \/ v = "null"
    THEN Nil
    ELSE v

IsEvent(name) ==
    /\ l <= Len(TraceLog)
    /\ logline.event.name = name

ValidatePostState ==
    /\ bookMain'.mtime = logline.event.state.bookMainMTime
    /\ Cardinality(bookConflicts') = logline.event.state.bookConflictCount
    /\ stateMain'.reading = TraceMaybe(logline.event.state.stateMainReading)
    /\ stateMain'.folder = TraceMaybe(logline.event.state.stateMainFolder)
    /\ Cardinality(stateConflicts') = logline.event.state.stateConflictCount
    /\ translationMain'.mtime = logline.event.state.translationMainMTime
    /\ Cardinality(translationMain'.versions) = logline.event.state.translationVersionCount
    /\ Cardinality(translationConflicts') = logline.event.state.translationConflictCount
    /\ Cardinality(dictionaryMain'.entries) = logline.event.state.dictionaryEntryCount
    /\ bookSaveStage' = logline.event.state.bookSaveStage
    /\ translationSaveStage' = logline.event.state.translationSaveStage
    /\ stateOpKind' = logline.event.state.stateOpKind

StepTrace == l' = l + 1

\* ============================================================================
\* ACTION WRAPPERS
\* ============================================================================

LoadBookFromMetadataIfLogged ==
    /\ IsEvent("LoadBookFromMetadata")
    /\ LoadBookFromMetadata
    /\ ValidatePostState
    /\ StepTrace

SaveBookBeginIfLogged ==
    /\ IsEvent("SaveBookBegin")
    /\ SaveBookBegin
    /\ ValidatePostState
    /\ StepTrace

SaveBookFinishIfLogged ==
    /\ IsEvent("SaveBookFinish")
    /\ SaveBookFinish
    /\ ValidatePostState
    /\ StepTrace

UpdateReadingStateReloadIfLogged ==
    /\ IsEvent("UpdateReadingStateReload")
    /\ "arg" \in DOMAIN logline.event
    /\ UpdateReadingStateReload(TraceMaybe(logline.event.arg.reading))
    /\ ValidatePostState
    /\ StepTrace

UpdateReadingStatePersistIfLogged ==
    /\ IsEvent("UpdateReadingStatePersist")
    /\ UpdateReadingStatePersist
    /\ ValidatePostState
    /\ StepTrace

UpdateFolderPathReloadIfLogged ==
    /\ IsEvent("UpdateFolderPathReload")
    /\ "arg" \in DOMAIN logline.event
    /\ UpdateFolderPathReload(TraceMaybe(logline.event.arg.folder))
    /\ ValidatePostState
    /\ StepTrace

UpdateFolderPathPersistIfLogged ==
    /\ IsEvent("UpdateFolderPathPersist")
    /\ UpdateFolderPathPersist
    /\ ValidatePostState
    /\ StepTrace

ResolveReadingStateFileIfLogged ==
    /\ IsEvent("ResolveReadingStateFile")
    /\ ResolveReadingStateFile
    /\ ValidatePostState
    /\ StepTrace

LoadTranslationFromMetadataIfLogged ==
    /\ IsEvent("LoadTranslationFromMetadata")
    /\ LoadTranslationFromMetadata
    /\ ValidatePostState
    /\ StepTrace

SaveTranslationBeginIfLogged ==
    /\ IsEvent("SaveTranslationBegin")
    /\ SaveTranslationBegin
    /\ ValidatePostState
    /\ StepTrace

SaveTranslationFinishIfLogged ==
    /\ IsEvent("SaveTranslationFinish")
    /\ SaveTranslationFinish
    /\ ValidatePostState
    /\ StepTrace

LoadDictionaryFromMetadataIfLogged ==
    /\ IsEvent("LoadDictionaryFromMetadata")
    /\ LoadDictionaryFromMetadata
    /\ ValidatePostState
    /\ StepTrace

\* ============================================================================
\* SILENT ACTIONS
\* ============================================================================

\* Silent setup for load-time book conflict traces.
SilentInjectBookConflict ==
    /\ l <= Len(TraceLog)
    /\ logline.event.name = "LoadBookFromMetadata"
    /\ bookConflicts = {}
    /\ \E e \in BookEdit : InjectBookConflict(e)
    /\ UNCHANGED l

\* Silent setup for state conflict traces.
SilentInjectStateConflict ==
    /\ l <= Len(TraceLog)
    /\ logline.event.name = "ResolveReadingStateFile"
    /\ stateConflicts = {}
    /\ \/ \E r \in ReadingPos : InjectStateReadingConflict(r)
       \/ \E f \in FolderVal : InjectStateFolderConflict(f)
    /\ UNCHANGED l

\* Silent setup for translation merge traces.
SilentInjectTranslationConflict ==
    /\ l <= Len(TraceLog)
    /\ logline.event.name = "LoadTranslationFromMetadata"
    /\ translationConflicts = {}
    /\ \E vid \in VersionId :
       \E ts \in 0..MaxTime :
       \E words \in SUBSET VisibleWord :
           InjectTranslationConflict(vid, ts, words)
    /\ UNCHANGED l

\* Silent setup for save traces that begin after local in-memory mutation.
SilentEditMemoryBook ==
    /\ l <= Len(TraceLog)
    /\ logline.event.name = "SaveBookBegin"
    /\ memBook.edits = {}
    /\ \E e \in BookEdit : EditMemoryBook(e)
    /\ UNCHANGED l

SilentAddMemoryTranslationVersion ==
    /\ l <= Len(TraceLog)
    /\ logline.event.name = "SaveTranslationBegin"
    /\ memTranslation.versions = {}
    /\ \E vid \in VersionId :
       \E ts \in 0..MaxTime :
       \E words \in SUBSET VisibleWord :
           AddMemoryTranslationVersion(vid, ts, words)
    /\ UNCHANGED l

\* ============================================================================
\* INITIALIZATION / NEXT
\* ============================================================================

TraceInit ==
    /\ Init
    /\ l = 1

TraceNext ==
    \/ LoadBookFromMetadataIfLogged
    \/ SaveBookBeginIfLogged
    \/ SaveBookFinishIfLogged
    \/ UpdateReadingStateReloadIfLogged
    \/ UpdateReadingStatePersistIfLogged
    \/ UpdateFolderPathReloadIfLogged
    \/ UpdateFolderPathPersistIfLogged
    \/ ResolveReadingStateFileIfLogged
    \/ LoadTranslationFromMetadataIfLogged
    \/ SaveTranslationBeginIfLogged
    \/ SaveTranslationFinishIfLogged
    \/ LoadDictionaryFromMetadataIfLogged

    \/ SilentInjectBookConflict
    \/ SilentInjectStateConflict
    \/ SilentInjectTranslationConflict
    \/ SilentEditMemoryBook
    \/ SilentAddMemoryTranslationVersion

    \/ /\ l > Len(TraceLog)
       /\ UNCHANGED <<vars, l>>

TraceMatched == <>(l > Len(TraceLog))

=============================================================================
