# Instrumentation Spec: FLTS Mutex / Lock Safety

Maps TLA+ spec actions to Rust source code locations for trace harness generation.

## Section 1: Trace Event Schema

### Event Envelope (Category B — Per-Task Timebox)

Each task emits events to its own trace array with `[start, end]` intervals:

```json
{
  "event": "<action_name>",
  "start": <monotonic_ns>,
  "end": <monotonic_ns>,
  "book": "<book_id>",
  "translation": "<translation_id>",
  "dictionary": "<dictionary_id>",
  "paragraph": "<paragraph_id>",
  "state": {
    "bookLockHolder": "<task_id_or_none>",
    "transLockHolder": "<task_id_or_none>",
    "dictLockHolder": "<task_id_or_none>",
    "queueLockHolder": "<task_id_or_none>"
  }
}
```

### Preprocessed Trace Format

The trace preprocessor groups per-task events and compresses timestamps into dense integers:

```json
{
  "t1": [
    {"event": "BeginWatcher", "start": 0, "end": 0, "book": "b1", "state": {...}},
    {"event": "WatcherAcqBook", "start": 1, "end": 2, "state": {...}},
    ...
  ],
  "t2": [
    {"event": "BeginTauriList", "start": 0, "end": 0, "book": "b1", "state": {...}},
    ...
  ]
}
```

### State Fields

| Implementation field | TLA+ variable | Access method |
|---------------------|---------------|---------------|
| Book's Mutex guard ownership | `bookLock[b]` | Shadow variable tracking `.lock().await` holder |
| Translation's Mutex guard ownership | `transLock[tr]` | Shadow variable tracking `.lock().await` holder |
| Dictionary's Mutex guard ownership | `dictLock[d]` | Shadow variable tracking `.lock().await` holder |
| TranslationQueue.state Mutex ownership | `queueLock` | Shadow variable tracking `.lock().await` holder |

**Note**: tokio::sync::Mutex does not expose its holder identity. Instrumentation must use shadow variables (e.g., `AtomicU64` set before/after `.lock().await` returns) or wrapper types that record the acquiring task ID.

### Task Identity

Each async task needs a unique string identifier. Options:
- Use `tokio::task::Id` (via `tokio::task::id()`)
- Assign sequential IDs at task spawn (preferred for deterministic mapping to TLA+ constants)

## Section 2: Action-to-Code Mapping

### Watcher Task

#### BeginWatcher
- **Spec action**: `BeginWatcher(t, b)`
- **Code location**: `site/src-tauri/src/app.rs:187-190` — start of `handle_file_change_event`
- **Trigger**: AFTER task dispatched, at entry to `handle_file_change_event`
- **Event name**: `"BeginWatcher"`
- **Fields**: `book` (from `ChangeEvent`'s book UUID)
- **Notes**: The watcher processes one book per `ChangeEvent::Book` dispatch

#### WatcherAcqBook
- **Spec action**: `WatcherAcqBook(t)`
- **Code location**: `library/src/library.rs:306` — `self.get_book(&path).await`
  - This calls `library_book.rs:254-260` — `LibraryBook::load()` / lock acquire
- **Trigger**: AFTER `get_book()` returns (lock acquired)
- **Event name**: `"WatcherAcqBook"`
- **Fields**: `state.bookLockHolder` = this task
- **Notes**: `get_book` acquires `books_cache` RwLock (read) then book Mutex. We model only the Mutex.

#### WatcherWaitBook
- **Spec action**: `WatcherWaitBook(t)`
- **Code location**: Same as WatcherAcqBook — emitted BEFORE `.lock().await` returns when contention detected
- **Trigger**: BEFORE `lock().await` (when poll returns Pending)
- **Event name**: `"WatcherWaitBook"`
- **Fields**: none (contention signal only)
- **Notes**: Only emitted if the lock is not immediately available. Requires custom Mutex wrapper or tokio tracing subscriber.

#### WatcherAcqTrans
- **Spec action**: `WatcherAcqTrans(t, tr)`
- **Code location**: `library/src/library/library_book.rs:369-377` — inside `get_or_create_translation`
  - Also `library_book.rs:400-480` — translation merge/load paths
- **Trigger**: AFTER `translation.lock().await` returns
- **Event name**: `"WatcherAcqTrans"`
- **Fields**: `translation` (language pair key), `state.transLockHolder` = this task

#### WatcherRelTrans
- **Spec action**: `WatcherRelTrans(t)`
- **Code location**: `library/src/library/library_book.rs:480` — guard dropped (scope exit)
- **Trigger**: AFTER Mutex guard is dropped
- **Event name**: `"WatcherRelTrans"`
- **Fields**: `state.transLockHolder` = "none"

#### WatcherAcqDict
- **Spec action**: `WatcherAcqDict(t, d)`
- **Code location**: `library/src/library/library_dictionary.rs:234-240` — `DictionaryCache::get_dictionary`
- **Trigger**: AFTER `dictionary.lock().await` returns
- **Event name**: `"WatcherAcqDict"`
- **Fields**: `dictionary` (language pair key), `state.dictLockHolder` = this task

#### WatcherRelDict
- **Spec action**: `WatcherRelDict(t)`
- **Code location**: `library/src/library/library_dictionary.rs:354` — guard dropped
- **Trigger**: AFTER Mutex guard dropped
- **Event name**: `"WatcherRelDict"`
- **Fields**: `state.dictLockHolder` = "none"

#### WatcherRelBook
- **Spec action**: `WatcherRelBook(t)`
- **Code location**: `library/src/library.rs:340` — end of `handle_file_change_event` (book guard dropped)
- **Trigger**: AFTER book Mutex guard dropped
- **Event name**: `"WatcherRelBook"`
- **Fields**: `state.bookLockHolder` = "none"

---

### TauriList Task (list_book_chapter_paragraphs → get_or_create_translation)

#### BeginTauriList
- **Spec action**: `BeginTauriList(t, b)`
- **Code location**: `site/src-tauri/src/app/library_view.rs:184` — entry to `list_book_chapter_paragraphs`
- **Trigger**: AFTER command dispatch, at function entry
- **Event name**: `"BeginTauriList"`
- **Fields**: `book` (from command parameter)

#### TauriListAcqBook
- **Spec action**: `TauriListAcqBook(t)`
- **Code location**: `site/src-tauri/src/app/library_view.rs:191-195` — `library.get_book()`
- **Trigger**: AFTER book lock acquired
- **Event name**: `"TauriListAcqBook"`
- **Fields**: `state.bookLockHolder` = this task

#### TauriListWaitBook
- **Spec action**: `TauriListWaitBook(t)`
- **Code location**: Same as TauriListAcqBook — contention case
- **Event name**: `"TauriListWaitBook"`
- **Notes**: Same pattern as WatcherWaitBook

#### TauriListGetTransFirst
- **Spec action**: `TauriListGetTransFirst(t, tr)`
- **Code location**: `library/src/library/library_book.rs:375` — first `.lock().await` in `get_or_create_translation`
- **Trigger**: AFTER first trans lock acquired
- **Event name**: `"TauriListGetTransFirst"`
- **Fields**: `translation`, `state.transLockHolder` = this task
- **Notes**: This is the `if let Some(t) = translations.get(key) { t.lock().await }` path

#### TauriListGetTransRelFirst
- **Spec action**: `TauriListGetTransRelFirst(t)`
- **Code location**: `library/src/library/library_book.rs:376` — guard dropped at end of `if let` block
- **Trigger**: AFTER first guard dropped
- **Event name**: `"TauriListGetTransRelFirst"`
- **Fields**: `state.transLockHolder` = "none"
- **Notes**: The temporary lock/unlock between first check and second acquire is the key Family 3 pattern

#### TauriListGetTransSecond
- **Spec action**: `TauriListGetTransSecond(t)`
- **Code location**: `library/src/library/library_book.rs:377` — second `.lock().await` (re-acquire for insert check)
- **Trigger**: AFTER second trans lock acquired
- **Event name**: `"TauriListGetTransSecond"`
- **Fields**: `state.transLockHolder` = this task

#### TauriListGetTransRelSecond
- **Spec action**: `TauriListGetTransRelSecond(t)`
- **Code location**: `library/src/library/library_book.rs:396` — end of `get_or_create_translation`
- **Trigger**: AFTER guard dropped
- **Event name**: `"TauriListGetTransRelSecond"`
- **Fields**: `state.transLockHolder` = "none"

#### TauriListAcqTransParagraph
- **Spec action**: `TauriListAcqTransParagraph(t)`
- **Code location**: `site/src-tauri/src/app/library_view.rs:200-210` — per-paragraph translation lock
- **Trigger**: AFTER trans lock acquired for paragraph access
- **Event name**: `"TauriListAcqTransParagraph"`
- **Fields**: `state.transLockHolder` = this task

#### TauriListRelTransParagraph
- **Spec action**: `TauriListRelTransParagraph(t)`
- **Code location**: `site/src-tauri/src/app/library_view.rs:215` — guard dropped after paragraph read
- **Trigger**: AFTER guard dropped
- **Event name**: `"TauriListRelTransParagraph"`
- **Fields**: `state.transLockHolder` = "none"

#### TauriListDone
- **Spec action**: `TauriListDone(t)`
- **Code location**: `site/src-tauri/src/app/library_view.rs:218` — return from function
- **Trigger**: AFTER book lock released, function returning
- **Event name**: `"TauriListDone"`
- **Fields**: `state.bookLockHolder` = "none"

---

### Translator Task (translation_queue::translate → handle_request)

#### BeginTranslator
- **Spec action**: `BeginTranslator(t, b, p)`
- **Code location**: `site/src-tauri/src/app/translation_queue.rs:155` — entry to `translate()`
- **Trigger**: AFTER `translate()` called
- **Event name**: `"BeginTranslator"`
- **Fields**: `book`, `paragraph` (from function parameters)

#### TranslatorCheckDedup
- **Spec action**: `TranslatorCheckDedup(t)`
- **Code location**: `site/src-tauri/src/app/translation_queue.rs:160-163` — `state.lock().await`
- **Trigger**: AFTER queue lock acquired
- **Event name**: `"TranslatorCheckDedup"`
- **Fields**: `state.queueLockHolder` = this task

#### TranslatorCheckDedupRead
- **Spec action**: `TranslatorCheckDedupRead(t)`
- **Code location**: `site/src-tauri/src/app/translation_queue.rs:164-170` — check `active_requests` map + release
- **Trigger**: AFTER checking the map and releasing the queue lock
- **Event name**: `"TranslatorCheckDedupRead"`
- **Fields**: `state.queueLockHolder` = "none"
- **Notes**: If dedup hit (already in progress), the translator skips to done. The trace event captures the outcome via subsequent events (no SendRequest = dedup detected).

#### TranslatorSendRequest
- **Spec action**: `TranslatorSendRequest(t)`
- **Code location**: `site/src-tauri/src/app/translation_queue.rs:171-175` — network call (outside lock)
- **Trigger**: AFTER sending request to translation service
- **Event name**: `"TranslatorSendRequest"`
- **Fields**: none
- **Notes**: This occurs between dedup check (lock released) and result insertion (lock re-acquired). The TOCTOU window is here: another translator for the same (book, paragraph) can pass the dedup check while this request is in-flight.

#### TranslatorInsertMap
- **Spec action**: `TranslatorInsertMap(t)`
- **Code location**: `site/src-tauri/src/app/translation_queue.rs:176-180` — re-acquire queue lock, insert into map
- **Trigger**: AFTER re-acquiring queue lock and inserting result
- **Event name**: `"TranslatorInsertMap"`
- **Fields**: `state.queueLockHolder` = this task

#### TranslatorRelQueue
- **Spec action**: `TranslatorRelQueue(t)`
- **Code location**: `site/src-tauri/src/app/translation_queue.rs:181` — release queue lock
- **Trigger**: AFTER queue lock released
- **Event name**: `"TranslatorRelQueue"`
- **Fields**: `state.queueLockHolder` = "none"

#### TranslatorAcqBook
- **Spec action**: `TranslatorAcqBook(t)`
- **Code location**: `site/src-tauri/src/app/translation_queue.rs:206-220` — `handle_request()` acquires book lock
- **Trigger**: AFTER book lock acquired
- **Event name**: `"TranslatorAcqBook"`
- **Fields**: `state.bookLockHolder` = this task

#### TranslatorWaitBook
- **Spec action**: `TranslatorWaitBook(t)`
- **Code location**: Same as TranslatorAcqBook — contention case
- **Event name**: `"TranslatorWaitBook"`

#### TranslatorGetTrans
- **Spec action**: `TranslatorGetTrans(t, tr)`
- **Code location**: `site/src-tauri/src/app/translation_queue.rs:225-240` — `get_or_create_translation()` under book lock
- **Trigger**: AFTER translation reference obtained (inside book lock scope)
- **Event name**: `"TranslatorGetTrans"`
- **Fields**: `translation` (language pair key)
- **Notes**: Translation is accessed while book lock is held; trans lock is NOT acquired here (just get Arc<Mutex<>>).

#### TranslatorRelBook
- **Spec action**: `TranslatorRelBook(t)`
- **Code location**: `site/src-tauri/src/app/translation_queue.rs:245` — book lock released before translation work
- **Trigger**: AFTER book lock released
- **Event name**: `"TranslatorRelBook"`
- **Fields**: `state.bookLockHolder` = "none"

#### TranslatorDoTranslation
- **Spec action**: `TranslatorDoTranslation(t)`
- **Code location**: `site/src-tauri/src/app/translation_queue.rs:250-280` — actual translation (network call, no locks held)
- **Trigger**: AFTER translation API call returns
- **Event name**: `"TranslatorDoTranslation"`
- **Fields**: none
- **Notes**: No locks held during this step. This is the long-running network call.

#### TranslatorStoreResult
- **Spec action**: `TranslatorStoreResult(t)`
- **Code location**: `site/src-tauri/src/app/translation_queue.rs:285-310` — acquire trans lock, store result
- **Trigger**: AFTER trans lock acquired and result stored
- **Event name**: `"TranslatorStoreResult"`
- **Fields**: `translation`, `state.transLockHolder` = this task

#### TranslatorWorkerCleanup
- **Spec action**: `TranslatorWorkerCleanup(t)`
- **Code location**: `site/src-tauri/src/app/translation_queue.rs:315-330` — release trans lock, cleanup
- **Trigger**: AFTER trans lock released
- **Event name**: `"TranslatorWorkerCleanup"`
- **Fields**: `state.transLockHolder` = "none"

#### TranslatorDone
- **Spec action**: `TranslatorDone(t)`
- **Code location**: `site/src-tauri/src/app/translation_queue.rs:335` — return from translate/handle_request
- **Trigger**: AFTER task cleanup complete
- **Event name**: `"TranslatorDone"`
- **Fields**: none

---

### Saver Task (run_saver → save)

#### BeginSaver
- **Spec action**: `BeginSaver(t, b)`
- **Code location**: `site/src-tauri/src/app/translation_queue.rs:338-345` — `run_saver()` picks a book
- **Trigger**: AFTER saver selects a book to save
- **Event name**: `"BeginSaver"`
- **Fields**: `book`

#### SaverAcqBook
- **Spec action**: `SaverAcqBook(t)`
- **Code location**: `library/src/library/library_book.rs:522-530` — entry to `save()`, acquires book lock
- **Trigger**: AFTER book lock acquired
- **Event name**: `"SaverAcqBook"`
- **Fields**: `state.bookLockHolder` = this task
- **Notes**: save() holds the book lock for the entire duration (Family 1 starvation root cause)

#### SaverWaitBook
- **Spec action**: `SaverWaitBook(t)`
- **Code location**: Same as SaverAcqBook — contention case
- **Event name**: `"SaverWaitBook"`

#### SaverAcqTrans
- **Spec action**: `SaverAcqTrans(t, tr)`
- **Code location**: `library/src/library/library_book.rs:580-600` — iterate translations, acquire each
- **Trigger**: AFTER trans lock acquired (per-translation in save loop)
- **Event name**: `"SaverAcqTrans"`
- **Fields**: `translation`, `state.transLockHolder` = this task
- **Notes**: save() acquires/releases multiple translation locks sequentially while holding book lock

#### SaverRelTrans
- **Spec action**: `SaverRelTrans(t)`
- **Code location**: `library/src/library/library_book.rs:620` — trans guard dropped after serialization
- **Trigger**: AFTER trans lock released
- **Event name**: `"SaverRelTrans"`
- **Fields**: `state.transLockHolder` = "none"

#### SaverAcqDict
- **Spec action**: `SaverAcqDict(t, d)`
- **Code location**: `library/src/library/library_book.rs:650-670` — iterate dictionaries, acquire each
- **Trigger**: AFTER dict lock acquired
- **Event name**: `"SaverAcqDict"`
- **Fields**: `dictionary`, `state.dictLockHolder` = this task

#### SaverRelDict
- **Spec action**: `SaverRelDict(t)`
- **Code location**: `library/src/library/library_book.rs:690` — dict guard dropped
- **Trigger**: AFTER dict lock released
- **Event name**: `"SaverRelDict"`
- **Fields**: `state.dictLockHolder` = "none"

#### SaverRelBook
- **Spec action**: `SaverRelBook(t)`
- **Code location**: `library/src/library/library_book.rs:734` — end of save(), book guard dropped
- **Trigger**: AFTER book lock released
- **Event name**: `"SaverRelBook"`
- **Fields**: `state.bookLockHolder` = "none"

---

### TauriMark Task (mark_word_visible)

#### BeginTauriMark
- **Spec action**: `BeginTauriMark(t, b, tr)`
- **Code location**: `site/src-tauri/src/app/library_view.rs:355` — entry to `mark_word_visible`
- **Trigger**: AFTER command dispatch
- **Event name**: `"BeginTauriMark"`
- **Fields**: `book`, `translation`

#### TauriMarkAcqBook
- **Spec action**: `TauriMarkAcqBook(t)`
- **Code location**: `site/src-tauri/src/app/library_view.rs:360` — acquire book lock
- **Trigger**: AFTER book lock acquired
- **Event name**: `"TauriMarkAcqBook"`
- **Fields**: `state.bookLockHolder` = this task

#### TauriMarkAcqTrans
- **Spec action**: `TauriMarkAcqTrans(t)`
- **Code location**: `site/src-tauri/src/app/library_view.rs:365` — acquire trans lock (nested under book)
- **Trigger**: AFTER trans lock acquired
- **Event name**: `"TauriMarkAcqTrans"`
- **Fields**: `state.transLockHolder` = this task

#### TauriMarkRelTrans
- **Spec action**: `TauriMarkRelTrans(t)`
- **Code location**: `site/src-tauri/src/app/library_view.rs:370` — release trans lock after mark
- **Trigger**: AFTER trans lock released
- **Event name**: `"TauriMarkRelTrans"`
- **Fields**: `state.transLockHolder` = "none"

#### TauriMarkSave
- **Spec action**: `TauriMarkSave(t)`
- **Code location**: `site/src-tauri/src/app/library_view.rs:377` — save + release book lock
- **Trigger**: AFTER save completes and book lock released
- **Event name**: `"TauriMarkSave"`
- **Fields**: `state.bookLockHolder` = "none"
- **Notes**: This is a combined save+release step. The spec models it as atomic.

## Section 3: Special Considerations

### Mutex Holder Tracking

tokio `Mutex<T>` does not expose holder identity. Two approaches for instrumentation:

1. **Wrapper type** (recommended): Create a `TrackedMutex<T>` wrapper that stores the current holder ID in an `AtomicU64` alongside the inner `tokio::sync::Mutex<T>`. The wrapper's `lock()` method acquires the inner mutex, then atomically sets the holder, and returns a guard that clears the holder on drop.

2. **Shadow variables**: Maintain a separate `HashMap<LockId, AtomicU64>` that's updated before/after each `.lock().await` call. More invasive but doesn't require changing the Mutex type.

### Contention Detection (WaitBook events)

Detecting lock contention requires knowing whether `.lock().await` resolved immediately or was suspended. Options:
- **tokio tracing subscriber**: Subscribe to tokio's internal resource poll events. When a Mutex poll returns `Pending`, emit a WaitBook event.
- **Timed approach**: If `.lock().await` takes longer than a threshold (e.g., 1μs), emit WaitBook before AcqBook.
- **try_lock first**: Call `try_lock()` first; if it fails, emit WaitBook then call `lock().await`.

### Timestamp Sources

For Category B timebox traces:
- Use `std::time::Instant::now()` for monotonic timestamps
- Convert to nanoseconds relative to a trace-start epoch
- The preprocessor will compress these into dense integers for TLC

### Task Spawn Points

Instrument at spawn points to associate task IDs:

| Task type | Spawn location |
|-----------|---------------|
| Watcher | `site/src-tauri/src/app.rs:175` — `tokio::spawn(handle_file_change_event(...))` |
| TauriList | Tauri command handler — `#[tauri::command]` async dispatch |
| TauriMark | Tauri command handler — `#[tauri::command]` async dispatch |
| Translator | `site/src-tauri/src/app/translation_queue.rs:150` — worker task spawn |
| Saver | `site/src-tauri/src/app/translation_queue.rs:338` — saver loop iteration |

### RwLock Simplification

The spec models `books_cache` and `dict_cache` RwLocks as transparent (not instrumented). These are always read-acquired in the modeled paths and don't contribute to deadlock risk. If future analysis requires RwLock modeling, instrument `RwLock::read().await` and `RwLock::write().await` separately.

### Async Drop Timing

Rust's async Mutex guards are dropped synchronously (not async). The "release" event should be emitted immediately after the guard variable goes out of scope. For guards held across `.await` points, the drop happens after the last `.await` that uses the guard.

### Trace Buffer Flushing

Use per-task trace buffers to avoid synchronization overhead:
1. Each task writes to a thread-local `Vec<TraceEvent>`
2. On task completion (Done event), flush the buffer to a shared writer
3. The shared writer serializes to the output file with a global lock

This ensures per-task events are ordered correctly while minimizing tracing overhead.

### Family 3 Fault Injection (RefactoredGetTrans)

The `RefactoredGetTransHold` and `RefactoredGetTransSecond` actions in the spec model a *hypothetical* refactoring where `get_or_create_translation` holds the translation lock across both the existence check and the insert. These actions are NOT instrumented from real code — they are spec-only fault injection.

If testing the refactored variant, create a separate test binary that uses the "hold-across-both-checks" pattern and instruments it with `RefactoredGetTransHold` / `RefactoredGetTransSecond` event names.
