# FLTS file sync modeling brief

## 1. System overview

FLTS is a Rust workspace with a library crate that persists books, translation histories, dictionaries, and per-book user state on disk. The sync-relevant logic lives in `library/src/library.rs`, `library/src/library/library_book.rs`, `library/src/book/translation.rs`, `library/src/library/library_dictionary.rs`, and `library/src/library/file_watcher.rs`.

**Category:** **A (Distributed / Message-Passing / persistence-driven)**. The interesting correctness boundaries are not thread-level atomics but independent producers of on-disk state: local memory, sync-conflict files, and watcher-driven reload/save paths that reconcile them.

The implementation does **not** use a single uniform merge algorithm. Translation and dictionary files merge semantically, but `book.dat` and `state.json` resolve conflicts by choosing the newest file and deleting the rest. Runtime reload on `book.dat` also differs from translation/dictionary reload by replacing memory with disk instead of merging.

Concurrency model: async Rust with cached `LibraryBook` / `LibraryDictionary` objects guarded by `tokio::sync::Mutex`, plus filesystem watcher events feeding `handle_file_change_event` in `library/src/library.rs:302-340`.

## 2. Bug families

### Family 1: Whole-file newest-wins conflict resolution

**Mechanism:** when multiple persisted files represent the same logical object, `book.dat` and `state.json` choose the newest mtime and delete all other candidates instead of field-wise or semantic merge.

**Evidence:**
- Historical: `bb1b075` introduced sync-conflict loading; later tests explicitly encoded newest-wins for books/state.
- Code analysis:
  - `library/src/library/library_book.rs:187-223` resolves `state*.json` by newest mtime, renames that file to `state.json`, and removes the others.
  - `library/src/library/library_book.rs:335-381` resolves `book*.dat` by newest mtime, moves that file to `book.dat`, and removes the others.
  - `library/src/library/library_book.rs:1219-1262` tests that reading-state load prefers the newest conflict copy.
  - `library/src/library/library_book.rs:1575-1690` tests that book load keeps the newest `book*.dat`.

**Affected code paths:** `resolve_reading_state_file`, `load_user_state_from_dir`, `LibraryBook::load_from_metadata`.

**Suggested modeling approach:**
- Variables: canonical `bookMain`, `bookConflicts`, `stateMain`, `stateConflicts`, each carrying content plus mtime.
- Actions: split `ResolveReadingStateFile` and `LoadBookFromMetadata` as separate actions.
- Granularity: single-step resolve actions are sufficient; the bug is in the selection rule itself.

**Priority:** High
**Rationale:** this is a direct user-edit loss mechanism and is already encoded as current behavior in tests.

### Family 2: Book save/reload overwrites newer state instead of merging

**Mechanism:** the runtime save path for `book.dat` replaces in-memory state with the newer on-disk book before rewrite, while translations and dictionaries merge newer disk state back into memory.

**Evidence:**
- Historical: `f2ac258` added translation conflict resolution on load; `e104b4a` fixed merge bugs; there is no equivalent semantic merge introduction for `book.dat`.
- Code analysis:
  - `library/src/library/library_book.rs:484-505` merges newer on-disk translation state into memory before writing.
  - `library/src/library/library_dictionary.rs:175-212` merges newer on-disk dictionary state into memory before writing.
  - `library/src/library/library_book.rs:561-575` loads newer `book.dat` and assigns `book.book = saved_book.book`, replacing memory instead of merging.
  - `library/src/library/library_book.rs:415-421` and `library/src/library.rs:308-315` route watcher-driven book reload through `save()`, making the overwrite path reachable on external file change.

**Affected code paths:** `Library::handle_file_change_event`, `LibraryBook::reload_book`, `LibraryBook::save`, translation/dictionary save paths as contrast cases.

**Suggested modeling approach:**
- Variables: `memBook`, `bookMain`, `bookLastModified`, `pendingBookSave`.
- Actions: split `SaveBookBegin` (capture newer-on-disk branch) and `SaveBookFinish` (rewrite canonical file); keep translation save as a separate merge-preserving action.
- Granularity: two-step action to expose the overwrite window and make loss checkable.

**Priority:** High
**Rationale:** this is a core code-path inconsistency inside the same subsystem and directly affects whether local edits survive external updates.

### Family 3: Translation history identity is timestamp-based

**Mechanism:** translation merge treats `timestamp` as both ordering key and duplicate identity. Same-timestamp versions are coalesced, and timestamps are generated from wall-clock seconds.

**Evidence:**
- Historical: `b793938` simplified visible-word merging around same-timestamp coalescing; merge behavior is intentionally centered on timestamps.
- Code analysis:
  - `library/src/book/translation.rs:399-479` merges paragraph histories by collecting versions, deduplicating by timestamp, sorting by timestamp, and rebuilding history.
  - `library/src/book/translation.rs:422-445` unions `visible_words` for matching timestamps.
  - `library/src/book/translation.rs:1973-2254` tests same-history merge, diverged histories, no-common-root merge, and same-timestamp visible-word union.
  - `library/src/translator/openai.rs:252-255` and `library/src/translator/gemini.rs:248-250` stamp versions with `SystemTime::now().duration_since(UNIX_EPOCH).as_secs()`.

**Affected code paths:** `Translation::merge`, `LibraryTranslation::merge`, translator timestamp assignment.

**Suggested modeling approach:**
- Variables: translation history as a set/sequence of version records with `ts`, `editId`, and `visibleWords`.
- Actions: `LoadTranslationFromMetadata`, `SaveTranslationBegin`, `SaveTranslationFinish`.
- Granularity: one merge action is sufficient, plus an injected same-timestamp conflict action in MC.

**Priority:** Medium
**Rationale:** translation merge is substantially better than book/state merge, but it still assumes timestamp uniqueness and correct clock ordering.

## 3. Modeling recommendations

### 3.1 Model

| What | Why | How |
|---|---|---|
| Canonical file plus sibling conflict files for books and state | Family 1 is driven by multiple same-object files on disk | Model `main` file plus a finite set of conflict candidates carrying mtime and content |
| Separate memory vs disk book state | Family 2 depends on overwrite of memory from newer disk state | Keep `memBook`, `bookMain`, and `bookSaveStage` distinct |
| Translation semantic merge | Family 2 contrast and Family 3 both depend on this path being modeled faithfully | Represent translation histories explicitly and model timestamp-based merge |
| Dictionary semantic merge | Confirms subsystem asymmetry and provides another merge-preserving contrast case | Use a simpler union model for dictionary entries |
| Watcher-triggered reload/save path | External changes become book reloads through this path | Include `HandleBookFileChange` / `HandleTranslationFileChange` wrappers in trace design |

### 3.2 Do not model

| What | Why |
|---|---|
| UI rendering and Svelte stores | Not needed to prove merge behavior; these are consumers of backend state |
| Compression / serialization internals | Format details are not the bug source here |
| Full translation sentence/grammar structure | The merge risk is version identity and visible-word preservation, not NLP content |
| Filesystem debouncer timing details | Debounce affects event multiplicity, but not the core overwrite/merge rule being targeted |

## 4. Proposed extensions

| Extension | Variables | Purpose | Bug family |
|---|---|---|---|
| Conflict-file state | `bookMain`, `bookConflicts`, `stateMain`, `stateConflicts` | Model newest-wins resolution for books/state | Family 1 |
| Save-stage memory/disk split | `memBook`, `bookLastModified`, `bookSaveStage`, `bookSaveIntent` | Expose overwrite-vs-merge during `book.dat` save | Family 2 |
| Translation version records | `translationMain`, `translationConflicts`, `translationSaveIntent` | Model semantic merge and timestamp coalescing | Family 3 |
| Dictionary merge state | `dictionaryMain`, `dictionaryConflicts` | Preserve contrast path that unions entries correctly | Family 2 |

## 5. Proposed invariants

| Invariant | Type | Description | Targets |
|---|---|---|---|
| `BookConflictPreservesAllEdits` | Safety | Effective canonical book contains the union of edits from all `book*.dat` candidates | Family 1 |
| `StateConflictPreservesIndependentFields` | Safety | Effective canonical state keeps the latest reading and folder edits independently | Family 1 |
| `BookSaveIntentPreserved` | Safety | Once a save begins, the intended union of memory and disk book edits is not discarded | Family 2 |
| `TranslationDistinctVersionsPreserved` | Safety | Distinct translation version IDs are not collapsed merely because timestamps collide | Family 3 |
| `DictionaryEntriesMonotonic` | Structural | Dictionary merge never loses previously known entries | Family 2 |

## 6. Findings pending verification

### 6.1 Model-checkable

| ID | Description | Expected invariant violation | Bug family |
|---|---|---|---|
| M1 | Two conflicting `book*.dat` files carry disjoint edits; load keeps only newest | `BookConflictPreservesAllEdits` | Family 1 |
| M2 | Two conflicting `state*.json` files carry newer values for different fields; resolve keeps only newest whole file | `StateConflictPreservesIndependentFields` | Family 1 |
| M3 | Memory book edit races with newer disk book during save; save replaces memory with disk then rewrites | `BookSaveIntentPreserved` | Family 2 |
| M4 | Two translation versions have different edit IDs but identical second-level timestamps | `TranslationDistinctVersionsPreserved` | Family 3 |

### 6.2 Test-verifiable

| ID | Description | Suggested test approach |
|---|---|---|
| T1 | `update_reading_state` and `update_folder_path` can regress independent fields under interleaving external writes | Add an integration test with conflicting `state*.json` contents carrying different newer fields |
| T2 | Book save loses in-memory edit when disk changes between load and rename | Add a save test analogous to `save_merges_translation_with_concurrent_on_disk_change` for `book.dat` |

### 6.3 Code-review-only

| ID | Description | Suggested action |
|---|---|---|
| C1 | Decide whether newest-wins for `book.dat` / `state.json` is deliberate product policy or a known limitation | Product / design review before changing semantics |
| C2 | Decide whether second-level translation timestamps are sufficient as logical version identity | Review whether edit IDs or finer-grained clocks should replace them |

## 7. Reference pointers

- Key source files:
  - `library/src/library/library_book.rs:101-128`
  - `library/src/library/library_book.rs:187-223`
  - `library/src/library/library_book.rs:250-298`
  - `library/src/library/library_book.rs:335-381`
  - `library/src/library/library_book.rs:450-598`
  - `library/src/book/translation.rs:399-479`
  - `library/src/library/library_dictionary.rs:121-212`
  - `library/src/library.rs:302-340`
- Relevant history:
  - `bb1b075`, `f2ac258`, `e104b4a`, `d790bb6`, `b793938`
- No matching GitHub issues/PRs were found via repository search during this analysis.
