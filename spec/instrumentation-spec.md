# Instrumentation spec: FLTS sync subsystem

Maps the generated TLA+ actions to Rust instrumentation points for trace generation.

## 1. Trace event schema

### Event envelope

```json
{
  "tag": "trace",
  "event": {
    "name": "<spec_action_name>",
    "arg": {
      "reading": "<reading_pos_or_nil>",
      "folder": "<folder_value_or_nil>"
    },
    "state": {
      "bookMainMTime": 0,
      "bookConflictCount": 0,
      "stateMainReading": "nil",
      "stateMainFolder": "nil",
      "stateConflictCount": 0,
      "translationMainMTime": 0,
      "translationVersionCount": 0,
      "translationConflictCount": 0,
      "dictionaryEntryCount": 0,
      "bookSaveStage": "idle",
      "translationSaveStage": "idle",
      "stateOpKind": "idle"
    }
  }
}
```

### State fields

| Implementation field / derived value | TLA+ field | Access notes |
|---|---|---|
| Canonical `book.dat` mtime | `state.bookMainMTime` | Read metadata for `<book>/book.dat` after the action |
| Count of sibling `book*.dat` conflicts | `state.bookConflictCount` | Count non-canonical `book*.dat` files in the book directory |
| Canonical `state.json.readingState` | `state.stateMainReading` | Emit `"nil"` when absent |
| Canonical `state.json.folderPath` | `state.stateMainFolder` | Encode as stable label or joined path string; emit `"nil"` when absent |
| Count of sibling `state*.json` conflicts | `state.stateConflictCount` | Count non-canonical `state*.json` files |
| Canonical translation file mtime | `state.translationMainMTime` | Read metadata for `translation_<src>_<tgt>.dat` |
| Number of versions in canonical translation history | `state.translationVersionCount` | Count paragraph-version nodes after merge/load |
| Count of sibling `translation_*.dat` conflicts | `state.translationConflictCount` | Count non-canonical translation files for the same pair |
| Number of entries in canonical dictionary | `state.dictionaryEntryCount` | Count dictionary pairs after load/merge |
| Save-stage marker for `book.dat` save path | `state.bookSaveStage` | `"idle"` before/after save, `"ready"` after the reconciliation branch inside save |
| Save-stage marker for translation save path | `state.translationSaveStage` | `"idle"` before/after save, `"ready"` after merge-before-write |
| User-state operation stage | `state.stateOpKind` | `"idle"`, `"reading"`, or `"folder"` |

### Message / argument fields

Only two actions require explicit arguments in the trace:

| Trace field | TLA+ usage | Source |
|---|---|---|
| `event.arg.reading` | `UpdateReadingStateReload(reading)` | Function argument to `update_reading_state` |
| `event.arg.folder` | `UpdateFolderPathReload(folder)` | Function argument to `update_folder_path` |

## 2. Action-to-code mapping

### 1. LoadBookFromMetadata

- **Spec action**: `LoadBookFromMetadata`
- **Code location**: `library/src/library/library_book.rs:335-381`
- **Trigger point**: After conflict selection / cleanup completes and after the chosen book has been loaded (`load_from_metadata` returns)
- **Trace event name**: `"LoadBookFromMetadata"`
- **Fields**: full state snapshot
- **Notes**: This is the newest-wins path for `book.dat`. Emit after `bookConflicts` have been deleted so the trace snapshot reflects the post-resolution directory state.

### 2. SaveBookBegin

- **Spec action**: `SaveBookBegin`
- **Code location**: `library/src/library/library_book.rs:555-575`
- **Trigger point**: After the `saved_book_last_modified > last_modified` branch has been evaluated and any overwrite from disk has happened, but before temp-file serialization
- **Trace event name**: `"SaveBookBegin"`
- **Fields**: full state snapshot
- **Notes**: This is the key instrumentation point for Family 2. Capture whether the runtime path replaced memory from disk before writing.

### 3. SaveBookFinish

- **Spec action**: `SaveBookFinish`
- **Code location**: `library/src/library/library_book.rs:577-598`
- **Trigger point**: After rename to canonical `book.dat` and after `last_modified` has been refreshed
- **Trace event name**: `"SaveBookFinish"`
- **Fields**: full state snapshot
- **Notes**: Emit after the canonical file is visible on disk.

### 4. UpdateReadingStateReload

- **Spec action**: `UpdateReadingStateReload`
- **Code location**: `library/src/library/library_book.rs:277-290`
- **Trigger point**: Immediately after `reload_user_state()` returns inside `update_reading_state`
- **Trace event name**: `"UpdateReadingStateReload"`
- **Fields**: full state snapshot + `event.arg.reading`
- **Notes**: This exposes the stale-snapshot boundary before the write happens.

### 5. UpdateReadingStatePersist

- **Spec action**: `UpdateReadingStatePersist`
- **Code location**: `library/src/library/library_book.rs:287-290` and `250-269`
- **Trigger point**: After `persist_user_state()` renames the temp file to `state.json`
- **Trace event name**: `"UpdateReadingStatePersist"`
- **Fields**: full state snapshot
- **Notes**: Emit after disk state is durable enough for subsequent reloads to observe it.

### 6. UpdateFolderPathReload

- **Spec action**: `UpdateFolderPathReload`
- **Code location**: `library/src/library/library_book.rs:294-297`
- **Trigger point**: Immediately after `reload_user_state()` returns inside `update_folder_path`
- **Trace event name**: `"UpdateFolderPathReload"`
- **Fields**: full state snapshot + `event.arg.folder`
- **Notes**: Symmetric to reading-state reload; needed to observe the cross-field overwrite hazard.

### 7. UpdateFolderPathPersist

- **Spec action**: `UpdateFolderPathPersist`
- **Code location**: `library/src/library/library_book.rs:294-297` and `250-269`
- **Trigger point**: After `persist_user_state()` renames the temp file to `state.json`
- **Trace event name**: `"UpdateFolderPathPersist"`
- **Fields**: full state snapshot
- **Notes**: Snapshot must include the canonical `state.json` fields after write.

### 8. ResolveReadingStateFile

- **Spec action**: `ResolveReadingStateFile`
- **Code location**: `library/src/library/library_book.rs:187-223`
- **Trigger point**: After newest candidate has been renamed to `state.json` and non-canonical state files have been removed
- **Trace event name**: `"ResolveReadingStateFile"`
- **Fields**: full state snapshot
- **Notes**: This is the load-time newest-wins path for user state.

### 9. LoadTranslationFromMetadata

- **Spec action**: `LoadTranslationFromMetadata`
- **Code location**: `library/src/library/library_book.rs:101-128`
- **Trigger point**: After conflicts have been merged into the canonical translation file and removed
- **Trace event name**: `"LoadTranslationFromMetadata"`
- **Fields**: full state snapshot
- **Notes**: The snapshot must show canonical history length after merge. This is the semantic-merge contrast path.

### 10. SaveTranslationBegin

- **Spec action**: `SaveTranslationBegin`
- **Code location**: `library/src/library/library_book.rs:473-505`
- **Trigger point**: After the newer-on-disk merge branch has completed, before temp-file write
- **Trace event name**: `"SaveTranslationBegin"`
- **Fields**: full state snapshot
- **Notes**: Emit after `translation.merge(saved_translation)` if that branch runs.

### 11. SaveTranslationFinish

- **Spec action**: `SaveTranslationFinish`
- **Code location**: `library/src/library/library_book.rs:507-540`
- **Trigger point**: After canonical translation rename and `last_modified` refresh
- **Trace event name**: `"SaveTranslationFinish"`
- **Fields**: full state snapshot
- **Notes**: Snapshot must reflect the canonical translation file after merge-preserving persistence.

### 12. LoadDictionaryFromMetadata

- **Spec action**: `LoadDictionaryFromMetadata`
- **Code location**: `library/src/library/library_dictionary.rs:121-149`
- **Trigger point**: After conflicting dictionary files have been merged into the canonical dictionary and removed
- **Trace event name**: `"LoadDictionaryFromMetadata"`
- **Fields**: full state snapshot
- **Notes**: This is another merge-preserving contrast path and helps validate that not all file types are newest-wins.

## 3. Special considerations

### 3.1 Watcher-driven entrypoints

- `library/src/library.rs:302-340` dispatches file watcher events into `reload_book`, `reload_translations`, and dictionary reload.
- If traces are collected from end-to-end tests, emit events from the concrete functions above, not from `handle_file_change_event`, to preserve 1:1 action mapping.

### 3.2 Stage markers are synthetic trace fields

- The Rust implementation does not store `bookSaveStage`, `translationSaveStage`, or `stateOpKind` as explicit fields.
- The harness should emit these markers from instrumentation context:
  - `"ready"` when inside the save/reconciliation window
  - `"idle"` before and after the window
  - `"reading"` / `"folder"` after reload and before persist in the user-state update functions

### 3.3 Counting conflicts

- Conflict counts should be computed from filenames:
  - `book*.dat` except `book.dat`
  - `state*.json` except `state.json`
  - `translation_*.dat` except the canonical pair filename
- Emit counts **after** the instrumented action so `ValidatePostState` checks the post-action directory state.

### 3.4 Translation version count

- For trace validation, count logical paragraph-version nodes in the canonical translation history, not raw sentences or words.
- This keeps the trace aligned with the TLA+ abstraction that treats versions as merge units.
