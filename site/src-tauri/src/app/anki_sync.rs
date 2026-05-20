//! Stage 7: periodic Anki sync orchestration. Mirrors the
//! `TranslationQueue` lifecycle — `init` spawns a tokio task that
//! ticks `library::anki::sync::sync_pass` on a fixed interval;
//! `shutdown` aborts + joins.
//!
//! Spawned from `AppState::eval_config` gated on `FLTS_ENABLE_ANKI_SYNC`.
//! See `.specs/ANKI_PLAN.md § Stage 7`.

use std::sync::Arc;
use std::time::Duration;

use library::anki::connect::AnkiConnect;
use library::anki::sync::{AnkiSyncState, SyncReport, sync_pass};
use library::library::Library;
use log::{info, warn};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

pub struct AnkiSyncTask {
    state: Arc<Mutex<AnkiSyncState>>,
    client: Arc<dyn AnkiConnect>,
    library: Arc<Library>,
    task_handle: Mutex<Option<JoinHandle<()>>>,
}

impl AnkiSyncTask {
    pub fn init(
        library: Arc<Library>,
        client: Arc<dyn AnkiConnect>,
        interval: Duration,
    ) -> Arc<Self> {
        let state = Arc::new(Mutex::new(AnkiSyncState::new()));

        let task = {
            let state = state.clone();
            let client = client.clone();
            let library = library.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                loop {
                    ticker.tick().await;
                    let mut guard = state.lock().await;
                    let now = tokio::time::Instant::now();
                    match sync_pass(client.as_ref(), library.as_ref(), &mut guard, now).await {
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
                        }
                        Err(err) => warn!("anki sync_pass failed: {err}"),
                    }
                }
            })
        };

        Arc::new(Self {
            state,
            client,
            library,
            task_handle: Mutex::new(Some(task)),
        })
    }

    pub async fn shutdown(&self) {
        if let Some(handle) = self.task_handle.lock().await.take() {
            handle.abort();
            let _ = handle.await;
        }
    }

    /// Run one synchronous sync_pass. Used by the app-quit final-pass hook.
    pub async fn run_one_pass(&self) -> anyhow::Result<SyncReport> {
        let mut guard = self.state.lock().await;
        let now = tokio::time::Instant::now();
        sync_pass(
            self.client.as_ref(),
            self.library.as_ref(),
            &mut guard,
            now,
        )
        .await
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
        let card = Card {
            version: 1,
            id: "flts_spa_rus_poder_verb".into(),
            lemma: "poder".into(),
            part_of_speech: "verb".into(),
            translations: vec!["мочь".into()],
            examples: vec![],
            anki_data: None,
        };
        library.card_store().save(&card, "spa", "rus").await.unwrap();
        (tmp, library)
    }

    #[tokio::test]
    async fn anki_sync_task_init_and_shutdown_does_not_panic() {
        let (_tmp, library) = seed_library_with_card("flts_anki_sync_smoke").await;
        let mock: Arc<dyn AnkiConnect> = Arc::new(MockAnkiConnect::new());
        let task = AnkiSyncTask::init(library, mock, Duration::from_millis(50));
        task.shutdown().await;
    }

    #[tokio::test]
    async fn anki_sync_task_runs_first_pass_within_interval() {
        let (_tmp, library) = seed_library_with_card("flts_anki_sync_first_tick").await;
        let mock_for_task: Arc<dyn AnkiConnect> = Arc::new(MockAnkiConnect::new());
        let task =
            AnkiSyncTask::init(library.clone(), mock_for_task, Duration::from_millis(10));

        // Let the first tick fire and complete.
        tokio::time::sleep(Duration::from_millis(100)).await;
        task.shutdown().await;

        let card = library
            .card_store()
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .expect("card present");
        assert!(
            card.anki_data.is_some(),
            "first periodic tick must have synced the card"
        );
    }
}
