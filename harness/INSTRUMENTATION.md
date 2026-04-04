# FLTS trace harness guide

## Instrumented locations

- `library/src/library/library_book.rs`
  - `resolve_reading_state_file` emits `ResolveReadingStateFile`
  - `update_reading_state` emits `UpdateReadingStateReload` and `UpdateReadingStatePersist`
  - `update_folder_path` emits `UpdateFolderPathReload` and `UpdateFolderPathPersist`
  - `LibraryBook::load_from_metadata` emits `LoadBookFromMetadata`
  - `LibraryBook::save` emits `SaveBookBegin`, `SaveBookFinish`, `SaveTranslationBegin`, and `SaveTranslationFinish`
- `library/src/library/library_dictionary.rs`
  - `LibraryDictionary::load_from_metadata` emits `LoadDictionaryFromMetadata`
- `library/src/tla_trace.rs`
  - owns the NDJSON schema, file writer, and state-capture helpers
- `library/tests/trace_harness.rs`
  - contains the scenario tests that write `traces/*.ndjson`

## Adding a field

1. Add the field to `TraceState` in `library/src/tla_trace.rs`.
2. Populate it in `capture_state`.
3. Update `spec/instrumentation-spec.md`.
4. Update `spec/Trace.tla` `ValidatePostState`.

## Adding a new event

1. Add an emit call in the real code path using one of:
   - `tla_trace::emit_book_event`
   - `tla_trace::emit_translation_event`
   - `tla_trace::emit_dictionary_event`
2. Add a scenario in `library/tests/trace_harness.rs` that triggers it.
3. Add the action wrapper in `spec/Trace.tla`.
4. Add the expected event name to `harness/run.sh` coverage check.

## Moving a capture point

- Change the emit location in the Rust function directly.
- Re-run `bash harness/run.sh`.
- If trace validation later disagrees on post-state, the usual fix is moving the emit from before to after rename / merge / reload, not changing the schema.

## Rebuild / rerun

```bash
bash harness/run.sh
```
