# Concurrency Audit Findings (Spec) — FLTS @ 2096371

User-reported symptom: "sometimes the app locks up in weird ways."

A whole-codebase concurrency review found no ABBA lock-ordering deadlock. The defects are
*blocking-inside-async* and *lock-held-across-slow-work*, concentrated in the native
sync/FFI subsystem. All line numbers refer to commit `2096371`.

## Critical

### C1 — Blocking Go FFI calls run on tokio workers and on the main thread at exit
`syncthing_sys::start`/`stop` (`syncthing-sys/src/lib.rs:65-90`) are synchronous,
potentially multi-second calls serialized on Go's global mutex. Called without
`spawn_blocking` from `library/src/sync/engine.rs:83` (start) and `:316-318` (stop).
On quit, `RunEvent::Exit` runs `block_on(shutdown())` on the **main thread**
(`site/src-tauri/src/lib.rs:251-261`); the 2-second `run_exit_step` timeout cannot fire
because `tokio::time::timeout` needs an await point and a synchronous FFI call has none.
A wedged `app.Stop` blocks the main thread forever → window won't close, force-kill needed.

**Fix:** wrap both FFI calls in `tokio::task::spawn_blocking`; make `SyncEngine::stop` async.

### C2 — `sync_task` mutex held across the blocking engine shutdown
`eval_sync` (`site/src-tauri/src/app.rs:541-544`): under Rust 2024, the `if let`
scrutinee's `MutexGuard` lives through the block, so the `sync_task` mutex is held across
`task.shutdown().await` (which includes the blocking Go stop from C1). Every sync command
(`sync_get_this_device`, `sync_list_devices`, `sync_list_pending` — all via
`AppState::sync_engine()` at `app.rs:413-421`) parks on that mutex; the sync/settings UI
freezes. Same shape for Anki at `app.rs:486-489`. `shutdown()` at `app.rs:704-711`
already does the correct two-statement take with an explanatory comment.

**Fix:** take the task out of the slot in a standalone statement, then await shutdown.

### C3 — `eval_config` / `eval_sync` are not serialized against each other
Overlapping entry points: `update_config` (`app.rs:433-455`), `sync_set_enabled`
(`app.rs:329-333`), `sync_wake` (`app.rs:340-357`, fires on every app foreground), startup
`eval_config` (`lib.rs:167-169`). Two concurrent `eval_sync` calls each pick a fresh port;
the Go engine is one-per-process and idempotently returns success to the second
`flts_st_start` **without binding its address** (`syncthing-core/wrapper.go:72-74`). The
loser polls its own dead port for the full 30 s `REST_READY_TIMEOUT` (`engine.rs:35`),
then stomps `sync_status` to Error. Sync appears wedged until app restart.

**Fix:** one `eval_lock: tokio::sync::Mutex<()>` serializing
`update_config` / `eval_config` / `eval_sync` / `wake_sync` end to end.

## Important

### I1 — `get_system_definition` blocks a tokio worker on std-mpsc `recv()`
`app.rs:1268-1285`: async command → `run_on_main_thread` → blocking `rx.recv()` with no
timeout. Each in-flight lookup parks a tokio worker; a burst of word taps can exhaust the
pool (default = #cores) and starve every other command. If the main loop stops pumping
(e.g. exit `block_on` from C1), `recv()` blocks forever.

**Fix:** `tokio::sync::oneshot` awaited under `tokio::time::timeout`.

### I2 — Unbounded retry loops in `LibraryBook::save()` spin while the book mutex is held
`library/src/library/library_book/mod.rs:556` (per-translation `loop`) and `:662` (book
`loop`) retry forever when the canonical file's mtime keeps changing between pre-save read
and rename. The embedded Syncthing engine writing `translation_*.dat` during a sync burst
is a persistent racer → full-CPU spin while every caller holds the book's `TracedMutex`,
freezing all reads of the open book (paragraph views, chapter lists) until the burst ends.

**Fix:** cap attempts (5) with a short backoff sleep; error on exhaustion (the saver's
retry/carry-dirty machinery at `translation_queue.rs:857-891` handles the error).

### I3 — Anki `run_pass` holds the `AnkiSyncState` mutex across the whole network pass
`site/src-tauri/src/app/anki_sync.rs:214-216`: `state.lock().await` then `sync_pass`
(per-card HTTP, 30 s timeout each). A long periodic pass makes the UI's "sync now" hang
for minutes behind the same mutex.

**Fix:** `try_lock` at the top of `run_pass`; bail with "already in progress" when contended.

### I4 — `wake_sync` health probe uses the client's full 30 s timeout
`app.rs:344-346` probes `my_id()` under the reqwest client's 30 s timeout
(`library/src/sync/control.rs:20`) in the app-foregrounding path where an unresponsive
engine is the *expected* case. Frontend awaits the invoke → up to 30 s frozen.

**Fix:** wrap the probe in a 2 s `tokio::time::timeout`.

### I5 — Stale-`Library` pinning race between `translate_paragraph` and `update_config`
`translate_paragraph` (`app.rs:829-845`) snapshots `library.borrow()` and initializes the
queue with it. Landing inside `update_config`'s stop→`eval_config` window builds a queue
pinning the old `Library`; its saves go to a detached instance (silent lost work). Read
paths already refuse to init the queue for this reason (`app.rs:906-909`); the write path
is unguarded.

**Fix:** queue getters re-read the library from the watch under the init lock;
`update_config` holds the queue-init locks across stop→library-swap so no queue can be
created against the old instance.

## Out of scope (Minor, deliberately deferred)
- Sync `RosterStore` / `Config::save` std-fs calls in async contexts (small files).
- `try_move_to_trash` blocking Finder IPC in `delete_book`.
- `get_or_create_translation` double-lock pattern (safe via `&mut self` exclusivity).
- Sequential `handle_file_change_event` processing (delay only, no loss).
- `TracedMutex` held-too-long watchdog instrumentation (recommended follow-up).
