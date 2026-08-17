//! The state `eval_config` installs, behind a startup gate.
//!
//! The command surface (webview IPC, and the E2E bridge) starts serving before
//! the spawned startup task has opened the library or spawned the sync tasks.
//! Everything a command could see half-built lives here with private fields,
//! reachable only through accessors that await the startup outcome first.
//!
//! Its own module on purpose: Rust privacy is per-module *and its descendants*,
//! so command code elsewhere in the `app` tree cannot touch these fields — the
//! gate is a compile-time property, not a convention new commands must follow.
//! The `*_unchecked` / install / take methods are the startup and shutdown
//! plumbing, which by definition runs before or after readiness.

use std::sync::Arc;
use std::time::Duration;

use library::library::Library;
use library::sync::engine::SyncEngine;
use tokio::sync::{Mutex, watch};

use crate::app::anki_sync::{AnkiSyncTask, SyncReportDto, sync_now_or_err};
use crate::app::sync_daemon::SyncTask;

/// Upper bound on how long a command waits for startup; past it the caller gets
/// an error instead of an unbounded hang.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

pub struct GatedState {
    /// `None` = startup still running, `Some(Ok)` = installed, `Some(Err)` =
    /// startup failed (the message is handed to every waiting command).
    ready: watch::Sender<Option<Result<(), String>>>,
    library: Arc<watch::Sender<Option<Arc<Library>>>>,
    anki_sync_task: Mutex<Option<Arc<AnkiSyncTask>>>,
    sync_task: Mutex<Option<Arc<SyncTask>>>,
}

impl Default for GatedState {
    fn default() -> Self {
        Self::new()
    }
}

impl GatedState {
    pub fn new() -> Self {
        Self {
            ready: watch::channel(None).0,
            library: Arc::new(watch::channel(None).0),
            anki_sync_task: Mutex::new(None),
            sync_task: Mutex::new(None),
        }
    }

    /// Records the startup outcome. `send_replace`, not `send`: with zero live
    /// receivers `send` errors and leaves the stored value untouched, so later
    /// subscribers would read `None` forever.
    pub fn publish_ready(&self, outcome: Result<(), String>) {
        self.ready.send_replace(Some(outcome));
    }

    /// Waits for startup to settle. Startup's own error is propagated; a
    /// startup that never settles is bounded by [`READY_TIMEOUT`].
    pub async fn await_ready(&self) -> Result<(), String> {
        await_ready_on(self.ready.subscribe(), READY_TIMEOUT).await
    }

    // --- gated accessors: the only way in for commands ---

    pub async fn library(&self) -> Result<Arc<Library>, String> {
        self.await_ready().await?;
        // Ready implies installed: `eval_library_config` either sets it or fails.
        self.library
            .borrow()
            .clone()
            .ok_or_else(|| "library is not configured".to_string())
    }

    /// `None` means sync is off or its engine failed to start — a real answer,
    /// unlike "not started yet", which the gate absorbs.
    pub async fn sync_engine(&self) -> Result<Option<Arc<SyncEngine>>, String> {
        self.await_ready().await?;
        Ok(self.sync_task.lock().await.as_ref().map(|task| task.engine()))
    }

    pub async fn sync_anki_now(&self) -> anyhow::Result<SyncReportDto> {
        self.await_ready().await.map_err(|err| anyhow::anyhow!(err))?;
        sync_now_or_err(&self.anki_sync_task).await
    }

    // --- startup / shutdown plumbing: no readiness wait ---

    pub fn install_library(&self, library: Arc<Library>) {
        self.library.send_replace(Some(library));
    }

    /// For paths that must not block on readiness: shutdown flushes, and the
    /// file-watcher loop, which is a no-op until the library exists anyway.
    /// Owned clone, so no watch read-guard can cross a caller's await.
    pub fn library_unchecked(&self) -> Option<Arc<Library>> {
        self.library.borrow().clone()
    }

    pub fn subscribe_library(&self) -> watch::Receiver<Option<Arc<Library>>> {
        self.library.subscribe()
    }

    pub fn notify_library_changed(&self) {
        self.library.send_modify(|_| {});
    }

    /// The raw handle `TranslationQueue` writes book updates through. The one
    /// ungated reference out of here; queue construction needs the sender.
    pub fn library_sender(&self) -> Arc<watch::Sender<Option<Arc<Library>>>> {
        Arc::clone(&self.library)
    }

    pub async fn install_anki_task(&self, task: Arc<AnkiSyncTask>) {
        *self.anki_sync_task.lock().await = Some(task);
    }

    /// Take standalone: the slot mutex must not span the caller's shutdown await.
    pub async fn take_anki_task(&self) -> Option<Arc<AnkiSyncTask>> {
        self.anki_sync_task.lock().await.take()
    }

    pub async fn install_sync_task(&self, task: Arc<SyncTask>) {
        *self.sync_task.lock().await = Some(task);
    }

    /// See [`GatedState::take_anki_task`].
    pub async fn take_sync_task(&self) -> Option<Arc<SyncTask>> {
        self.sync_task.lock().await.take()
    }
}

/// Free fn so the wait's semantics are testable without a `GatedState`.
async fn await_ready_on(
    mut rx: watch::Receiver<Option<Result<(), String>>>,
    timeout: Duration,
) -> Result<(), String> {
    if let Some(outcome) = rx.borrow_and_update().clone() {
        return outcome;
    }
    let wait = async {
        loop {
            if rx.changed().await.is_err() {
                return Err("startup state was dropped".to_string());
            }
            if let Some(outcome) = rx.borrow_and_update().clone() {
                return outcome;
            }
        }
    };
    tokio::time::timeout(timeout, wait)
        .await
        .unwrap_or_else(|_| Err(format!("startup did not complete within {timeout:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn library_answers_at_once_when_startup_already_succeeded() {
        let state = GatedState::new();
        state.publish_ready(Ok(()));
        // No library installed, but the wait is over: the error is about the
        // library, not about startup.
        assert_eq!(
            state.library().await.err(),
            Some("library is not configured".to_string())
        );
    }

    #[tokio::test]
    async fn accessors_propagate_the_startup_error() {
        let state = GatedState::new();
        state.publish_ready(Err("library open failed".to_string()));
        let err = Some("library open failed".to_string());
        assert_eq!(state.library().await.err(), err);
        assert_eq!(state.sync_engine().await.err(), err);
    }

    #[tokio::test]
    async fn await_ready_waits_for_a_pending_startup() {
        let state = Arc::new(GatedState::new());
        let waiter = tokio::spawn({
            let state = state.clone();
            async move { state.await_ready().await }
        });
        // Must be parked on `changed()`, not answered from the initial value.
        tokio::task::yield_now().await;
        state.publish_ready(Ok(()));
        assert_eq!(waiter.await.unwrap(), Ok(()));
    }

    /// Must expire, so the short timeout races nothing.
    #[tokio::test]
    async fn await_ready_gives_up_instead_of_hanging() {
        let never = watch::channel(None).0;
        assert_eq!(
            await_ready_on(never.subscribe(), Duration::from_millis(10)).await,
            Err("startup did not complete within 10ms".to_string())
        );
    }
}
