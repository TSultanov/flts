//! Anki sync orchestration: a tokio task ticking
//! `library::anki::sync::sync_pass`, with `sync_now` serving the UI button
//! through the same path.
//!
//! Spawned from `AppState::eval_config`, opted out of with the
//! `ankiSyncEnabled` config setting. Status goes out on an `AppState`-owned
//! `watch::Sender<AnkiSyncStatus>`, forwarded as `anki_sync_status_changed`.

use std::sync::Arc;
use std::time::Duration;

use library::anki::connect::AnkiConnect;
use library::anki::sync::{AnkiSyncState, SyncReport, sync_pass};
use library::library::Library;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Drives the nav button's icon state machine.
#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AnkiSyncStatusState {
    /// Nothing has run yet, or no task is installed.
    #[default]
    Idle,
    Syncing,
    Ok,
    /// AnkiConnect answered but `sync_pass` failed.
    Err,
    /// The `version()` ping failed; the button hides in this state.
    Unreachable,
}

/// [`library::anki::sync::SyncReport`] plus `Serialize`, kept here so the
/// library needs no serde.
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReportDto {
    pub total_cards: usize,
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub persistent_failures: Vec<String>,
}

impl From<SyncReport> for SyncReportDto {
    fn from(value: SyncReport) -> Self {
        Self {
            total_cards: value.total_cards,
            attempted: value.attempted,
            succeeded: value.succeeded,
            failed: value.failed,
            persistent_failures: value.persistent_failures,
        }
    }
}

/// Snapshot of the latest sync attempt, pushed on every state transition.
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnkiSyncStatus {
    pub state: AnkiSyncStatusState,
    /// Unix epoch ms the last attempt finished.
    pub last_finished_at_ms: Option<i64>,
    /// Set while `state` is `Err` or `Unreachable`.
    pub last_error: Option<String>,
    /// Set while `state` is `Ok`.
    pub last_report: Option<SyncReportDto>,
}

pub struct AnkiSyncTask {
    state: Arc<Mutex<AnkiSyncState>>,
    client: Arc<dyn AnkiConnect>,
    library: Arc<Library>,
    status_tx: Arc<watch::Sender<AnkiSyncStatus>>,
    task_handle: Mutex<Option<JoinHandle<()>>>,
}

impl AnkiSyncTask {
    pub fn init(
        library: Arc<Library>,
        client: Arc<dyn AnkiConnect>,
        interval: Duration,
        status_tx: Arc<watch::Sender<AnkiSyncStatus>>,
    ) -> Arc<Self> {
        let state = Arc::new(Mutex::new(AnkiSyncState::new()));

        // Syncs as soon as a card lands on disk; Notify's single pending permit
        // collapses bursts into one follow-up pass.
        let wake = library.card_store().change_notify();

        let task = {
            let state = state.clone();
            let client = client.clone();
            let library = library.clone();
            let status_tx = status_tx.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                // `interval`'s immediate first tick is wanted: a pass runs
                // shortly after init.
                loop {
                    tokio::select! {
                        _ = ticker.tick() => {}
                        _ = wake.notified() => {}
                    }
                    let _ = run_pass(client.as_ref(), &library, &state, &status_tx).await;
                }
            })
        };

        Arc::new(Self {
            state,
            client,
            library,
            status_tx,
            task_handle: Mutex::new(Some(task)),
        })
    }

    pub async fn shutdown(&self) {
        if let Some(handle) = self.task_handle.lock().await.take() {
            handle.abort();
            let _ = handle.await;
        }
    }

    /// On-demand sync for the UI button, along the periodic tick's path:
    /// Syncing → version() → sync_pass → status update.
    pub async fn sync_now(&self) -> anyhow::Result<SyncReportDto> {
        run_pass(
            self.client.as_ref(),
            &self.library,
            &self.state,
            &self.status_tx,
        )
        .await
    }
}

/// `sync_anki_now`'s body, taken out of `AppState` so tests can drive it with
/// just a task slot.
pub async fn sync_now_or_err(
    task_slot: &Mutex<Option<Arc<AnkiSyncTask>>>,
) -> anyhow::Result<SyncReportDto> {
    let task = task_slot.lock().await.clone();
    match task {
        None => {
            anyhow::bail!("no anki sync task installed (library not configured or sync disabled)")
        }
        Some(task) => task.sync_now().await,
    }
}

/// One sync attempt with its status side effects, shared by the periodic tick
/// and `sync_now`.
async fn run_pass(
    client: &dyn AnkiConnect,
    library: &Arc<Library>,
    state: &Mutex<AnkiSyncState>,
    status_tx: &watch::Sender<AnkiSyncStatus>,
) -> anyhow::Result<SyncReportDto> {
    // One pass at a time, no queueing: a pass can hold this for minutes. Bail
    // before the status flip so it stays owned by the running pass.
    let Ok(mut guard) = state.try_lock() else {
        anyhow::bail!("anki sync already in progress");
    };

    status_tx.send_modify(|s| s.state = AnkiSyncStatusState::Syncing);

    if let Err(err) = client.version().await {
        warn!("anki version() probe failed: {err}");
        status_tx.send_replace(AnkiSyncStatus {
            state: AnkiSyncStatusState::Unreachable,
            last_finished_at_ms: Some(now_unix_ms()),
            last_error: Some(err.to_string()),
            last_report: None,
        });
        return Err(err);
    }

    let now = tokio::time::Instant::now();
    match sync_pass(client, library.as_ref(), &mut guard, now).await {
        Ok(report) => {
            if report.total_cards > 0 {
                info!(
                    "anki sync_pass: total={} attempted={} succeeded={} failed={} persistent={}",
                    report.total_cards,
                    report.attempted,
                    report.succeeded,
                    report.failed,
                    report.persistent_failures.len(),
                );
            }
            let dto: SyncReportDto = report.into();
            status_tx.send_replace(AnkiSyncStatus {
                state: AnkiSyncStatusState::Ok,
                last_finished_at_ms: Some(now_unix_ms()),
                last_error: None,
                last_report: Some(dto.clone()),
            });
            Ok(dto)
        }
        Err(err) => {
            warn!("anki sync_pass failed: {err}");
            status_tx.send_replace(AnkiSyncStatus {
                state: AnkiSyncStatusState::Err,
                last_finished_at_ms: Some(now_unix_ms()),
                last_error: Some(err.to_string()),
                last_report: None,
            });
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::anki::connect::MockAnkiConnect;
    use library::card::Card;
    use std::path::PathBuf;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!("{}_{}", prefix, uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    async fn seed_library_with_card(tmp_prefix: &str) -> (TempDir, Arc<Library>) {
        let tmp = TempDir::new(tmp_prefix);
        let library = Arc::new(Library::open(tmp.path.clone()).await.unwrap());
        let mut translations: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        translations.insert("verb".into(), vec!["мочь".into()]);
        let card = Card {
            version: 2,
            id: "flts_spa_rus_poder".into(),
            lemma: "poder".into(),
            translations,
            examples: vec![],
            anki_data: None,
        };
        library
            .card_store()
            .save(&card, "spa", "rus")
            .await
            .unwrap();
        (tmp, library)
    }

    fn make_status_tx() -> Arc<watch::Sender<AnkiSyncStatus>> {
        let (tx, _rx) = watch::channel(AnkiSyncStatus::default());
        Arc::new(tx)
    }

    #[tokio::test]
    async fn anki_sync_task_init_and_shutdown_does_not_panic() {
        let (_tmp, library) = seed_library_with_card("flts_anki_sync_smoke").await;
        let mock: Arc<dyn AnkiConnect> = Arc::new(MockAnkiConnect::new());
        let task = AnkiSyncTask::init(library, mock, Duration::from_millis(50), make_status_tx());
        task.shutdown().await;
    }

    #[tokio::test]
    async fn anki_sync_task_runs_pass_when_card_change_notify_fires() {
        // Long interval so the periodic ticker can't be what triggers the
        // pass — only the card-store wake from `save()` should drive it.
        let (_tmp, library) = seed_library_with_card("flts_anki_sync_wake").await;
        let mock_for_task: Arc<dyn AnkiConnect> = Arc::new(MockAnkiConnect::new());
        let task = AnkiSyncTask::init(
            library.clone(),
            mock_for_task,
            Duration::from_secs(3600),
            make_status_tx(),
        );

        // `save()` fires the wake, which must drive a pass syncing both cards.
        let mut translations2: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        translations2.insert("verb".into(), vec!["есть".into()]);
        let card2 = Card {
            version: 2,
            id: "flts_spa_rus_comer".into(),
            lemma: "comer".into(),
            translations: translations2,
            examples: vec![],
            anki_data: None,
        };
        library
            .card_store()
            .save(&card2, "spa", "rus")
            .await
            .unwrap();

        // 500 ms is far under the 1-hour interval, so a miss means no wake.
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        loop {
            let loaded = library
                .card_store()
                .load("spa", "rus", "comer")
                .await
                .unwrap()
                .expect("comer card present");
            if loaded.anki_data.is_some() {
                break;
            }
            if std::time::Instant::now() > deadline {
                panic!(
                    "card_change_notify wake did not trigger a sync_pass within 500 ms (card still unsynced)"
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        task.shutdown().await;
    }

    #[tokio::test]
    async fn anki_sync_task_runs_first_pass_within_interval() {
        let (_tmp, library) = seed_library_with_card("flts_anki_sync_first_tick").await;
        let mock_for_task: Arc<dyn AnkiConnect> = Arc::new(MockAnkiConnect::new());
        let task = AnkiSyncTask::init(
            library.clone(),
            mock_for_task,
            Duration::from_millis(10),
            make_status_tx(),
        );

        // Let the first tick fire and complete.
        tokio::time::sleep(Duration::from_millis(100)).await;
        task.shutdown().await;

        let card = library
            .card_store()
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert!(
            card.anki_data.is_some(),
            "first periodic tick must have synced the card"
        );
    }

    #[test]
    fn anki_sync_status_default_is_idle() {
        let status = AnkiSyncStatus::default();
        assert_eq!(status.state, AnkiSyncStatusState::Idle);
        assert!(status.last_finished_at_ms.is_none());
        assert!(status.last_error.is_none());
        assert!(status.last_report.is_none());
    }

    #[test]
    fn anki_sync_status_serializes_state_as_lowercase() {
        let cases = [
            (AnkiSyncStatusState::Idle, "\"idle\""),
            (AnkiSyncStatusState::Syncing, "\"syncing\""),
            (AnkiSyncStatusState::Ok, "\"ok\""),
            (AnkiSyncStatusState::Err, "\"err\""),
            (AnkiSyncStatusState::Unreachable, "\"unreachable\""),
        ];
        for (variant, expected) in cases {
            let s = serde_json::to_string(&variant).unwrap();
            assert_eq!(s, expected, "state variant must serialize as {expected}");
        }
    }

    #[test]
    fn anki_sync_status_serializes_fields_as_camel_case() {
        let status = AnkiSyncStatus {
            state: AnkiSyncStatusState::Ok,
            last_finished_at_ms: Some(1_700_000_000_000),
            last_error: None,
            last_report: Some(SyncReportDto {
                total_cards: 3,
                attempted: 2,
                succeeded: 2,
                failed: 0,
                persistent_failures: vec![],
            }),
        };
        let s = serde_json::to_string(&status).unwrap();
        assert!(s.contains("\"lastFinishedAtMs\""), "got {s}");
        assert!(s.contains("\"lastReport\""), "got {s}");
        assert!(s.contains("\"totalCards\":3"), "got {s}");
        assert!(s.contains("\"persistentFailures\""), "got {s}");
    }

    #[tokio::test]
    async fn anki_sync_task_emits_ok_status_after_first_tick() {
        let (_tmp, library) = seed_library_with_card("flts_anki_sync_status_ok").await;
        let mock: Arc<dyn AnkiConnect> = Arc::new(MockAnkiConnect::new());
        let (status_tx, status_rx) = tokio::sync::watch::channel(AnkiSyncStatus::default());
        let task = AnkiSyncTask::init(
            library,
            mock,
            Duration::from_millis(10),
            Arc::new(status_tx),
        );

        // Let the first tick fire and complete.
        tokio::time::sleep(Duration::from_millis(100)).await;
        task.shutdown().await;

        let status = status_rx.borrow().clone();
        assert_eq!(status.state, AnkiSyncStatusState::Ok);
        assert!(
            status.last_report.is_some(),
            "successful tick must populate last_report"
        );
        assert!(
            status.last_finished_at_ms.is_some(),
            "successful tick must populate last_finished_at_ms"
        );
        assert!(status.last_error.is_none());
    }

    #[tokio::test]
    async fn anki_sync_task_emits_unreachable_when_version_fails() {
        let (_tmp, library) = seed_library_with_card("flts_anki_sync_unreachable").await;
        let mock = Arc::new(MockAnkiConnect::new());
        // Every call fails, so each tick stops at the version() probe.
        mock.fail_next_n_calls(usize::MAX);
        let client: Arc<dyn AnkiConnect> = mock;
        let (status_tx, status_rx) = tokio::sync::watch::channel(AnkiSyncStatus::default());
        let task = AnkiSyncTask::init(
            library.clone(),
            client,
            Duration::from_millis(10),
            Arc::new(status_tx),
        );

        tokio::time::sleep(Duration::from_millis(100)).await;
        task.shutdown().await;

        let status = status_rx.borrow().clone();
        assert_eq!(status.state, AnkiSyncStatusState::Unreachable);
        assert!(
            status.last_error.is_some(),
            "Unreachable status must carry the version() error"
        );
        // sync_pass must not have run — no card should have synced.
        let card = library
            .card_store()
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("card present");
        assert!(
            card.anki_data.is_none(),
            "sync_pass must be skipped when version() fails"
        );
    }

    #[tokio::test]
    async fn anki_sync_task_recovers_to_ok_after_version_succeeds() {
        let (_tmp, library) = seed_library_with_card("flts_anki_sync_recover").await;
        let mock = Arc::new(MockAnkiConnect::new());
        // Only the first tick's version() fails; later ticks reach sync_pass.
        mock.fail_next_n_calls(1);
        let client: Arc<dyn AnkiConnect> = mock;
        let (status_tx, status_rx) = tokio::sync::watch::channel(AnkiSyncStatus::default());
        let task = AnkiSyncTask::init(
            library.clone(),
            client,
            Duration::from_millis(10),
            Arc::new(status_tx),
        );

        // Long enough for several ticks.
        tokio::time::sleep(Duration::from_millis(200)).await;
        task.shutdown().await;

        let status = status_rx.borrow().clone();
        assert_eq!(
            status.state,
            AnkiSyncStatusState::Ok,
            "status must recover to Ok once version() starts succeeding"
        );
    }

    #[tokio::test]
    async fn anki_sync_task_sync_now_runs_a_pass_and_returns_report() {
        let (_tmp, library) = seed_library_with_card("flts_anki_sync_now_ok").await;
        let mock: Arc<dyn AnkiConnect> = Arc::new(MockAnkiConnect::new());
        let (status_tx, status_rx) = tokio::sync::watch::channel(AnkiSyncStatus::default());
        // Long interval so the periodic loop doesn't race the explicit
        // sync_now call.
        let task = AnkiSyncTask::init(
            library.clone(),
            mock,
            Duration::from_secs(3600),
            Arc::new(status_tx),
        );

        let report = task.sync_now().await.expect("sync_now succeeds");
        assert!(
            report.succeeded > 0,
            "sync_now must report at least one succeeded card; got {report:?}"
        );

        let status = status_rx.borrow().clone();
        assert_eq!(status.state, AnkiSyncStatusState::Ok);
        assert!(status.last_report.is_some());

        task.shutdown().await;
    }

    #[tokio::test]
    async fn sync_now_reports_in_progress_instead_of_waiting() {
        let (_tmp, library) = seed_library_with_card("flts_anki_sync_busy").await;
        let mock: Arc<dyn AnkiConnect> = Arc::new(MockAnkiConnect::new());
        // Long interval so the periodic loop can't interfere mid-test.
        let task = AnkiSyncTask::init(library, mock, Duration::from_secs(3600), make_status_tx());

        // Model an in-flight pass by holding the state lock.
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

    #[tokio::test]
    async fn anki_sync_task_sync_now_returns_err_when_version_fails() {
        let (_tmp, library) = seed_library_with_card("flts_anki_sync_now_unreachable").await;
        let mock = Arc::new(MockAnkiConnect::new());
        mock.fail_next_n_calls(usize::MAX);
        let client: Arc<dyn AnkiConnect> = mock;
        let (status_tx, status_rx) = tokio::sync::watch::channel(AnkiSyncStatus::default());
        let task = AnkiSyncTask::init(
            library,
            client,
            Duration::from_secs(3600),
            Arc::new(status_tx),
        );

        let result = task.sync_now().await;
        assert!(result.is_err(), "version() failure must propagate");
        let status = status_rx.borrow().clone();
        assert_eq!(status.state, AnkiSyncStatusState::Unreachable);
        assert!(status.last_error.is_some());

        task.shutdown().await;
    }

    #[tokio::test]
    async fn sync_now_or_err_returns_err_when_task_is_none() {
        let slot: Mutex<Option<Arc<AnkiSyncTask>>> = Mutex::new(None);
        let err = sync_now_or_err(&slot)
            .await
            .expect_err("missing task must error");
        let msg = err.to_string();
        assert!(
            msg.contains("anki sync task"),
            "error must explain why; got {msg:?}"
        );
    }

    #[tokio::test]
    async fn sync_now_or_err_returns_report_when_task_present() {
        let (_tmp, library) = seed_library_with_card("flts_anki_sync_slot_present").await;
        let mock: Arc<dyn AnkiConnect> = Arc::new(MockAnkiConnect::new());
        let task = AnkiSyncTask::init(library, mock, Duration::from_secs(3600), make_status_tx());
        let slot: Mutex<Option<Arc<AnkiSyncTask>>> = Mutex::new(Some(task.clone()));

        let report = sync_now_or_err(&slot)
            .await
            .expect("present task must succeed");
        assert!(report.succeeded > 0);
        task.shutdown().await;
    }

    #[test]
    fn sync_report_dto_round_trips_from_library_report() {
        let report = library::anki::sync::SyncReport {
            total_cards: 5,
            attempted: 4,
            succeeded: 3,
            failed: 1,
            persistent_failures: vec!["flts_spa_rus_poder_verb".into()],
        };
        let dto: SyncReportDto = report.clone().into();
        assert_eq!(dto.total_cards, report.total_cards);
        assert_eq!(dto.attempted, report.attempted);
        assert_eq!(dto.succeeded, report.succeeded);
        assert_eq!(dto.failed, report.failed);
        assert_eq!(dto.persistent_failures, report.persistent_failures);
    }
}
