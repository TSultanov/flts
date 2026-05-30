//! Sync task lifecycle — the embedded Syncthing engine's app-side owner.
//!
//! Mirrors [`AnkiSyncTask`](crate::app::anki_sync::AnkiSyncTask): spawned from
//! `eval_config`, reports status through a stable `watch::Sender`, and is shut
//! down gracefully on app exit. It owns the single [`SyncEngine`] and a poller
//! that refreshes connection state.

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
    /// Engine up; `device_id` known.
    Online,
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

        // List this device in the shared roster so peers add it back (mutual
        // pairing → mesh).
        if let Err(err) = engine.ensure_self_in_roster(&device_name) {
            warn!("Could not record this device in the roster: {err}");
        }

        // Seed status and spawn the reconcile+status poller.
        push_status(engine.client().as_ref(), &status_tx, &my_id).await;
        let handle = {
            let engine = engine.clone();
            let status_tx = status_tx.clone();
            let my_id = my_id.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(DEFAULT_POLL_INTERVAL);
                loop {
                    ticker.tick().await;
                    // Bring the engine in line with the roster (devices paired
                    // on other nodes), then refresh status.
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
        if let Err(err) = self.engine.stop() {
            warn!("Sync engine stop failed: {err}");
        }
        self.status_tx.send_replace(SyncStatus::disabled());
    }
}

/// Pure predicate for the `FLTS_DISABLE_SYNC` env gate (mirrors
/// `anki_sync_disabled`): any non-empty value disables sync regardless of the
/// `syncEnabled` config flag.
pub fn sync_disabled(env_value: Option<&std::ffi::OsStr>) -> bool {
    env_value.is_some_and(|v| !v.is_empty())
}

/// Refreshes device/connection counts into the status sender. On a REST error
/// keeps the device id but flips to `Error` so the UI can surface it. Takes the
/// control client (not the engine) so it is unit-testable against a mock.
async fn push_status(
    client: &dyn library::sync::control::SyncthingApi,
    status_tx: &watch::Sender<SyncStatus>,
    my_id: &str,
) {
    let devices = client.list_devices().await;
    let connections = client.connections().await;

    match (devices, connections) {
        (Ok(devices), Ok(connections)) => {
            // Peers = devices excluding self.
            let peers: Vec<_> = devices
                .into_iter()
                .filter(|d| d.device_id != my_id)
                .collect();
            let connected = peers
                .iter()
                .filter(|d| connections.get(&d.device_id).copied().unwrap_or(false))
                .count();
            status_tx.send_replace(SyncStatus {
                state: SyncState::Online,
                device_id: Some(my_id.to_string()),
                device_count: peers.len(),
                connected_count: connected,
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

    #[test]
    fn sync_disabled_predicate_matches_anki_semantics() {
        use std::ffi::OsStr;
        assert!(!sync_disabled(None));
        assert!(!sync_disabled(Some(OsStr::new(""))));
        assert!(sync_disabled(Some(OsStr::new("1"))));
    }
}
