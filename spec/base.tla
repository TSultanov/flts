--------------------------- MODULE base ---------------------------
(*
 * TLA+ specification for the FLTS file synchronization / merge subsystem.
 *
 * Derived from:
 *   - library/src/library.rs
 *   - library/src/library/library_book.rs
 *   - library/src/book/translation.rs
 *   - library/src/library/library_dictionary.rs
 *
 * Bug families:
 *   1. Whole-file newest-wins conflict resolution for book.dat and state.json
 *   2. Book save/reload overwrites newer disk state instead of merging
 *   3. Translation history identity is timestamp-based
 *)

EXTENDS Naturals, FiniteSets, TLC

\* ============================================================================
\* CONSTANTS
\* ============================================================================

CONSTANTS
    BookEdit,      \* abstract book edit tokens
    ReadingPos,    \* abstract reading positions
    FolderVal,     \* abstract folder assignments
    VersionId,     \* abstract translation version ids
    VisibleWord,   \* abstract visible-word ids
    DictEntry,     \* abstract dictionary entries
    Nil,           \* sentinel for "no value"
    MaxTime        \* maximum modeled clock / mtime

\* ============================================================================
\* VARIABLES
\* ============================================================================

\* --- Disk state: canonical files plus conflict siblings ---
VARIABLE bookMain
VARIABLE bookConflicts
VARIABLE stateMain
VARIABLE stateConflicts
VARIABLE translationMain
VARIABLE translationConflicts
VARIABLE dictionaryMain
VARIABLE dictionaryConflicts

\* --- In-memory cached state ---
VARIABLE memBook
VARIABLE memState
VARIABLE memTranslation

\* --- Save/reload bookkeeping ---
VARIABLE bookLastModified
VARIABLE bookSaveStage
VARIABLE bookSaveIntent

VARIABLE translationLastModified
VARIABLE translationSaveStage
VARIABLE translationSaveIntent

VARIABLE stateOpKind
VARIABLE pendingReading
VARIABLE pendingFolder

\* --- Logical clock used for mtimes and persisted rewrites ---
VARIABLE nextTime

\* ============================================================================
\* VARIABLE GROUPS
\* ============================================================================

diskVars ==
    <<bookMain, bookConflicts,
      stateMain, stateConflicts,
      translationMain, translationConflicts,
      dictionaryMain, dictionaryConflicts>>

memoryVars ==
    <<memBook, memState, memTranslation>>

saveVars ==
    <<bookLastModified, bookSaveStage, bookSaveIntent,
      translationLastModified, translationSaveStage, translationSaveIntent,
      stateOpKind, pendingReading, pendingFolder>>

vars == <<diskVars, memoryVars, saveVars, nextTime>>

\* ============================================================================
\* TYPES AND HELPERS
\* ============================================================================

BookFileType == [edits : SUBSET BookEdit, mtime : 0..(MaxTime + 1)]
StateFileType == [reading : ReadingPos \cup {Nil},
                  folder  : FolderVal \cup {Nil},
                  mtime   : 0..(MaxTime + 1)]
VersionType == [id : VersionId, ts : 0..MaxTime, visible : SUBSET VisibleWord]
TranslationFileType == [versions : SUBSET VersionType, mtime : 0..(MaxTime + 1)]
DictionaryFileType == [entries : SUBSET DictEntry, mtime : 0..(MaxTime + 1)]

BookFiles == bookConflicts \cup {bookMain}
StateFiles == stateConflicts \cup {stateMain}
TranslationFiles == translationConflicts \cup {translationMain}
DictionaryFiles == dictionaryConflicts \cup {dictionaryMain}

LatestByMTime(files) ==
    CHOOSE f \in files : f.mtime = Max({g.mtime : g \in files})

LoadedBookEdits ==
    LatestByMTime(BookFiles).edits

LoadedState ==
    LET latest == LatestByMTime(StateFiles)
    IN [reading |-> latest.reading,
        folder  |-> latest.folder]

LatestReadingField(files) ==
    LET withReading == {f \in files : f.reading /= Nil}
    IN IF withReading = {}
       THEN Nil
       ELSE (CHOOSE f \in withReading :
                f.mtime = Max({g.mtime : g \in withReading})).reading

LatestFolderField(files) ==
    LET withFolder == {f \in files : f.folder /= Nil}
    IN IF withFolder = {}
       THEN Nil
       ELSE (CHOOSE f \in withFolder :
                f.mtime = Max({g.mtime : g \in withFolder})).folder

FieldwiseState(files) ==
    [reading |-> LatestReadingField(files),
     folder  |-> LatestFolderField(files)]

AllTranslationVersions ==
    UNION {f.versions : f \in TranslationFiles}

VersionsAtTS(vs, ts) ==
    {v \in vs : v.ts = ts}

MergedVersionAtTS(vs, ts) ==
    LET same   == VersionsAtTS(vs, ts)
        chosen == CHOOSE v \in same : TRUE
    IN [id      |-> chosen.id,
        ts      |-> ts,
        visible |-> UNION {v.visible : v \in same}]

CollapseByTimestamp(vs) ==
    IF vs = {}
    THEN {}
    ELSE {MergedVersionAtTS(vs, ts) : ts \in {v.ts : v \in vs}}

LoadedTranslationVersions ==
    CollapseByTimestamp(AllTranslationVersions)

AllVersionIds(vs) ==
    {v.id : v \in vs}

LoadedDictionaryEntries ==
    UNION {f.entries : f \in DictionaryFiles}

\* ============================================================================
\* INITIALIZATION
\* ============================================================================

Init ==
    /\ bookMain = [edits |-> {}, mtime |-> 0]
    /\ bookConflicts = {}
    /\ stateMain = [reading |-> Nil, folder |-> Nil, mtime |-> 0]
    /\ stateConflicts = {}
    /\ translationMain = [versions |-> {}, mtime |-> 0]
    /\ translationConflicts = {}
    /\ dictionaryMain = [entries |-> {}, mtime |-> 0]
    /\ dictionaryConflicts = {}

    /\ memBook = [edits |-> {}]
    /\ memState = [reading |-> Nil, folder |-> Nil]
    /\ memTranslation = [versions |-> {}]

    /\ bookLastModified = 0
    /\ bookSaveStage = "idle"
    /\ bookSaveIntent = {}

    /\ translationLastModified = 0
    /\ translationSaveStage = "idle"
    /\ translationSaveIntent = {}

    /\ stateOpKind = "idle"
    /\ pendingReading = Nil
    /\ pendingFolder = Nil

    /\ nextTime = 1

\* ============================================================================
\* ACTIONS
\* ============================================================================

\* --------------------------------------------------------------------------
\* Environment action: create a divergent book conflict sibling.
\* Models an external sync system producing `book*.dat` siblings.
\* --------------------------------------------------------------------------
InjectBookConflict(e) ==
    /\ e \in BookEdit
    /\ nextTime <= MaxTime
    /\ bookConflicts' = bookConflicts \cup {
           [edits |-> {e}, mtime |-> nextTime]
       }
    /\ nextTime' = nextTime + 1
    /\ UNCHANGED <<stateMain, stateConflicts,
                   translationMain, translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memBook, memState, memTranslation,
                   saveVars>>

\* --------------------------------------------------------------------------
\* LoadBookFromMetadata: choose newest `book*.dat`, move it to main, delete rest.
\* Reference: library/src/library/library_book.rs:335-381
\* --------------------------------------------------------------------------
LoadBookFromMetadata ==
    /\ bookConflicts /= {}
    /\ LET newest == LatestByMTime(BookFiles) IN
       /\ \* Select newest candidate and make it canonical.
          \* library_book.rs:357-374
          bookMain' = newest
       /\ \* Delete all conflict siblings after selection.
          \* library_book.rs:376-381
          bookConflicts' = {}
       /\ \* Load chosen book into memory and record its mtime.
          \* library_book.rs:383-394
          memBook' = [edits |-> newest.edits]
       /\ bookLastModified' = newest.mtime
    /\ UNCHANGED <<stateMain, stateConflicts,
                   translationMain, translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memState, memTranslation,
                   bookSaveStage, bookSaveIntent,
                   translationLastModified, translationSaveStage, translationSaveIntent,
                   stateOpKind, pendingReading, pendingFolder,
                   nextTime>>

\* --------------------------------------------------------------------------
\* Local book edit in the cached LibraryBook object.
\* Abstracts direct mutations like title/chapter/paragraph edits before save.
\* --------------------------------------------------------------------------
EditMemoryBook(e) ==
    /\ e \in BookEdit
    /\ memBook' = [edits |-> memBook.edits \cup {e}]
    /\ UNCHANGED <<diskVars, memState, memTranslation, saveVars, nextTime>>

\* --------------------------------------------------------------------------
\* Environment action: external process rewrites canonical book.dat.
\* Models a watcher-visible disk update that races with local save.
\* --------------------------------------------------------------------------
InjectNewerBookOnDisk(e) ==
    /\ e \in BookEdit
    /\ nextTime <= MaxTime
    /\ bookMain' = [edits |-> bookMain.edits \cup {e}, mtime |-> nextTime]
    /\ nextTime' = nextTime + 1
    /\ UNCHANGED <<bookConflicts,
                   stateMain, stateConflicts,
                   translationMain, translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memoryVars, saveVars>>

\* --------------------------------------------------------------------------
\* SaveBookBegin: pre-save reconciliation for canonical book.dat.
\* Reference: library/src/library/library_book.rs:555-575
\* --------------------------------------------------------------------------
SaveBookBegin ==
    /\ bookSaveStage = "idle"
    /\ \* Record the semantic intent of the save: preserve both memory and disk edits.
       bookSaveIntent' = memBook.edits \cup bookMain.edits
    /\ \* If disk is newer than our cached mtime, implementation replaces memory
       \* with disk rather than merging.
       \* library_book.rs:561-575
       IF bookMain.mtime > bookLastModified
       THEN /\ memBook' = [edits |-> bookMain.edits]
            /\ bookLastModified' = bookMain.mtime
       ELSE /\ UNCHANGED memBook
            /\ UNCHANGED bookLastModified
    /\ bookSaveStage' = "ready"
    /\ UNCHANGED <<bookMain, bookConflicts,
                   stateMain, stateConflicts,
                   translationMain, translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memState, memTranslation,
                   translationLastModified, translationSaveStage, translationSaveIntent,
                   stateOpKind, pendingReading, pendingFolder,
                   nextTime>>

\* --------------------------------------------------------------------------
\* SaveBookFinish: rewrite canonical book.dat from current memory state.
\* Reference: library/src/library/library_book.rs:577-598
\* --------------------------------------------------------------------------
SaveBookFinish ==
    /\ bookSaveStage = "ready"
    /\ nextTime <= MaxTime
    /\ \* Serialize memory state to a temp file and rename to book.dat.
       \* library_book.rs:577-594
       bookMain' = [edits |-> memBook.edits, mtime |-> nextTime]
    /\ bookLastModified' = nextTime
    /\ bookSaveStage' = "idle"
    /\ bookSaveIntent' = {}
    /\ nextTime' = nextTime + 1
    /\ UNCHANGED <<bookConflicts,
                   stateMain, stateConflicts,
                   translationMain, translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memBook, memState, memTranslation,
                   translationLastModified, translationSaveStage, translationSaveIntent,
                   stateOpKind, pendingReading, pendingFolder>>

\* --------------------------------------------------------------------------
\* UpdateReadingStateReload: read newest resolved state before writing.
\* Reference: library/src/library/library_book.rs:277-290
\* --------------------------------------------------------------------------
UpdateReadingStateReload(r) ==
    /\ r \in ReadingPos
    /\ stateOpKind = "idle"
    /\ \* reload_user_state() resolves state files before mutation.
       \* library_book.rs:277-289
       memState' = LoadedState
    /\ stateOpKind' = "reading"
    /\ pendingReading' = r
    /\ pendingFolder' = Nil
    /\ UNCHANGED <<diskVars, memBook, memTranslation,
                   bookLastModified, bookSaveStage, bookSaveIntent,
                   translationLastModified, translationSaveStage, translationSaveIntent,
                   nextTime>>

\* --------------------------------------------------------------------------
\* UpdateReadingStatePersist: persist whole BookUserState after changing reading.
\* Reference: library/src/library/library_book.rs:287-290, 250-269
\* --------------------------------------------------------------------------
UpdateReadingStatePersist ==
    /\ stateOpKind = "reading"
    /\ nextTime <= MaxTime
    /\ \* Persist full state object, carrying folder from the earlier reload snapshot.
       \* library_book.rs:287-290 and persist_user_state at 250-269
       stateMain' = [reading |-> pendingReading,
                     folder  |-> memState.folder,
                     mtime   |-> nextTime]
    /\ memState' = [reading |-> pendingReading,
                    folder  |-> memState.folder]
    /\ stateOpKind' = "idle"
    /\ pendingReading' = Nil
    /\ pendingFolder' = Nil
    /\ nextTime' = nextTime + 1
    /\ UNCHANGED <<bookMain, bookConflicts,
                   stateConflicts,
                   translationMain, translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memBook, memTranslation,
                   bookLastModified, bookSaveStage, bookSaveIntent,
                   translationLastModified, translationSaveStage, translationSaveIntent>>

\* --------------------------------------------------------------------------
\* UpdateFolderPathReload: read newest resolved state before writing folder path.
\* Reference: library/src/library/library_book.rs:277-298
\* --------------------------------------------------------------------------
UpdateFolderPathReload(f) ==
    /\ f \in FolderVal
    /\ stateOpKind = "idle"
    /\ \* reload_user_state() resolves state files before mutation.
       \* library_book.rs:277-298
       memState' = LoadedState
    /\ stateOpKind' = "folder"
    /\ pendingReading' = Nil
    /\ pendingFolder' = f
    /\ UNCHANGED <<diskVars, memBook, memTranslation,
                   bookLastModified, bookSaveStage, bookSaveIntent,
                   translationLastModified, translationSaveStage, translationSaveIntent,
                   nextTime>>

\* --------------------------------------------------------------------------
\* UpdateFolderPathPersist: persist whole BookUserState after changing folder path.
\* Reference: library/src/library/library_book.rs:294-297, 250-269
\* --------------------------------------------------------------------------
UpdateFolderPathPersist ==
    /\ stateOpKind = "folder"
    /\ nextTime <= MaxTime
    /\ \* Persist full state object, carrying reading value from the earlier reload snapshot.
       \* library_book.rs:294-297 and persist_user_state at 250-269
       stateMain' = [reading |-> memState.reading,
                     folder  |-> pendingFolder,
                     mtime   |-> nextTime]
    /\ memState' = [reading |-> memState.reading,
                    folder  |-> pendingFolder]
    /\ stateOpKind' = "idle"
    /\ pendingReading' = Nil
    /\ pendingFolder' = Nil
    /\ nextTime' = nextTime + 1
    /\ UNCHANGED <<bookMain, bookConflicts,
                   stateConflicts,
                   translationMain, translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memBook, memTranslation,
                   bookLastModified, bookSaveStage, bookSaveIntent,
                   translationLastModified, translationSaveStage, translationSaveIntent>>

\* --------------------------------------------------------------------------
\* Environment action: sync creates a state conflict with a newer reading field.
\* --------------------------------------------------------------------------
InjectStateReadingConflict(r) ==
    /\ r \in ReadingPos
    /\ nextTime <= MaxTime
    /\ stateConflicts' = stateConflicts \cup {
           [reading |-> r, folder |-> stateMain.folder, mtime |-> nextTime]
       }
    /\ nextTime' = nextTime + 1
    /\ UNCHANGED <<bookMain, bookConflicts,
                   stateMain,
                   translationMain, translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memoryVars, saveVars>>

\* --------------------------------------------------------------------------
\* Environment action: sync creates a state conflict with a newer folder field.
\* --------------------------------------------------------------------------
InjectStateFolderConflict(f) ==
    /\ f \in FolderVal
    /\ nextTime <= MaxTime
    /\ stateConflicts' = stateConflicts \cup {
           [reading |-> stateMain.reading, folder |-> f, mtime |-> nextTime]
       }
    /\ nextTime' = nextTime + 1
    /\ UNCHANGED <<bookMain, bookConflicts,
                   stateMain,
                   translationMain, translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memoryVars, saveVars>>

\* --------------------------------------------------------------------------
\* ResolveReadingStateFile: choose newest state*.json and delete the rest.
\* Reference: library/src/library/library_book.rs:187-223
\* --------------------------------------------------------------------------
ResolveReadingStateFile ==
    /\ stateConflicts /= {}
    /\ LET newest == LatestByMTime(StateFiles) IN
       /\ \* Move newest candidate to state.json.
          \* library_book.rs:193-212
          stateMain' = newest
       /\ \* Delete all remaining state conflict files.
          \* library_book.rs:214-220
          stateConflicts' = {}
       /\ memState' = [reading |-> newest.reading,
                       folder  |-> newest.folder]
    /\ UNCHANGED <<bookMain, bookConflicts,
                   translationMain, translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memBook, memTranslation,
                   bookLastModified, bookSaveStage, bookSaveIntent,
                   translationLastModified, translationSaveStage, translationSaveIntent,
                   stateOpKind, pendingReading, pendingFolder,
                   nextTime>>

\* --------------------------------------------------------------------------
\* AddMemoryTranslationVersion: local translation history grows in memory.
\* Abstracts add_paragraph_translation / mark_word_visible state before save.
\* --------------------------------------------------------------------------
AddMemoryTranslationVersion(vid, ts, words) ==
    /\ vid \in VersionId
    /\ ts \in 0..MaxTime
    /\ words \in SUBSET VisibleWord
    /\ memTranslation' = [versions |-> memTranslation.versions \cup {
           [id |-> vid, ts |-> ts, visible |-> words]
       }]
    /\ UNCHANGED <<diskVars, memBook, memState, saveVars, nextTime>>

\* --------------------------------------------------------------------------
\* Environment action: create translation conflict sibling.
\* --------------------------------------------------------------------------
InjectTranslationConflict(vid, ts, words) ==
    /\ vid \in VersionId
    /\ ts \in 0..MaxTime
    /\ words \in SUBSET VisibleWord
    /\ nextTime <= MaxTime
    /\ translationConflicts' = translationConflicts \cup {
           [versions |-> {
               [id |-> vid, ts |-> ts, visible |-> words]
            },
            mtime |-> nextTime]
       }
    /\ nextTime' = nextTime + 1
    /\ UNCHANGED <<bookMain, bookConflicts,
                   stateMain, stateConflicts,
                   translationMain,
                   dictionaryMain, dictionaryConflicts,
                   memoryVars, saveVars>>

\* --------------------------------------------------------------------------
\* LoadTranslationFromMetadata: merge main + conflict translations and persist.
\* Reference: library/src/library/library_book.rs:101-128
\* --------------------------------------------------------------------------
LoadTranslationFromMetadata ==
    /\ translationConflicts /= {}
    /\ nextTime <= MaxTime
    /\ LET merged == LoadedTranslationVersions IN
       /\ \* Merge conflict histories by timestamp and rewrite canonical file.
          \* library_book.rs:105-125 and translation.rs:399-479
          translationMain' = [versions |-> merged, mtime |-> nextTime]
       /\ translationConflicts' = {}
       /\ memTranslation' = [versions |-> merged]
       /\ translationLastModified' = nextTime
    /\ nextTime' = nextTime + 1
    /\ UNCHANGED <<bookMain, bookConflicts,
                   stateMain, stateConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memBook, memState,
                   bookLastModified, bookSaveStage, bookSaveIntent,
                   translationSaveStage, translationSaveIntent,
                   stateOpKind, pendingReading, pendingFolder>>

\* --------------------------------------------------------------------------
\* SaveTranslationBegin: merge newer canonical translation into memory before save.
\* Reference: library/src/library/library_book.rs:473-505
\* --------------------------------------------------------------------------
SaveTranslationBegin ==
    /\ translationSaveStage = "idle"
    /\ translationSaveIntent' =
         AllVersionIds(memTranslation.versions) \cup AllVersionIds(translationMain.versions)
    /\ \* If canonical translation is newer than our cached mtime, merge it in.
       \* library_book.rs:484-505
       IF translationMain.mtime > translationLastModified
       THEN /\ memTranslation' = [versions |->
                    CollapseByTimestamp(memTranslation.versions \cup translationMain.versions)]
            /\ translationLastModified' = translationMain.mtime
       ELSE /\ UNCHANGED memTranslation
            /\ UNCHANGED translationLastModified
    /\ translationSaveStage' = "ready"
    /\ UNCHANGED <<bookMain, bookConflicts,
                   stateMain, stateConflicts,
                   translationMain, translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memBook, memState,
                   bookLastModified, bookSaveStage, bookSaveIntent,
                   stateOpKind, pendingReading, pendingFolder,
                   nextTime>>

\* --------------------------------------------------------------------------
\* SaveTranslationFinish: persist merged translation history to canonical file.
\* Reference: library/src/library/library_book.rs:507-540
\* --------------------------------------------------------------------------
SaveTranslationFinish ==
    /\ translationSaveStage = "ready"
    /\ nextTime <= MaxTime
    /\ translationMain' = [versions |-> memTranslation.versions, mtime |-> nextTime]
    /\ translationLastModified' = nextTime
    /\ translationSaveStage' = "idle"
    /\ translationSaveIntent' = {}
    /\ nextTime' = nextTime + 1
    /\ UNCHANGED <<bookMain, bookConflicts,
                   stateMain, stateConflicts,
                   translationConflicts,
                   dictionaryMain, dictionaryConflicts,
                   memBook, memState, memTranslation,
                   bookLastModified, bookSaveStage, bookSaveIntent,
                   stateOpKind, pendingReading, pendingFolder>>

\* --------------------------------------------------------------------------
\* Environment action: create dictionary conflict sibling.
\* --------------------------------------------------------------------------
InjectDictionaryConflict(d) ==
    /\ d \in DictEntry
    /\ nextTime <= MaxTime
    /\ dictionaryConflicts' = dictionaryConflicts \cup {
           [entries |-> {d}, mtime |-> nextTime]
       }
    /\ nextTime' = nextTime + 1
    /\ UNCHANGED <<bookMain, bookConflicts,
                   stateMain, stateConflicts,
                   translationMain, translationConflicts,
                   dictionaryMain,
                   memoryVars, saveVars>>

\* --------------------------------------------------------------------------
\* LoadDictionaryFromMetadata: merge dictionary conflicts by entry union.
\* Reference: library/src/library/library_dictionary.rs:121-149
\* --------------------------------------------------------------------------
LoadDictionaryFromMetadata ==
    /\ dictionaryConflicts /= {}
    /\ nextTime <= MaxTime
    /\ \* Merge each conflict dictionary into the base dictionary.
       \* library_dictionary.rs:121-149
       dictionaryMain' = [entries |-> LoadedDictionaryEntries, mtime |-> nextTime]
    /\ dictionaryConflicts' = {}
    /\ nextTime' = nextTime + 1
    /\ UNCHANGED <<bookMain, bookConflicts,
                   stateMain, stateConflicts,
                   translationMain, translationConflicts,
                   memoryVars, saveVars>>

\* ============================================================================
\* NEXT-STATE RELATION
\* ============================================================================

Next ==
    \/ \E e \in BookEdit : InjectBookConflict(e)
    \/ LoadBookFromMetadata
    \/ \E e \in BookEdit : EditMemoryBook(e)
    \/ \E e \in BookEdit : InjectNewerBookOnDisk(e)
    \/ SaveBookBegin
    \/ SaveBookFinish

    \/ \E r \in ReadingPos : UpdateReadingStateReload(r)
    \/ UpdateReadingStatePersist
    \/ \E f \in FolderVal : UpdateFolderPathReload(f)
    \/ UpdateFolderPathPersist
    \/ \E r \in ReadingPos : InjectStateReadingConflict(r)
    \/ \E f \in FolderVal : InjectStateFolderConflict(f)
    \/ ResolveReadingStateFile

    \/ \E vid \in VersionId :
       \E ts \in 0..MaxTime :
       \E words \in SUBSET VisibleWord :
           AddMemoryTranslationVersion(vid, ts, words)
    \/ \E vid \in VersionId :
       \E ts \in 0..MaxTime :
       \E words \in SUBSET VisibleWord :
           InjectTranslationConflict(vid, ts, words)
    \/ LoadTranslationFromMetadata
    \/ SaveTranslationBegin
    \/ SaveTranslationFinish

    \/ \E d \in DictEntry : InjectDictionaryConflict(d)
    \/ LoadDictionaryFromMetadata

\* ============================================================================
\* INVARIANTS
\* ============================================================================

TypeOK ==
    /\ bookMain \in BookFileType
    /\ bookConflicts \subseteq BookFileType
    /\ stateMain \in StateFileType
    /\ stateConflicts \subseteq StateFileType
    /\ translationMain \in TranslationFileType
    /\ translationConflicts \subseteq TranslationFileType
    /\ dictionaryMain \in DictionaryFileType
    /\ dictionaryConflicts \subseteq DictionaryFileType

    /\ memBook \in [edits : SUBSET BookEdit]
    /\ memState \in [reading : ReadingPos \cup {Nil},
                     folder  : FolderVal \cup {Nil}]
    /\ memTranslation \in [versions : SUBSET VersionType]

    /\ bookLastModified \in 0..(MaxTime + 1)
    /\ bookSaveStage \in {"idle", "ready"}
    /\ bookSaveIntent \subseteq BookEdit

    /\ translationLastModified \in 0..(MaxTime + 1)
    /\ translationSaveStage \in {"idle", "ready"}
    /\ translationSaveIntent \subseteq VersionId

    /\ stateOpKind \in {"idle", "reading", "folder"}
    /\ pendingReading \in ReadingPos \cup {Nil}
    /\ pendingFolder \in FolderVal \cup {Nil}

    /\ nextTime \in 1..(MaxTime + 1)

StagesWellFormed ==
    /\ stateOpKind = "idle" => /\ pendingReading = Nil /\ pendingFolder = Nil
    /\ stateOpKind = "reading" => pendingReading /= Nil
    /\ stateOpKind = "folder"  => pendingFolder /= Nil

\* Family 1: newest-wins book conflict resolution loses edits from older siblings.
BookConflictPreservesAllEdits ==
    LoadedBookEdits = UNION {f.edits : f \in BookFiles}

\* Family 1: newest-wins state resolution should preserve latest independent fields.
StateConflictPreservesIndependentFields ==
    /\ LoadedState.reading = FieldwiseState(StateFiles).reading
    /\ LoadedState.folder  = FieldwiseState(StateFiles).folder

\* Family 2: once save intent is captured, in-memory book should still contain it.
BookSaveIntentPreserved ==
    bookSaveStage = "idle" \/ bookSaveIntent \subseteq memBook.edits

\* Family 3: distinct translation versions should not collapse solely by timestamp.
TranslationDistinctVersionsPreserved ==
    AllVersionIds(LoadedTranslationVersions) = AllVersionIds(AllTranslationVersions)

\* Structural contrast: dictionary merge path is monotonic under union.
DictionaryEntriesMonotonic ==
    dictionaryMain.entries \subseteq LoadedDictionaryEntries

=============================================================================
