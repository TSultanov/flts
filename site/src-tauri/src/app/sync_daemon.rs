//! Sync task lifecycle — the embedded Syncthing engine's app-side owner.
//!
//! Owns the single [`SyncEngine`] plus its status poller. Spawned from
//! `eval_config` and shut down on app exit, like
//! [`AnkiSyncTask`](crate::app::anki_sync::AnkiSyncTask).

use std::{path::PathBuf, sync::Arc, time::Duration};

use library::sync::engine::{EngineConfig, SyncEngine};
use log::{info, warn};
use serde::Serialize;
use tokio::{sync::Mutex, sync::watch, task::JoinHandle};

/// Coarse sync state for the UI. Lowercase to match the frontend.
#[derive(Debug, Clone, Copy, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SyncState {
    /// Sync is off (not enabled, or disabled by env).
    #[default]
    Disabled,
    /// Engine is starting / not yet reachable.
    Starting,
    /// Engine up and the library folder is fully in sync.
    Online,
    /// Actively transferring data with a peer (`completion` < 100).
    Syncing,
    /// Engine failed to start or a poll errored; see `last_error`.
    Error,
}

/// Status snapshot pushed to the frontend.
#[derive(Debug, Clone, Serialize, Default)]
pub struct SyncStatus {
    pub state: SyncState,
    /// This device's Syncthing ID (the pairing payload), when known.
    #[serde(rename = "deviceId")]
    pub device_id: Option<String>,
    /// Number of paired peer devices (excluding self).
    #[serde(rename = "deviceCount")]
    pub device_count: usize,
    /// How many of those peers are currently connected.
    #[serde(rename = "connectedCount")]
    pub connected_count: usize,
    /// Folder sync progress (0–100) while `state` is `Syncing`.
    #[serde(rename = "completion")]
    pub completion: Option<f64>,
    #[serde(rename = "lastError")]
    pub last_error: Option<String>,
}

impl SyncStatus {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn error(msg: String) -> Self {
        Self {
            state: SyncState::Error,
            last_error: Some(msg),
            ..Default::default()
        }
    }
}

/// How often the poller refreshes device/connection counts.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(10);

/// Wake-probe budget: an unreachable engine is the expected case on wake and
/// the frontend awaits the invoke.
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

pub struct SyncTask {
    engine: Arc<SyncEngine>,
    status_tx: Arc<watch::Sender<SyncStatus>>,
    task_handle: Mutex<Option<JoinHandle<()>>>,
}

impl SyncTask {
    /// Starts the engine (pointing the `flts-library` folder at `library_root`,
    /// home under `home`) and spawns the status poller. Returns an error if the
    /// engine fails to come up; the caller reflects that as `SyncState::Error`.
    pub async fn init(
        home: PathBuf,
        library_root: PathBuf,
        device_name: String,
        hermetic: bool,
        status_tx: Arc<watch::Sender<SyncStatus>>,
    ) -> anyhow::Result<Arc<Self>> {
        status_tx.send_replace(SyncStatus {
            state: SyncState::Starting,
            ..Default::default()
        });

        // Hermetic (tests/E2E): stay fully local. Otherwise reach peers anywhere
        // on dynamic ports.
        let options = if hermetic {
            library::sync::control::OptionsPatch::loopback()
        } else {
            library::sync::control::OptionsPatch::default()
        };
        let engine = Arc::new(
            SyncEngine::start(EngineConfig {
                home,
                library_root,
                options,
                loopback_only: hermetic,
            })
            .await?,
        );
        let my_id = engine.my_id().to_string();
        info!("Sync engine online; device id = {my_id}");

        // Names the roster entry as well, which is how peers add this device back.
        if let Err(err) = engine.set_device_name(&device_name).await {
            warn!("Could not set this device's name: {err}");
        }

        push_status(engine.client().as_ref(), &status_tx, &my_id).await;
        let handle = {
            let engine = engine.clone();
            let status_tx = status_tx.clone();
            let my_id = my_id.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(DEFAULT_POLL_INTERVAL);
                loop {
                    ticker.tick().await;
                    // Picks up devices paired on other nodes.
                    if let Err(err) = engine.reconcile_once().await {
                        warn!("Sync reconcile failed: {err}");
                    }
                    push_status(engine.client().as_ref(), &status_tx, &my_id).await;
                }
            })
        };

        Ok(Arc::new(Self {
            engine,
            status_tx,
            task_handle: Mutex::new(Some(handle)),
        }))
    }

    /// The running engine (for Tauri commands: this-device id, add/remove peer).
    pub fn engine(&self) -> Arc<SyncEngine> {
        self.engine.clone()
    }

    /// Aborts the poller, stops the engine, and resets status to disabled.
    pub async fn shutdown(&self) {
        if let Some(handle) = self.task_handle.lock().await.take() {
            handle.abort();
            let _ = handle.await;
        }
        if let Err(err) = self.engine.stop().await {
            warn!("Sync engine stop failed: {err}");
        }
        self.status_tx.send_replace(SyncStatus::disabled());
    }
}

/// `FLTS_DISABLE_SYNC` gate: any non-empty value overrides `syncEnabled`.
pub fn sync_disabled(env_value: Option<&std::ffi::OsStr>) -> bool {
    env_value.is_some_and(|v| !v.is_empty())
}

/// Refreshes device/connection counts into the status sender; a REST error keeps
/// the device id but flips to `Error`. Takes the control client, not the engine,
/// so a mock can drive it.
async fn push_status(
    client: &dyn library::sync::control::SyncthingApi,
    status_tx: &watch::Sender<SyncStatus>,
    my_id: &str,
) {
    let devices = client.list_devices().await;
    let connections = client.connections().await;
    let completion = client
        .folder_completion(library::sync::engine::LIBRARY_FOLDER_ID)
        .await
        .ok();

    match (devices, connections) {
        (Ok(devices), Ok(connections)) => {
            let peers: Vec<_> = devices
                .into_iter()
                .filter(|d| d.device_id != my_id)
                .collect();
            let connected = peers
                .iter()
                .filter(|d| connections.get(&d.device_id).copied().unwrap_or(false))
                .count();
            // Needs a connected peer too, else the percentage sticks and misleads.
            let syncing = connected > 0 && completion.is_some_and(|c| c < 99.99);
            status_tx.send_replace(SyncStatus {
                state: if syncing {
                    SyncState::Syncing
                } else {
                    SyncState::Online
                },
                device_id: Some(my_id.to_string()),
                device_count: peers.len(),
                connected_count: connected,
                completion: if syncing { completion } else { None },
                last_error: None,
            });
        }
        (Err(err), _) | (_, Err(err)) => {
            warn!("Sync status poll failed: {err}");
            status_tx.send_replace(SyncStatus {
                device_id: Some(my_id.to_string()),
                ..SyncStatus::error(err.to_string())
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::sync::control::{MockSyncthing, SyncthingApi};

    #[tokio::test]
    async fn push_status_counts_peers_excluding_self_and_connected() {
        let api = MockSyncthing::new("SELF");
        // Self may appear in the device list; it must not count as a peer.
        api.add_device("SELF", "me").await.unwrap();
        api.add_device("PEER1", "a").await.unwrap();
        api.add_device("PEER2", "b").await.unwrap();
        api.set_connected("PEER1", true);

        let (tx, rx) = watch::channel(SyncStatus::default());
        push_status(&api, &tx, "SELF").await;

        let status = rx.borrow();
        assert_eq!(status.state, SyncState::Online);
        assert_eq!(status.device_id.as_deref(), Some("SELF"));
        assert_eq!(status.device_count, 2, "peers exclude self");
        assert_eq!(status.connected_count, 1);
    }

    #[tokio::test]
    async fn push_status_reports_syncing_with_completion() {
        let api = MockSyncthing::new("SELF");
        api.add_device("SELF", "me").await.unwrap();
        api.add_device("PEER", "p").await.unwrap();
        api.set_connected("PEER", true);
        api.set_completion(42.0);

        let (tx, rx) = watch::channel(SyncStatus::default());
        push_status(&api, &tx, "SELF").await;
        assert_eq!(rx.borrow().state, SyncState::Syncing);
        assert_eq!(rx.borrow().completion, Some(42.0));

        // Caught up → Online, no percentage.
        api.set_completion(100.0);
        push_status(&api, &tx, "SELF").await;
        assert_eq!(rx.borrow().state, SyncState::Online);
        assert_eq!(rx.borrow().completion, None);
    }

    #[tokio::test]
    async fn not_syncing_without_a_connected_peer() {
        let api = MockSyncthing::new("SELF");
        api.add_device("PEER", "p").await.unwrap();
        api.set_completion(10.0); // behind, but nobody connected to catch up from
        let (tx, rx) = watch::channel(SyncStatus::default());
        push_status(&api, &tx, "SELF").await;
        assert_eq!(rx.borrow().state, SyncState::Online);
    }

    #[test]
    fn sync_disabled_predicate_matches_anki_semantics() {
        use std::ffi::OsStr;
        assert!(!sync_disabled(None));
        assert!(!sync_disabled(Some(OsStr::new(""))));
        assert!(sync_disabled(Some(OsStr::new("1"))));
    }

    #[tokio::test]
    async fn probe_healthy_true_for_responsive_engine() {
        let api = MockSyncthing::new("SELF");
        assert!(probe_healthy(&api, Duration::from_secs(1)).await);
    }

    #[tokio::test]
    async fn probe_healthy_returns_false_quickly_when_my_id_hangs() {
        /// my_id never resolves; no other method is reachable from probe_healthy.
        struct HangingApi;
        #[async_trait::async_trait]
        impl library::sync::control::SyncthingApi for HangingApi {
            async fn my_id(&self) -> anyhow::Result<String> {
                std::future::pending().await
            }
            async fn list_devices(
                &self,
            ) -> anyhow::Result<Vec<library::sync::control::DeviceInfo>> {
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
            async fn ensure_folder(
                &self,
                _: library::sync::control::FolderSpec,
            ) -> anyhow::Result<()> {
                unreachable!()
            }
            async fn set_options(
                &self,
                _: library::sync::control::OptionsPatch,
            ) -> anyhow::Result<()> {
                unreachable!()
            }
            async fn pending_devices(
                &self,
            ) -> anyhow::Result<Vec<library::sync::control::PendingDevice>> {
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
}
