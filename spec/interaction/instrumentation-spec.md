# Instrumentation Spec — Frontend–Backend Command/Event Protocol

Maps each TLA+ action in `base.tla` to source code locations and defines
the NDJSON event schema for trace collection.

## Event Format

All events are NDJSON (one JSON object per line), keyed by task/actor:

```json
{"event":"<ActionName>","task":"t1","start":1234567,"end":1234568,...}
```

- `start`/`end`: monotonic nanosecond timestamps (for ViablePIDs ordering)
- `task`: task identifier mapped to TLC model value (e.g. `t1`, `t2`)
- `event`: must match the CASE branch name in `Trace.tla` MatchEvent

Frontend events use actor `"ui"`:
```json
{"event":"DeliverEvent","version":3,"start":..., "end":...}
```

## Action → Source Mapping

### ConfigChange
- **Source**: `site/src-tauri/src/app.rs:104-173` (update_config → eval_config)
- **Emit at**: After `eval_config` returns (line 138)
- **Fields**: `{"event":"ConfigChange","task":"<thread>","newLib":<id>,"start":...,"end":...}`
- **Notes**: `newLib` is the new library ID (monotonically increasing counter)

### BeginWorker
- **Source**: `site/src-tauri/src/app/translation_queue.rs:106-146`
- **Emit at**: Start of translator worker task (line 108, inside tokio::spawn)
- **Fields**: `{"event":"BeginWorker","task":"<tokio_task_id>","book":"<book_id>","lib":<lib_id>,"start":...,"end":...}`
- **Notes**: `lib` is the library ID captured at TranslationQueue::init time

### WorkerReadParagraph
- **Source**: `site/src-tauri/src/app/translation_queue.rs:227-237`
- **Emit at**: After `paragraph_view()` call (line 231)
- **Fields**: `{"event":"WorkerReadParagraph","task":"<id>","start":...,"end":...}`

### WorkerCallAPI
- **Source**: `site/src-tauri/src/app/translation_queue.rs:311-313`
- **Emit at**: After `translator.get_translation()` returns
- **Fields**: `{"event":"WorkerCallAPI","task":"<id>","start":...,"end":...}`

### WorkerStoreResult
- **Source**: `site/src-tauri/src/app/translation_queue.rs:330-334`
- **Emit at**: After `add_paragraph_translation()` (line 334)
- **Fields**: `{"event":"WorkerStoreResult","task":"<id>","start":...,"end":...}`

### WorkerSave
- **Source**: `site/src-tauri/src/app/translation_queue.rs:428-432`
- **Emit at**: After `book.save().await` (line 431)
- **Fields**: `{"event":"WorkerSave","task":"<id>","lib":<captured_lib_id>,"start":...,"end":...}`
- **Notes**: `lib` is the captured library ID, NOT the current one (for F1 detection)

### WorkerComputeSnapshot
- **Source**: `site/src-tauri/src/app/translation_queue.rs:446-469`
- **Emit at**: After `lv.list_books(...)` returns (line 467)
- **Fields**: `{"event":"WorkerComputeSnapshot","task":"<id>","start":...,"end":...}`

### WorkerEmit
- **Source**: `site/src-tauri/src/app/translation_queue.rs:468-469`
- **Emit at**: After `app.emit("library_updated", books)` (line 469)
- **Fields**: `{"event":"WorkerEmit","task":"<id>","start":...,"end":...}`

### BeginTauri
- **Source**: `site/src-tauri/src/app/library_view.rs:269-301` (import), `:327-377` (move/delete/mark)
- **Emit at**: Entry of Tauri command handler
- **Fields**: `{"event":"BeginTauri","task":"<thread>","book":"<book_id>","start":...,"end":...}`

### TauriModify
- **Source**: `site/src-tauri/src/app/library_view.rs:277-279` (create_book), `:349` (delete_book), `:335-336` (update_folder)
- **Emit at**: After the mutation call returns
- **Fields**: `{"event":"TauriModify","task":"<thread>","start":...,"end":...}`

### TauriComputeSnapshot
- **Source**: `site/src-tauri/src/app/library_view.rs:282,297,339,350`
- **Emit at**: After `self.list_books(target_language).await` returns
- **Fields**: `{"event":"TauriComputeSnapshot","task":"<thread>","start":...,"end":...}`

### TauriEmit
- **Source**: `site/src-tauri/src/app/library_view.rs:283,298,340,351`
- **Emit at**: After `self.app.emit("library_updated", books)` returns
- **Fields**: `{"event":"TauriEmit","task":"<thread>","start":...,"end":...}`

### BeginWatcher
- **Source**: `site/src-tauri/src/app.rs:187-245` (handle_file_change_event)
- **Emit at**: Entry of handler, after reading library reference (line 188)
- **Fields**: `{"event":"BeginWatcher","task":"<thread>","book":"<book_id>","start":...,"end":...}`

### WatcherReload
- **Source**: `library/src/library.rs:308-316` → `library_book.rs:512-519`
- **Emit at**: After reload_book/reload_translations completes
- **Fields**: `{"event":"WatcherReload","task":"<thread>","start":...,"end":...}`

### WatcherComputeSnapshot
- **Source**: `site/src-tauri/src/app.rs:205-210`
- **Emit at**: After `library_view.list_books(...)` returns
- **Fields**: `{"event":"WatcherComputeSnapshot","task":"<thread>","start":...,"end":...}`

### WatcherEmit
- **Source**: `site/src-tauri/src/app.rs:208-211`
- **Emit at**: After `self.app.emit("library_updated", ...)` returns
- **Fields**: `{"event":"WatcherEmit","task":"<thread>","start":...,"end":...}`

### DeliverEvent (UI actor)
- **Source**: `site/src/lib/data/tauri.ts:7-42` (eventToReadable)
- **Emit at**: Inside the `listen` callback, after `setter(event.payload)` (line 12)
- **Fields**: `{"event":"DeliverEvent","version":<snapshot_version>,"start":...,"end":...}`
- **Notes**: Logged on the `"ui"` actor (not a Task). Version is the snapshot version from the payload.

### MarkWordVisible
- **Source**: `site/src-tauri/src/app/library_view.rs:355-377`
- **Emit at**: After `book.save().await` (line 372-373)
- **Fields**: `{"event":"MarkWordVisible","task":"<thread>","book":"<book_id>","start":...,"end":...}`

### AppClose
- **Source**: `site/src-tauri/src/lib.rs` (window close / process exit)
- **Emit at**: On app close signal (if instrumented) or inferred from trace end
- **Fields**: `{"event":"AppClose","task":"main","start":...,"end":...}`

## Preprocessor Requirements

The trace preprocessor (`harness/interaction/preprocess.py`) must:

1. **Map task IDs**: Assign tokio task IDs / thread IDs to `t1`, `t2`, etc.
2. **Map book IDs**: Assign book UUIDs to `b1`, `b2`, etc.
3. **Assign library IDs**: Monotonically increasing counter per ConfigChange
4. **Group by actor**: Output JSON object keyed by actor name (task + "ui")
5. **Sort within actor**: Events within each actor sorted by `start` timestamp
6. **Output format**: Single JSON file with structure:
   ```json
   {
     "t1": [{"event":"BeginWorker","task":"t1","book":"b1","lib":1,"start":100,"end":110}, ...],
     "t2": [{"event":"BeginTauri","task":"t2","book":"b1","start":200,"end":210}, ...],
     "ui": [{"event":"DeliverEvent","version":2,"start":300,"end":310}, ...]
   }
   ```

## Trace Collection Strategy

### Backend (Rust)
Instrument using a lightweight trace macro wrapper:
```rust
macro_rules! trace_event {
    ($event:expr, $($key:tt : $val:expr),*) => {
        if let Some(writer) = TRACE_WRITER.get() {
            let start = std::time::Instant::now();
            // ... action happens ...
            let end = std::time::Instant::now();
            writer.write_event(json!({
                "event": $event,
                "task": current_task_id(),
                $($key: $val,)*
                "start": start.as_nanos(),
                "end": end.as_nanos()
            }));
        }
    }
}
```

### Frontend (TypeScript)
Wrap `eventToReadable` listeners:
```typescript
listen<T>(eventName, (event) => {
    const start = performance.now();
    setter(event.payload);
    const end = performance.now();
    traceLog({ event: "DeliverEvent", version: extractVersion(event.payload), start, end });
});
```
