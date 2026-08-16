# Concurrency Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate every lockup-causing concurrency defect found in the 2026-08-16 audit: blocking Go FFI on tokio/main threads, mutexes held across slow work, unserialized engine restarts, unbounded save retry spins, and a blocking channel recv in an async command.

**Architecture:** All fixes are local hardening of existing code — no new subsystems. The Go FFI moves onto `spawn_blocking` so timeouts get await points; a single `eval_lock` serializes every config/sync evaluation; task-slot mutexes are released before awaiting shutdowns; retry loops get caps + backoff; the dictionary command switches to an awaited oneshot.

**Tech Stack:** Rust 2024, tokio (`sync`, `time`, `rt-multi-thread` — all already enabled), Tauri 2, `async-trait` (already a dependency of both crates).

**Spec:** `docs/superpowers/plans/2026-08-16-concurrency-audit-spec.md` — read it first; each task cites its finding ID (C1–C3, I1–I5).

## Global Constraints

- Workspace root: `/Volumes/sources/flts`. Rust only — no frontend changes. (If any JS tooling is ever needed: pnpm, never npm.)
- Run tests with `cargo test -p app --lib` (Tauri backend crate is named `app`, lives in `site/src-tauri`) and `cargo test -p library`. `cargo test -p library sync::engine` and `-p syncthing-sys` link the real Go engine — they already build in this repo; do not skip them for engine-touching tasks.
- Every commit message ends with the two trailers exactly as in the Task 1 commit step (Co-Authored-By + Claude-Session).
- Lock-order discipline introduced by this plan (document, never violate): `eval_lock` → `translation_queue_init_lock` → `summary_generation_queue_init_lock`. Task-slot mutexes (`sync_task`, `anki_sync_task`) are leaf locks: take, clone/take the value, release — never `.await` other work while holding them.
- Do not reformat unrelated code. Match existing comment style (comments state constraints, not narration).

---

### Task 1: C1 — Route blocking Go FFI calls through `spawn_blocking`

The Go `flts_st_start`/`flts_st_stop` calls block for seconds (cert generation, DB open/flush) and today run directly on tokio workers — and, at app exit, on the main thread where `run_exit_step`'s `tokio::time::timeout` can never fire (no await point inside a synchronous FFI call). Wrapping them in `spawn_blocking` gives every caller a real await point, so the existing exit timeouts become effective.

**Files:**
- Modify: `library/src/sync/engine.rs:83` (start), `library/src/sync/engine.rs:315-319` (stop), `library/src/sync/engine.rs:520` (test)
- Modify: `site/src-tauri/src/app/sync_daemon.rs:154` (caller of stop)
- Test: `site/src-tauri/src/app.rs` (tests module, after the existing `run_exit_step` tests ~line 999)

**Interfaces:**
- Produces: `SyncEngine::stop` becomes `pub async fn stop(&self) -> Result<()>` (was sync). `SyncEngine::start` signature unchanged.
- Consumed by: Task 4's `eval_sync` (unchanged call shape — `task.shutdown().await` internally awaits the new async stop).

- [ ] **Step 1: Write the failing regression test** — in the `tests` module of `site/src-tauri/src/app.rs`, alongside the existing `run_exit_step` tests:

```rust
    /// C1 regression: a shutdown step whose slow work runs on the blocking
    /// pool must still be preemptable by run_exit_step's timeout. (A raw
    /// synchronous FFI call inside the future has no await point, so the
    /// timeout could never fire — that was the app-exit hang.)
    #[tokio::test]
    async fn exit_step_times_out_when_step_blocks_a_thread_via_spawn_blocking() {
        let started = Instant::now();
        let success = run_exit_step("blocked step", Duration::from_millis(50), async {
            let _ = tokio::task::spawn_blocking(|| {
                std::thread::sleep(Duration::from_millis(500));
            })
            .await;
        })
        .await;
        assert!(!success, "step must time out, not complete");
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "timeout must preempt the blocked thread, elapsed {:?}",
            started.elapsed()
        );
    }
```

- [ ] **Step 2: Run it** — `cargo test -p app --lib exit_step_times_out_when_step_blocks` — Expected: PASS immediately (it tests the mechanism the fix relies on; it exists to pin the pattern). The *failing* half of this task is the compile break in Step 4.

- [ ] **Step 3: Make `syncthing_sys::start` run on the blocking pool** — in `library/src/sync/engine.rs`, replace lines 83-84:

```rust
        syncthing_sys::start(&cfg.home, &addr, &api_key, cfg.loopback_only)
            .map_err(|e| anyhow!("starting syncthing engine failed: {e}"))?;
```

with:

```rust
        // The Go call blocks for the whole engine boot (cert generation, DB
        // open). Run it on the blocking pool so it can never pin a tokio
        // worker, and so callers' timeouts have an await point to fire at.
        {
            let home = cfg.home.clone();
            let addr = addr.clone();
            let api_key = api_key.clone();
            let loopback_only = cfg.loopback_only;
            tokio::task::spawn_blocking(move || {
                syncthing_sys::start(&home, &addr, &api_key, loopback_only)
            })
            .await
            .map_err(|e| anyhow!("syncthing start task panicked: {e}"))?
            .map_err(|e| anyhow!("starting syncthing engine failed: {e}"))?;
        }
```

- [ ] **Step 4: Make `SyncEngine::stop` async** — replace `library/src/sync/engine.rs:315-319`:

```rust
    /// Stops the engine cleanly. Idempotent on the Go side.
    pub fn stop(&self) -> Result<()> {
        syncthing_sys::stop().map_err(|e| anyhow!("stopping syncthing engine failed: {e}"))
    }
```

with:

```rust
    /// Stops the engine cleanly. Idempotent on the Go side. The Go call blocks
    /// for the full teardown (connection close, DB flush), so it runs on the
    /// blocking pool — exit-path timeouts must be able to preempt it.
    pub async fn stop(&self) -> Result<()> {
        tokio::task::spawn_blocking(syncthing_sys::stop)
            .await
            .map_err(|e| anyhow!("syncthing stop task panicked: {e}"))?
            .map_err(|e| anyhow!("stopping syncthing engine failed: {e}"))
    }
```

- [ ] **Step 5: Fix the two callers** —
  - `site/src-tauri/src/app/sync_daemon.rs:154`: change `if let Err(err) = self.engine.stop() {` to `if let Err(err) = self.engine.stop().await {`
  - `library/src/sync/engine.rs:520` (test `engine_starts_configures_and_stops`): change `engine.stop().expect("engine stops cleanly");` to `engine.stop().await.expect("engine stops cleanly");`

- [ ] **Step 6: Run the tests**

Run: `cargo test -p library sync:: && cargo test -p app --lib`
Expected: PASS (including the real-engine test `engine_starts_configures_and_stops`).

- [ ] **Step 7: Commit**

```bash
git add library/src/sync/engine.rs site/src-tauri/src/app/sync_daemon.rs site/src-tauri/src/app.rs docs/superpowers/plans/
git commit -m "fix: run blocking syncthing FFI start/stop on the blocking pool

The Go engine's start/stop are synchronous multi-second calls. Called
directly from async contexts they pinned tokio workers, and on app exit
they ran inside block_on on the main thread where run_exit_step's
timeout had no await point to fire at — a wedged engine stop hung the
whole app on quit (audit C1).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01MH7bebY7kKHqTzEc9XHrSe"
```

---

### Task 2: C2 — Release task-slot mutexes before awaiting shutdowns

Under Rust 2024, `if let Some(task) = self.sync_task.lock().await.take()` keeps the `MutexGuard` alive through the block, so the slot mutex is held across `task.shutdown().await` (seconds of engine teardown). Every sync command parks on that mutex via `AppState::sync_engine()` → frozen sync UI. `AppState::shutdown()` (app.rs:707-714) already does this correctly in two statements, with a comment; make the other two sites match it.

**Files:**
- Modify: `site/src-tauri/src/app.rs:486-489` (Anki, in `eval_config`), `site/src-tauri/src/app.rs:541-544` (sync, in `eval_sync`)

**Interfaces:** none change.

- [ ] **Step 1: Fix the sync site** — in `eval_sync`, replace:

```rust
        // Stop any prior task first (config may have changed).
        if let Some(task) = self.sync_task.lock().await.take() {
            info!("Stopping prior sync task before re-spawn");
            task.shutdown().await;
        }
```

with:

```rust
        // Stop any prior task first (config may have changed). Take the task
        // out in a standalone statement — under Rust 2024 an `if let` on the
        // lock().await temporary holds the slot mutex across the (slow) engine
        // shutdown, freezing every sync command that reads the slot.
        let prior = self.sync_task.lock().await.take();
        if let Some(task) = prior {
            info!("Stopping prior sync task before re-spawn");
            task.shutdown().await;
        }
```

- [ ] **Step 2: Fix the Anki site** — in `eval_config`, replace:

```rust
        // Stop any prior Anki sync task (config may have changed).
        if let Some(task) = self.anki_sync_task.lock().await.take() {
            info!("Stopping prior Anki sync task before re-spawn");
            task.shutdown().await;
        }
```

with:

```rust
        // Stop any prior Anki sync task (config may have changed). Standalone
        // take so the slot mutex is not held across the await (see eval_sync).
        let prior = self.anki_sync_task.lock().await.take();
        if let Some(task) = prior {
            info!("Stopping prior Anki sync task before re-spawn");
            task.shutdown().await;
        }
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p app --lib`
Expected: PASS. (No new unit test: exercising this requires a real `AppState`/AppHandle, which the crate's tests never construct. The guard-lifetime property is pinned by the code comment and re-checked in the Task 8 review.)

- [ ] **Step 4: Commit**

```bash
git add site/src-tauri/src/app.rs
git commit -m "fix: don't hold sync/anki task-slot mutexes across shutdown awaits

Rust 2024 if-let scrutinee lifetime kept the slot MutexGuard alive
through task.shutdown().await, so every sync command blocked behind an
engine restart (audit C2). Matches the two-statement pattern shutdown()
already uses.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01MH7bebY7kKHqTzEc9XHrSe"
```

---

### Task 3: I4 — Short-timeout engine health probe for `wake_sync`

`wake_sync` runs on every app foreground and probes `my_id()` under the REST client's 30 s timeout — in exactly the path where an unresponsive engine is the expected case. The frontend awaits the invoke, so that's up to 30 s of frozen UI. Add a 2-second probe helper (also used by Task 4's re-check under the eval lock).

**Files:**
- Modify: `site/src-tauri/src/app/sync_daemon.rs` (add helper + tests), `site/src-tauri/src/app.rs:340-357` (`wake_sync`)

**Interfaces:**
- Produces: `pub(crate) async fn probe_healthy(client: &dyn library::sync::control::SyncthingApi, timeout: Duration) -> bool` and `pub(crate) const WAKE_PROBE_TIMEOUT: Duration = Duration::from_secs(2);` in `crate::app::sync_daemon`. Task 4 calls both.

- [ ] **Step 1: Write the failing tests** — in the `tests` module of `site/src-tauri/src/app/sync_daemon.rs`:

```rust
    #[tokio::test]
    async fn probe_healthy_true_for_responsive_engine() {
        let api = MockSyncthing::new("SELF");
        assert!(probe_healthy(&api, Duration::from_secs(1)).await);
    }

    #[tokio::test]
    async fn probe_healthy_returns_false_quickly_when_my_id_hangs() {
        /// A client whose my_id never resolves — models the wedged engine a
        /// foregrounding iOS app probes. Only my_id is reachable from
        /// probe_healthy; every other method is unreachable in this test.
        struct HangingApi;
        #[async_trait::async_trait]
        impl library::sync::control::SyncthingApi for HangingApi {
            async fn my_id(&self) -> anyhow::Result<String> {
                std::future::pending().await
            }
            async fn list_devices(&self) -> anyhow::Result<Vec<library::sync::control::DeviceInfo>> {
                unreachable!()
            }
            async fn add_device(&self, _: &str, _: &str) -> anyhow::Result<()> {
                unreachable!()
            }
            async fn remove_device(&self, _: &str) -> anyhow::Result<()> {
                unreachable!()
            }
            async fn rename_device(&self, _: &str, _: &str) -> anyhow::Result<()> {
                unreachable!()
            }
            async fn set_device_addresses(&self, _: &str, _: Vec<String>) -> anyhow::Result<()> {
                unreachable!()
            }
            async fn connections(&self) -> anyhow::Result<std::collections::HashMap<String, bool>> {
                unreachable!()
            }
            async fn ensure_folder(&self, _: library::sync::control::FolderSpec) -> anyhow::Result<()> {
                unreachable!()
            }
            async fn set_options(&self, _: library::sync::control::OptionsPatch) -> anyhow::Result<()> {
                unreachable!()
            }
            async fn pending_devices(&self) -> anyhow::Result<Vec<library::sync::control::PendingDevice>> {
                unreachable!()
            }
            async fn folder_completion(&self, _: &str) -> anyhow::Result<f64> {
                unreachable!()
            }
        }

        let started = std::time::Instant::now();
        assert!(!probe_healthy(&HangingApi, Duration::from_millis(50)).await);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "probe must give up at its own timeout, elapsed {:?}",
            started.elapsed()
        );
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p app --lib probe_healthy`
Expected: FAIL to compile — `probe_healthy` not found.

- [ ] **Step 3: Implement the helper** — in `site/src-tauri/src/app/sync_daemon.rs`, after the `DEFAULT_POLL_INTERVAL` const:

```rust
/// Probe budget for "is the engine still reachable" checks (app wake). Much
/// shorter than the REST client's own 30 s timeout: on wake an unresponsive
/// engine is the *expected* case and the frontend awaits the invoke.
pub(crate) const WAKE_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// True when the engine's REST API answers `my_id` within `timeout`.
pub(crate) async fn probe_healthy(
    client: &dyn library::sync::control::SyncthingApi,
    timeout: Duration,
) -> bool {
    tokio::time::timeout(timeout, client.my_id())
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}
```

- [ ] **Step 4: Use it in `wake_sync`** — in `site/src-tauri/src/app.rs`, replace the probe:

```rust
        let healthy = match self.sync_engine().await {
            Some(engine) => engine.client().my_id().await.is_ok(),
            None => false,
        };
```

with:

```rust
        let healthy = match self.sync_engine().await {
            Some(engine) => {
                crate::app::sync_daemon::probe_healthy(
                    engine.client().as_ref(),
                    crate::app::sync_daemon::WAKE_PROBE_TIMEOUT,
                )
                .await
            }
            None => false,
        };
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p app --lib`
Expected: PASS, including both new probe tests.

- [ ] **Step 6: Commit**

```bash
git add site/src-tauri/src/app/sync_daemon.rs site/src-tauri/src/app.rs
git commit -m "fix: bound wake_sync's engine health probe to 2s

The foreground-wake probe ran under the REST client's 30s timeout in
exactly the path where an unreachable engine is expected, freezing the
awaited invoke for up to 30s (audit I4).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01MH7bebY7kKHqTzEc9XHrSe"
```

---

### Task 4: C3 + I5 — Serialize config/sync evaluation; stop queues pinning a stale Library

Two changes that land together because they share the new lock:

1. **C3:** a single `eval_lock` serializes `update_config`, `eval_config` (startup), and `wake_sync`'s restart, so two engine restarts can never interleave (the loser used to poll a dead port for 30 s and stomp status to Error).
2. **I5:** `update_config` holds both queue-init locks across stop → library swap, and the translation-queue getter re-reads the library from the watch **under its init lock** — so no queue can ever be built against the outgoing `Library` instance. The init locks are dropped *before* the slow `eval_sync` tail so translates never wait behind an engine restart.

Lock order (also add to the module docs): `eval_lock` → `translation_queue_init_lock` → `summary_generation_queue_init_lock`.

**Files:**
- Modify: `site/src-tauri/src/app.rs` — `AppState` struct + `new` (~230-305), `update_config` (~433-455), `eval_config` (~457-531 split into two fns), `wake_sync` (~340-357), `get_or_init_translation_queue` (~766-805), `translate_paragraph` (~829-845), `translate_chapter` (~847-899)

**Interfaces:**
- Produces: `get_or_init_translation_queue(&self)` loses its `library: Arc<Library>` parameter (it re-reads the watch itself). Private `eval_library_config(&self) -> anyhow::Result<(Config, PathBuf)>` = old `eval_config` body minus the `eval_sync` tail, returning the config + resolved library root. `eval_sync`'s doc comment gains "caller must hold `eval_lock`".
- Consumes: `probe_healthy` / `WAKE_PROBE_TIMEOUT` from Task 3.

- [ ] **Step 1: Add the lock** — in the `AppState` struct after `backfill_lock`:

```rust
    /// Serializes every config/sync evaluation (`update_config`, startup
    /// `eval_config`, `wake_sync`): the Go engine is one-per-process, so two
    /// interleaved restarts leave the loser polling a dead port for 30 s.
    /// Lock order: eval_lock → translation_queue_init_lock →
    /// summary_generation_queue_init_lock. Task-slot mutexes are leaves.
    eval_lock: Mutex<()>,
```

and in `AppState::new`'s struct literal, after `backfill_lock: ...`:

```rust
            eval_lock: Mutex::new(()),
```

- [ ] **Step 2: Split `eval_config`** — rename the existing body to a private fn and re-create the public entry:

```rust
    pub async fn eval_config(&self) -> anyhow::Result<()> {
        let _eval = self.eval_lock.lock().await;
        let (config, library_root) = self.eval_library_config().await?;
        self.eval_sync(&config, &library_root).await;
        Ok(())
    }

    /// Everything `eval_config` does short of the sync engine: migrate + open
    /// the library, (re)spawn the Anki task, point the watcher. Returns the
    /// config snapshot and resolved library root for the `eval_sync` tail.
    /// Caller must hold `eval_lock`.
    async fn eval_library_config(&self) -> anyhow::Result<(Config, PathBuf)> {
        let config = self.config.borrow().clone();
        // ... existing body of eval_config, verbatim, up to and including the
        // watcher.set_path block (app.rs:520-526) ...
        Ok((config, library_root))
    }
```

The moved body is byte-identical to today's `eval_config` except: (a) drop the final `self.eval_sync(&config, &library_root).await;` line, (b) end with `Ok((config, library_root))` instead of `Ok(())`. It contains the Task 2 two-statement Anki take. Also change `eval_sync`'s doc comment first line to end with: `Caller must hold `eval_lock`.`

- [ ] **Step 3: Rewrite `update_config`** — replace the whole fn:

```rust
    pub async fn update_config(&self, config: Config) -> anyhow::Result<()> {
        // Serialize against every other config/sync evaluation: concurrent
        // engine restarts leave one side polling a dead port for 30 s.
        let _eval = self.eval_lock.lock().await;

        // Hold both queue-init locks across stop → library swap so no command
        // can build a queue pinned to the outgoing Library instance (its
        // saves would go to a detached library — silent lost work). Dropped
        // before the slow eval_sync tail so translates don't queue behind an
        // engine restart.
        let (config, library_root) = {
            let _tq_init = self.translation_queue_init_lock.lock().await;
            let _sq_init = self.summary_generation_queue_init_lock.lock().await;

            // Translator settings (provider/key/model) are captured when the
            // translation queue is created. Reset it so the next translation
            // uses the latest config; the summary queue captures its
            // summarizer the same way.
            self.stop_translation_queue().await;
            self.stop_summary_generation_queue().await;

            // eval_library_config below swaps in a freshly-opened Library;
            // any book the stopped queue translated but had not yet saved
            // would be lost with the old instance. Flush everything first
            // (mirrors shutdown()).
            self.save_all().await;

            info!("config = {:?}", config);
            config.save(&self.config_path)?;
            self.config.send_replace(config);
            self.eval_library_config().await?
        };
        self.eval_sync(&config, &library_root).await;
        Ok(())
    }
```

- [ ] **Step 4: Guard `wake_sync`'s restart with the eval lock** — replace the tail of `wake_sync` (after the `healthy` check from Task 3):

```rust
        if healthy {
            return;
        }
        info!("Sync engine unreachable after wake; restarting");
        let _eval = self.eval_lock.lock().await;
        // Another evaluation may have restarted the engine while we waited on
        // the lock; don't bounce a healthy engine.
        if let Some(engine) = self.sync_engine().await {
            if crate::app::sync_daemon::probe_healthy(
                engine.client().as_ref(),
                crate::app::sync_daemon::WAKE_PROBE_TIMEOUT,
            )
            .await
            {
                return;
            }
        }
        let config = self.config.borrow().clone();
        match resolve_library_root(Some(&self.app)) {
            Ok(root) => self.eval_sync(&config, &root).await,
            Err(err) => warn!("wake_sync: cannot resolve library root: {err}"),
        }
```

- [ ] **Step 5: Make the translation-queue getter self-source its Library** — change the signature and head of `get_or_init_translation_queue`:

```rust
    async fn get_or_init_translation_queue(&self) -> anyhow::Result<Arc<TranslationQueue>> {
        if let Some(queue) = self.translation_queue.borrow().clone() {
            return Ok(queue);
        }

        let _guard = self.translation_queue_init_lock.lock().await;

        // Another caller may have populated the queue while we were waiting.
        if let Some(queue) = self.translation_queue.borrow().clone() {
            return Ok(queue);
        }

        // Re-read the library under the init lock: update_config holds this
        // lock across stop → library swap, so what we read here can never be
        // the outgoing instance.
        let library = self
            .library
            .borrow()
            .clone()
            .ok_or(AppError::NoLibraryError)?;

        let config = self.config.borrow().clone();
        // ... rest of the existing body unchanged (caches, summary queue,
        // TranslationQueue::init(library, ...), send_replace, Ok(queue)) ...
    }
```

- [ ] **Step 6: Fix the two callers** —

`translate_paragraph` becomes:

```rust
    pub async fn translate_paragraph(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
        model: TranslationModel,
        use_cache: bool,
    ) -> anyhow::Result<usize> {
        let queue = self.get_or_init_translation_queue().await?;
        queue
            .translate(book_id, paragraph_id, model, use_cache)
            .await
    }
```

`translate_chapter`: keep its own `let library = self.library.borrow().clone().ok_or(AppError::NoLibraryError)?;` (it needs it to collect untranslated paragraph ids), and change only the getter call to `let queue = self.get_or_init_translation_queue().await?;`.

- [ ] **Step 7: Compile and run the tests**

Run: `cargo build -p app && cargo test -p app --lib`
Expected: builds clean, all tests PASS. (No new unit test: every path here needs a constructed `AppState` with a Tauri `AppHandle`, which this crate's test setup cannot build. The serialization property is enforced structurally — one lock, all four entry points — and re-verified in the Task 8 review; end-to-end coverage comes from the existing E2E suite.)

- [ ] **Step 8: Commit**

```bash
git add site/src-tauri/src/app.rs
git commit -m "fix: serialize config/sync evaluation and unpin stale-Library queues

One eval_lock now covers update_config, startup eval_config, and
wake_sync's restart, so two engine restarts can't interleave (the loser
used to poll a dead port for 30s and stomp sync status to Error —
audit C3). update_config additionally holds the queue-init locks across
stop -> library swap and the translation-queue getter re-reads the
library under its init lock, so a translate command can no longer build
a queue pinned to the outgoing Library instance (audit I5).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01MH7bebY7kKHqTzEc9XHrSe"
```

---

### Task 5: I1 — Awaited oneshot (with timeout) in `get_system_definition`

The macOS dictionary command currently parks a tokio worker on a blocking std-mpsc `recv()` with no timeout while the main thread does per-lookup `dlopen` + `DCSCopyDefinitionMarkup`. A burst of word taps can exhaust the worker pool; a stalled main loop leaves the invoke pending forever.

**Files:**
- Modify: `site/src-tauri/src/app.rs:1259-1291` (`get_system_definition`)

**Interfaces:** command signature unchanged (frontend untouched).

- [ ] **Step 1: Replace the channel** — rewrite the macOS block of `get_system_definition`:

```rust
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();

        app.run_on_main_thread(move || {
            let _ = tx.send(library::system_dictionary::system_macos::get_definition(
                &word,
                &source_lang,
                &target_lang,
            ));
        })
        .map_err(|e| e.to_string())?;

        // Await without parking a tokio worker, and bound the wait so a
        // stalled main loop can't leave the invoke pending forever.
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => Err("system dictionary lookup was dropped".to_string()),
            Err(_) => Err("system dictionary lookup timed out".to_string()),
        }
    }
```

Delete the now-dead `let word = word.clone();` / `source_lang` / `target_lang` clones (the originals move into the closure) and the `use std::sync::mpsc::channel;` line.

- [ ] **Step 2: Build and test**

Run: `cargo build -p app && cargo test -p app --lib`
Expected: builds clean, tests PASS. (No unit test — the command needs a live `AppHandle` + main-thread pump; covered by manual verification in Task 8.)

- [ ] **Step 3: Commit**

```bash
git add site/src-tauri/src/app.rs
git commit -m "fix: await dictionary lookups on a oneshot with a 5s bound

get_system_definition blocked a tokio worker on a std-mpsc recv() with
no timeout while the main thread ran the lookup; rapid word taps could
exhaust the worker pool and a stalled main loop left the invoke pending
forever (audit I1).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01MH7bebY7kKHqTzEc9XHrSe"
```

---

### Task 6: I2 — Cap the `LibraryBook::save()` retry loops

Both save loops retry forever when the canonical file's mtime keeps changing between the pre-save read and the rename — and the embedded Syncthing engine delivering files during a sync burst is a persistent racer. The spin happens while the caller holds the book's `TracedMutex`, freezing every reader of the open book. Cap attempts with a short backoff; on exhaustion return an error (the translation-queue saver already has carry-dirty retry machinery for failed saves).

**Files:**
- Modify: `library/src/library/library_book/mod.rs` — constants near the top of the file, translation loop (~556-655), book loop (~662-744)

**Interfaces:** `save()`'s signature is unchanged; it can now fail with a "kept changing on disk" error after ~0.75 s of contention instead of spinning.

- [ ] **Step 1: Add the constants** — near the other file-level items of `library_book/mod.rs`:

```rust
/// Cap for the save/merge retry loops. Each retry means the canonical file
/// changed on disk between our pre-save read and the rename (e.g. a Syncthing
/// delivery); with a persistent racer an uncapped loop spins at full CPU while
/// the book mutex is held, freezing every reader of this book.
const MAX_SAVE_ATTEMPTS: u32 = 5;
const SAVE_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(50);
```

- [ ] **Step 2: Cap the translation loop** — change `loop {` at ~line 556 into a bounded loop. Shape (the `...` bodies are today's code, unchanged):

```rust
            let mut saved = false;
            for attempt in 1..=MAX_SAVE_ATTEMPTS {
                let translation_path_modified_pre_save = ...;   // unchanged
                ...merge-from-disk + trace emit, unchanged...

                if translation.changed {
                    ...write temp file, unchanged...
                    if (...mtime unchanged check, unchanged...) {
                        ...rename + bookkeeping, unchanged...
                        merged_translations.push(translation_arc.clone());
                        saved = true;
                        break;
                    }
                } else {
                    merged_translations.push(translation_arc.clone());
                    saved = true;
                    break;
                }

                // The canonical file changed mid-save; back off before the
                // re-read+merge so a sync burst can land instead of racing us.
                if attempt < MAX_SAVE_ATTEMPTS {
                    tokio::time::sleep(SAVE_RETRY_BACKOFF * attempt).await;
                }
            }
            if !saved {
                anyhow::bail!(
                    "translation {} kept changing on disk during save ({MAX_SAVE_ATTEMPTS} attempts)",
                    translation_path.display()
                );
            }
```

(`SAVE_RETRY_BACKOFF * attempt` works because `attempt` is `u32` and `Duration * u32` is defined.)

- [ ] **Step 3: Cap the book loop** — same transformation for `loop {` at ~line 662: `let mut saved = false; for attempt in 1..=MAX_SAVE_ATTEMPTS { ... }`, both existing `break`s become `saved = true; break;` (the identical-content skip at ~line 711 and the post-rename break at ~line 741), backoff before retry, then:

```rust
        if !saved {
            anyhow::bail!(
                "book {} kept changing on disk during save ({MAX_SAVE_ATTEMPTS} attempts)",
                book_path.display()
            );
        }
```

- [ ] **Step 4: Run the library save tests**

Run: `cargo test -p library library_book && cargo test -p library save`
Expected: PASS — the uncontended paths take exactly one iteration, so behavior is unchanged. (No deterministic unit test for the exhaustion path: forcing five consecutive mtime flips between our read and rename is inherently racy from a test. The cap itself is the safety property; existing save/merge tests are the regression net.)

- [ ] **Step 5: Run the full library suite**

Run: `cargo test -p library`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add library/src/library/library_book/mod.rs
git commit -m "fix: cap LibraryBook::save retry loops at 5 attempts with backoff

Both save loops retried forever while the canonical file kept changing
on disk — and the embedded Syncthing engine delivering files during a
sync burst is a persistent racer. The spin ran at full CPU with the
book mutex held, freezing every reader of the open book (audit I2).
On exhaustion save() now errors; the queue's saver already carries
dirty state forward and retries.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01MH7bebY7kKHqTzEc9XHrSe"
```

---

### Task 7: I3 — `try_lock` in Anki `run_pass`

`run_pass` holds the `AnkiSyncState` mutex across the entire network pass (per-card HTTP, 30 s timeout each). A user's "sync now" then hangs for minutes behind a periodic pass. With `try_lock`, a contended pass reports "already in progress" immediately — the watch status already tells the UI a sync is running.

**Files:**
- Modify: `site/src-tauri/src/app/anki_sync.rs:195-216` (`run_pass`)
- Test: same file, tests module

**Interfaces:** `run_pass`/`sync_now` signatures unchanged; new error case "anki sync already in progress".

- [ ] **Step 1: Write the failing test** — in the `tests` module of `anki_sync.rs`:

```rust
    #[tokio::test]
    async fn sync_now_reports_in_progress_instead_of_waiting() {
        let (_tmp, library) = seed_library_with_card("flts_anki_sync_busy").await;
        let mock: Arc<dyn AnkiConnect> = Arc::new(MockAnkiConnect::new());
        // Long interval so the periodic loop can't interfere mid-test.
        let task = AnkiSyncTask::init(library, mock, Duration::from_secs(3600), make_status_tx());

        // Model an in-flight pass by holding the state lock (run_pass holds it
        // for the whole pass).
        let in_flight = task.state.lock().await;

        let started = std::time::Instant::now();
        let err = task
            .sync_now()
            .await
            .expect_err("sync_now must not wait behind an in-flight pass");
        assert!(
            err.to_string().contains("in progress"),
            "error must say a sync is running; got {err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "must return immediately, took {:?}",
            started.elapsed()
        );

        drop(in_flight);
        task.shutdown().await;
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p app --lib sync_now_reports_in_progress`
Expected: FAIL — `sync_now` blocks on the held lock and the test times out, or (if the first periodic tick already returned) it succeeds instead of erroring. Either way: not passing.

Note: the periodic loop's first tick fires at init and may briefly hold the lock; the test's `lock().await` simply waits those few milliseconds out (mock is instant), then owns it.

- [ ] **Step 3: Implement** — in `run_pass`, move lock acquisition to the top and make it non-blocking. Replace the beginning of the fn:

```rust
async fn run_pass(
    client: &dyn AnkiConnect,
    library: &Arc<Library>,
    state: &Mutex<AnkiSyncState>,
    status_tx: &watch::Sender<AnkiSyncStatus>,
) -> anyhow::Result<SyncReportDto> {
    // One pass at a time, without queueing: a pass can hold this lock for
    // minutes (per-card HTTP), and both callers are better served by an
    // immediate answer — the periodic tick just skips (the in-flight pass is
    // already doing the work) and the UI's "sync now" reports instead of
    // hanging. Acquired before the status flip so a bail leaves status to the
    // pass that actually runs.
    let Ok(mut guard) = state.try_lock() else {
        anyhow::bail!("anki sync already in progress");
    };

    status_tx.send_modify(|s| s.state = AnkiSyncStatusState::Syncing);

    if let Err(err) = client.version().await {
        ...unchanged...
    }

    let now = tokio::time::Instant::now();
    match sync_pass(client, library.as_ref(), &mut guard, now).await {
        ...unchanged...
    }
}
```

and delete the old `let mut guard = state.lock().await;` line at ~214.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p app --lib anki`
Expected: PASS — the new test and all existing anki_sync tests (they run uncontended, `try_lock` always succeeds).

- [ ] **Step 5: Commit**

```bash
git add site/src-tauri/src/app/anki_sync.rs
git commit -m "fix: make anki run_pass non-blocking on the pass mutex

A pass holds the AnkiSyncState mutex for its full network duration;
the UI's sync-now invoke queued behind a periodic pass for minutes
(audit I3). try_lock + \"already in progress\" answers immediately; the
periodic tick skips since the in-flight pass is already doing the work.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01MH7bebY7kKHqTzEc9XHrSe"
```

---

### Task 8: Full verification + code review

**Files:** none modified (verification only; fix anything found, amend into the relevant commit or add a fixup commit).

- [ ] **Step 1: Full workspace test run**

Run: `cargo test -p library && cargo test -p app --lib && cargo test -p syncthing-sys`
Expected: all PASS.

- [ ] **Step 2: Lints**

Run: `cargo clippy -p app -p library --all-targets -- -D warnings && cargo fmt --check -p app -p library` (if the repo doesn't gate on clippy warnings, still read the output for new warnings introduced by these changes and fix those).
Expected: no new warnings from the changed files.

- [ ] **Step 3: Manual smoke check (dev app)** — launch with `cargo tauri dev` (never `pnpm tauri dev`) and verify: (a) app starts, library loads; (b) with sync enabled, toggling sync off/on in settings doesn't freeze the sync pane; (c) on macOS, a word lookup still shows a dictionary definition; (d) quitting the app exits promptly.

- [ ] **Step 4: Request code review** — use the superpowers:requesting-code-review skill: `BASE_SHA` = commit before Task 1's commit (`git rev-parse <task1>^`), `HEAD_SHA` = `git rev-parse HEAD`, DESCRIPTION = "Concurrency fixes for audit findings C1-C3, I1-I5", PLAN_OR_REQUIREMENTS = this file + the audit spec. Fix Critical/Important findings before declaring done.
