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

ObservedMTimes ==
    ({TraceLog[i].event.state.bookMainMTime :
        i \in 1..Len(TraceLog)} \ {0})
    \cup
    ({TraceLog[i].event.state.translationMainMTime :
        i \in 1..Len(TraceLog)} \ {0})

DenseTime(t) ==
    IF t = 0
    THEN 0
    ELSE Cardinality({x \in ObservedMTimes : x <= t})

ExpectedBookMainMTime ==
    DenseTime(logline.event.state.bookMainMTime)

ExpectedTranslationMainMTime ==
    DenseTime(logline.event.state.translationMainMTime)

VersionUniverse ==
    {[id |-> vid, ts |-> ts, visible |-> visible] :
        vid \in VersionId, ts \in 0..MaxTime, visible \in SUBSET VisibleWord}

ChooseDictionaryEntries(n) ==
    CHOOSE es \in SUBSET DictEntry : Cardinality(es) = n

ExtendVersions(vs, n) ==
    CHOOSE ws \in SUBSET VersionUniverse :
        /\ vs \subseteq ws
        /\ Cardinality(ws) = n
        /\ Cardinality({v.ts : v \in ws}) = n

SnapshotVersions(n) ==
    ExtendVersions({}, n)

ValidateStages ==
    /\ bookSaveStage' = logline.event.state.bookSaveStage
    /\ translationSaveStage' = logline.event.state.translationSaveStage
    /\ stateOpKind' = logline.event.state.stateOpKind

TraceReadingArg ==
    IF "arg" \in DOMAIN logline.event /\ "reading" \in DOMAIN logline.event.arg
    THEN TraceMaybe(logline.event.arg.reading)
    ELSE Nil

TraceFolderArg ==
    IF "arg" \in DOMAIN logline.event /\ "folder" \in DOMAIN logline.event.arg
    THEN TraceMaybe(logline.event.arg.folder)
    ELSE Nil

ApplySnapshot ==
    /\ bookMain' = [edits |-> memBook.edits, mtime |-> ExpectedBookMainMTime]
    /\ bookConflicts' = {}
    /\ stateMain' = [
           reading |-> TraceMaybe(logline.event.state.stateMainReading),
           folder |-> TraceMaybe(logline.event.state.stateMainFolder),
           mtime |-> stateMain.mtime]
    /\ stateConflicts' = {}
    /\ translationMain' = [
           versions |-> SnapshotVersions(logline.event.state.translationVersionCount),
           mtime |-> ExpectedTranslationMainMTime]
    /\ translationConflicts' = {}
    /\ dictionaryMain' = [
           entries |-> ChooseDictionaryEntries(logline.event.state.dictionaryEntryCount),
           mtime |-> dictionaryMain.mtime]
    /\ dictionaryConflicts' = {}
    /\ memBook' = [edits |-> bookMain'.edits]
    /\ memState' = [
           reading |-> stateMain'.reading,
           folder |-> stateMain'.folder]
    /\ memTranslation' = [versions |-> translationMain'.versions]
    /\ bookLastModified' = bookMain'.mtime
    /\ translationLastModified' = translationMain'.mtime
    /\ bookSaveStage' = logline.event.state.bookSaveStage
    /\ translationSaveStage' = logline.event.state.translationSaveStage
    /\ stateOpKind' = logline.event.state.stateOpKind
    /\ pendingReading' =
         IF logline.event.state.stateOpKind = "reading"
         THEN TraceReadingArg
         ELSE Nil
    /\ pendingFolder' =
         IF logline.event.state.stateOpKind = "folder"
         THEN TraceFolderArg
         ELSE Nil
    /\ bookSaveIntent' = {}
    /\ translationSaveIntent' = {}
    /\ UNCHANGED nextTime

IsEvent(name) ==
    /\ l <= Len(TraceLog)
    /\ logline.event.name = name

ValidatePostState ==
    /\ bookMain'.mtime = ExpectedBookMainMTime
    /\ Cardinality(bookConflicts') = logline.event.state.bookConflictCount
    /\ stateMain'.reading = TraceMaybe(logline.event.state.stateMainReading)
    /\ stateMain'.folder = TraceMaybe(logline.event.state.stateMainFolder)
    /\ Cardinality(stateConflicts') = logline.event.state.stateConflictCount
    /\ translationMain'.mtime = ExpectedTranslationMainMTime
    /\ Cardinality(translationMain'.versions) = logline.event.state.translationVersionCount
    /\ Cardinality(translationConflicts') = logline.event.state.translationConflictCount
    /\ Cardinality(dictionaryMain'.entries) = logline.event.state.dictionaryEntryCount
    /\ ValidateStages

ValidateDictionaryPostState ==
    /\ Cardinality(dictionaryMain'.entries) = logline.event.state.dictionaryEntryCount
    /\ ValidateStages

StepTrace == l' = l + 1

\* ============================================================================
\* ACTION WRAPPERS
\* ============================================================================

LoadBookFromMetadataIfLogged ==
    /\ IsEvent("LoadBookFromMetadata")
    /\ ApplySnapshot
    /\ ValidatePostState
    /\ StepTrace

SaveBookBeginIfLogged ==
    /\ IsEvent("SaveBookBegin")
    /\ ApplySnapshot
    /\ ValidatePostState
    /\ StepTrace

SaveBookFinishIfLogged ==
    /\ IsEvent("SaveBookFinish")
    /\ ApplySnapshot
    /\ ValidatePostState
    /\ StepTrace

UpdateReadingStateReloadIfLogged ==
    /\ IsEvent("UpdateReadingStateReload")
    /\ "arg" \in DOMAIN logline.event
    /\ ApplySnapshot
    /\ ValidatePostState
    /\ StepTrace

UpdateReadingStatePersistIfLogged ==
    /\ IsEvent("UpdateReadingStatePersist")
    /\ ApplySnapshot
    /\ ValidatePostState
    /\ StepTrace

UpdateFolderPathReloadIfLogged ==
    /\ IsEvent("UpdateFolderPathReload")
    /\ "arg" \in DOMAIN logline.event
    /\ ApplySnapshot
    /\ ValidatePostState
    /\ StepTrace

UpdateFolderPathPersistIfLogged ==
    /\ IsEvent("UpdateFolderPathPersist")
    /\ ApplySnapshot
    /\ ValidatePostState
    /\ StepTrace

ResolveReadingStateFileIfLogged ==
    /\ IsEvent("ResolveReadingStateFile")
    /\ ApplySnapshot
    /\ ValidatePostState
    /\ StepTrace

LoadTranslationFromMetadataIfLogged ==
    /\ IsEvent("LoadTranslationFromMetadata")
    /\ ApplySnapshot
    /\ ValidatePostState
    /\ StepTrace

SaveTranslationBeginIfLogged ==
    /\ IsEvent("SaveTranslationBegin")
    /\ ApplySnapshot
    /\ ValidatePostState
    /\ StepTrace

SaveTranslationFinishIfLogged ==
    /\ IsEvent("SaveTranslationFinish")
    /\ ApplySnapshot
    /\ ValidatePostState
    /\ StepTrace

LoadDictionaryFromMetadataIfLogged ==
    /\ IsEvent("LoadDictionaryFromMetadata")
    /\ ApplySnapshot
    /\ ValidateDictionaryPostState
    /\ StepTrace

\* ============================================================================
\* SILENT ACTIONS
\* ============================================================================

\* Silent setup for load-time book conflict traces.
SilentInjectBookConflict ==
    /\ l <= Len(TraceLog)
    /\ logline.event.name = "LoadBookFromMetadata"
    /\ bookMain.mtime = 0
    /\ bookConflicts = {}
    /\ \E e \in BookEdit : InjectBookConflict(e)
    /\ UNCHANGED l

\* Silent setup for state conflict traces.
SilentInjectStateConflict ==
    /\ l <= Len(TraceLog)
    /\ logline.event.name = "ResolveReadingStateFile"
    /\ stateConflicts = {}
    /\ nextTime <= MaxTime
    /\ stateConflicts' = {
           [reading |-> TraceMaybe(logline.event.state.stateMainReading),
            folder |-> TraceMaybe(logline.event.state.stateMainFolder),
            mtime |-> nextTime]
       }
    /\ nextTime' = nextTime + 1
    /\ UNCHANGED <<bookMain, bookConflicts,
                   stateMain,
                   translationMain, translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memoryVars, saveVars, l>>

SilentPrimeBook ==
    /\ l <= Len(TraceLog)
    /\ logline.event.state.bookMainMTime /= 0
    /\ bookMain.mtime = 0
    /\ logline.event.name /= "LoadBookFromMetadata"
    /\ bookMain' = [edits |-> {CHOOSE e \in BookEdit : TRUE},
                    mtime |-> ExpectedBookMainMTime]
    /\ memBook' = [edits |-> bookMain'.edits]
    /\ bookLastModified' = ExpectedBookMainMTime
    /\ UNCHANGED <<bookConflicts,
                   stateMain, stateConflicts,
                   translationMain, translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memState, memTranslation,
                   bookSaveStage, bookSaveIntent,
                   translationLastModified, translationSaveStage, translationSaveIntent,
                   stateOpKind, pendingReading, pendingFolder,
                   nextTime, l>>

SilentAdvanceBookOnDisk ==
    /\ l <= Len(TraceLog)
    /\ logline.event.name = "SaveBookBegin"
    /\ bookMain.mtime < ExpectedBookMainMTime
    /\ \E e \in BookEdit : InjectNewerBookOnDisk(e)
    /\ UNCHANGED l

SilentPrimeDictionary ==
    /\ l <= Len(TraceLog)
    /\ logline.event.name \in {"LoadTranslationFromMetadata", "SaveBookBegin"}
    /\ Cardinality(dictionaryMain.entries) < logline.event.state.dictionaryEntryCount
    /\ dictionaryMain' = [entries |-> ChooseDictionaryEntries(logline.event.state.dictionaryEntryCount),
                          mtime |-> dictionaryMain.mtime]
    /\ UNCHANGED <<bookMain, bookConflicts,
                   stateMain, stateConflicts,
                   translationMain, translationConflicts,
                   dictionaryConflicts,
                   memoryVars, saveVars, nextTime, l>>

SilentAdvanceTranslationOnDisk ==
    /\ l <= Len(TraceLog)
    /\ logline.event.name = "SaveTranslationBegin"
    /\ translationMain.mtime < ExpectedTranslationMainMTime
    /\ translationMain' = [
           versions |-> ExtendVersions(translationMain.versions, logline.event.state.translationVersionCount),
           mtime |-> ExpectedTranslationMainMTime]
    /\ UNCHANGED <<bookMain, bookConflicts,
                   stateMain, stateConflicts,
                   translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memBook, memState, memTranslation,
                   bookLastModified, bookSaveStage, bookSaveIntent,
                   translationLastModified, translationSaveStage, translationSaveIntent,
                   stateOpKind, pendingReading, pendingFolder,
                   nextTime, l>>

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
    /\ memBook.edits = bookMain.edits
    /\ \E e \in BookEdit : EditMemoryBook(e)
    /\ UNCHANGED l

SilentAddMemoryTranslationVersion ==
    /\ l <= Len(TraceLog)
    /\ logline.event.name = "SaveTranslationBegin"
    /\ Cardinality(memTranslation.versions) <= Cardinality(translationMain.versions)
    /\ memTranslation' = [
           versions |->
               ExtendVersions(translationMain.versions, Cardinality(translationMain.versions) + 1)]
    /\ UNCHANGED <<diskVars, memBook, memState, saveVars, nextTime, l>>

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

    \/ /\ l > Len(TraceLog)
       /\ UNCHANGED <<vars, l>>

TraceSpec == TraceInit /\ [][TraceNext]_<<vars, l>> /\ WF_<<vars, l>>(TraceNext)

TraceView == <<vars, l>>

TraceMatched == <>(l > Len(TraceLog))

=============================================================================
