--------------------------- MODULE MC ---------------------------
(*
 * Model checking wrapper for the FLTS sync spec.
 *
 * Bound the environment/fault actions that introduce divergence:
 *   - user edits
 *   - conflict-file creation
 *   - external canonical-file rewrites
 *
 * Leave implementation-style reconciliation actions unbounded:
 *   - load_from_metadata
 *   - resolve_reading_state_file
 *   - save begin/finish paths
 *)

EXTENDS base

flts == INSTANCE base

\* ============================================================================
\* CONSTRAINT CONSTANTS
\* ============================================================================

CONSTANTS
    MaxUserBookEditLimit,
    MaxBookConflictLimit,
    MaxDiskBookUpdateLimit,
    MaxUserStateUpdateLimit,
    MaxStateConflictLimit,
    MaxUserTranslationLimit,
    MaxTranslationConflictLimit,
    MaxDictionaryConflictLimit

\* ============================================================================
\* COUNTER VARIABLES
\* ============================================================================

VARIABLE faultCounters

faultVars == <<faultCounters>>

CounterType ==
    [userBook            : 0..MaxUserBookEditLimit,
     bookConflict        : 0..MaxBookConflictLimit,
     diskBookUpdate      : 0..MaxDiskBookUpdateLimit,
     userStateUpdate     : 0..MaxUserStateUpdateLimit,
     stateConflict       : 0..MaxStateConflictLimit,
     userTranslation     : 0..MaxUserTranslationLimit,
     translationConflict : 0..MaxTranslationConflictLimit,
     dictionaryConflict  : 0..MaxDictionaryConflictLimit]

\* ============================================================================
\* BOUNDED ENVIRONMENT / FAULT ACTIONS
\* ============================================================================

MCInjectBookConflict(e) ==
    /\ faultCounters.bookConflict < MaxBookConflictLimit
    /\ flts!InjectBookConflict(e)
    /\ faultCounters' = [faultCounters EXCEPT !.bookConflict = @ + 1]

MCEditMemoryBook(e) ==
    /\ faultCounters.userBook < MaxUserBookEditLimit
    /\ flts!EditMemoryBook(e)
    /\ faultCounters' = [faultCounters EXCEPT !.userBook = @ + 1]

MCInjectNewerBookOnDisk(e) ==
    /\ faultCounters.diskBookUpdate < MaxDiskBookUpdateLimit
    /\ flts!InjectNewerBookOnDisk(e)
    /\ faultCounters' = [faultCounters EXCEPT !.diskBookUpdate = @ + 1]

MCUpdateReadingStateReload(r) ==
    /\ faultCounters.userStateUpdate < MaxUserStateUpdateLimit
    /\ flts!UpdateReadingStateReload(r)
    /\ faultCounters' = [faultCounters EXCEPT !.userStateUpdate = @ + 1]

MCUpdateFolderPathReload(f) ==
    /\ faultCounters.userStateUpdate < MaxUserStateUpdateLimit
    /\ flts!UpdateFolderPathReload(f)
    /\ faultCounters' = [faultCounters EXCEPT !.userStateUpdate = @ + 1]

MCInjectStateReadingConflict(r) ==
    /\ faultCounters.stateConflict < MaxStateConflictLimit
    /\ flts!InjectStateReadingConflict(r)
    /\ faultCounters' = [faultCounters EXCEPT !.stateConflict = @ + 1]

MCAddMemoryTranslationVersion(vid, ts, words) ==
    /\ faultCounters.userTranslation < MaxUserTranslationLimit
    /\ flts!AddMemoryTranslationVersion(vid, ts, words)
    /\ faultCounters' = [faultCounters EXCEPT !.userTranslation = @ + 1]

MCInjectTranslationConflict(vid, ts, words) ==
    /\ faultCounters.translationConflict < MaxTranslationConflictLimit
    /\ flts!InjectTranslationConflict(vid, ts, words)
    /\ faultCounters' = [faultCounters EXCEPT !.translationConflict = @ + 1]

MCInjectDictionaryConflict(d) ==
    /\ faultCounters.dictionaryConflict < MaxDictionaryConflictLimit
    /\ flts!InjectDictionaryConflict(d)
    /\ faultCounters' = [faultCounters EXCEPT !.dictionaryConflict = @ + 1]

\* ============================================================================
\* UNBOUNDED IMPLEMENTATION / REACTIVE ACTIONS
\* ============================================================================

MCLoadBookFromMetadata ==
    /\ flts!LoadBookFromMetadata
    /\ UNCHANGED faultVars

MCSaveBookBegin ==
    /\ flts!SaveBookBegin
    /\ UNCHANGED faultVars

MCSaveBookFinish ==
    /\ flts!SaveBookFinish
    /\ UNCHANGED faultVars

MCUpdateReadingStatePersist ==
    /\ flts!UpdateReadingStatePersist
    /\ UNCHANGED faultVars

MCUpdateFolderPathPersist ==
    /\ flts!UpdateFolderPathPersist
    /\ UNCHANGED faultVars

MCResolveReadingStateFile ==
    /\ flts!ResolveReadingStateFile
    /\ UNCHANGED faultVars

MCLoadTranslationFromMetadata ==
    /\ flts!LoadTranslationFromMetadata
    /\ UNCHANGED faultVars

MCSaveTranslationBegin ==
    /\ flts!SaveTranslationBegin
    /\ UNCHANGED faultVars

MCSaveTranslationFinish ==
    /\ flts!SaveTranslationFinish
    /\ UNCHANGED faultVars

MCLoadDictionaryFromMetadata ==
    /\ flts!LoadDictionaryFromMetadata
    /\ UNCHANGED faultVars

\* ============================================================================
\* INITIALIZATION
\* ============================================================================

MCInit ==
    /\ Init
    /\ faultCounters = [
          userBook            |-> 0,
          bookConflict        |-> 0,
          diskBookUpdate      |-> 0,
          userStateUpdate     |-> 0,
          stateConflict       |-> 0,
          userTranslation     |-> 0,
          translationConflict |-> 0,
          dictionaryConflict  |-> 0]

\* ============================================================================
\* NEXT-STATE RELATION
\* ============================================================================

MCNext ==
    \/ \E e \in BookEdit : MCInjectBookConflict(e)
    \/ MCLoadBookFromMetadata
    \/ \E e \in BookEdit : MCEditMemoryBook(e)
    \/ \E e \in BookEdit : MCInjectNewerBookOnDisk(e)
    \/ MCSaveBookBegin
    \/ MCSaveBookFinish

    \/ \E r \in ReadingPos : MCUpdateReadingStateReload(r)
    \/ MCUpdateReadingStatePersist
    \/ \E f \in FolderVal : MCUpdateFolderPathReload(f)
    \/ MCUpdateFolderPathPersist
    \/ \E r \in ReadingPos : MCInjectStateReadingConflict(r)
    \/ MCResolveReadingStateFile

    \/ \E vid \in VersionId :
       \E ts \in 0..MaxTime :
       \E words \in SUBSET VisibleWord :
           MCAddMemoryTranslationVersion(vid, ts, words)
    \/ \E vid \in VersionId :
       \E ts \in 0..MaxTime :
       \E words \in SUBSET VisibleWord :
           MCInjectTranslationConflict(vid, ts, words)
    \/ MCLoadTranslationFromMetadata
    \/ MCSaveTranslationBegin
    \/ MCSaveTranslationFinish

    \/ \E d \in DictEntry : MCInjectDictionaryConflict(d)
    \/ MCLoadDictionaryFromMetadata

\* ============================================================================
\* VIEW / PROPERTIES
\* ============================================================================

View ==
    <<bookMain, bookConflicts,
      stateMain, stateConflicts,
      translationMain, translationConflicts,
      dictionaryMain, dictionaryConflicts,
      memBook, memState, memTranslation,
      bookLastModified, bookSaveStage,
      translationLastModified, translationSaveStage,
      stateOpKind>>

=============================================================================
