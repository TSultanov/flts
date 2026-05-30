//! Embedded Syncthing engine lifecycle.
//!
//! Brings the Go engine up via `syncthing-sys`, waits for its REST API, and
//! configures the single FLTS folder + discovery options. One engine per
//! process (the Go side holds global state), so this is created once by the
//! sync daemon and torn down on shutdown.

use std::{
    collections::BTreeSet,
    net::TcpListener,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};

use super::control::{DeviceInfo, FolderSpec, HttpSyncthing, OptionsPatch, SyncthingApi};
use super::reconcile::reconcile;
use super::roster::RosterStore;

/// Fixed app folder ID for the synced library. Stable across devices.
pub const LIBRARY_FOLDER_ID: &str = "flts-library";

/// A paired peer as shown in the device-management UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    pub device_id: String,
    pub name: String,
    pub connected: bool,
}

/// How long to wait for the engine's REST API to come up before giving up.
const REST_READY_TIMEOUT: Duration = Duration::from_secs(30);
const REST_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Inputs for bringing up the engine.
pub struct EngineConfig {
    /// Syncthing home: certs (device identity), `config.xml`, index DB. Lives
    /// outside the synced folder and is never itself synced.
    pub home: PathBuf,
    /// The folder to sync — the app-managed library root.
    pub library_root: PathBuf,
    /// Hermetic mode (tests/Docker): no public/LAN discovery, relays, or NAT.
    pub hermetic: bool,
}

/// A running engine plus a control client bound to it.
pub struct SyncEngine {
    client: Arc<dyn SyncthingApi>,
    my_id: String,
    /// The synced library path, kept so we can re-share the folder when the
    /// peer set changes.
    library_root: String,
    /// The shared device roster (`<library_root>/.flts/devices.json`); the
    /// source of truth for the mesh.
    roster: RosterStore,
}

impl SyncEngine {
    /// Starts the engine, waits for REST, and applies the FLTS configuration
    /// (discovery options + the `flts-library` folder pointed at the library
    /// root, initially shared only with this device).
    pub async fn start(cfg: EngineConfig) -> Result<Self> {
        std::fs::create_dir_all(&cfg.home)
            .map_err(|e| anyhow!("creating syncthing home {:?} failed: {e}", cfg.home))?;

        let api_key = generate_api_key();
        let port = pick_free_port()?;
        let addr = format!("127.0.0.1:{port}");

        syncthing_sys::start(&cfg.home, &addr, &api_key, cfg.hermetic)
            .map_err(|e| anyhow!("starting syncthing engine failed: {e}"))?;

        let client: Arc<dyn SyncthingApi> =
            Arc::new(HttpSyncthing::new(format!("http://{addr}"), api_key));
        let my_id = wait_until_up(client.as_ref()).await?;
        let roster = RosterStore::new(&cfg.library_root);
        let library_root = cfg.library_root.to_string_lossy().into_owned();

        // Hermetic mode keeps discovery off and binds loopback only (matching
        // the startup flags); the default reaches peers anywhere (global
        // discovery + relays) on dynamic ports that won't collide with a user's
        // own Syncthing.
        let options = if cfg.hermetic {
            OptionsPatch {
                global_discovery: false,
                local_discovery: false,
                relays: false,
                nat: false,
                listen_addresses: vec!["tcp://127.0.0.1:0".into()],
            }
        } else {
            OptionsPatch::default()
        };
        client.set_options(options).await?;

        client
            .ensure_folder(FolderSpec {
                id: LIBRARY_FOLDER_ID.to_string(),
                label: "FLTS Library".to_string(),
                path: library_root.clone(),
                device_ids: vec![my_id.clone()],
            })
            .await?;

        Ok(Self {
            client,
            my_id,
            library_root,
            roster,
        })
    }

    /// Adds (or renames) a peer and shares the library folder with the full
    /// peer set. The peer's `autoAcceptFolders` is set, so once they add us back
    /// the folder is accepted on their side automatically.
    pub async fn add_peer(&self, device_id: &str, name: &str) -> anyhow::Result<()> {
        self.client.add_device(device_id, name).await?;
        self.reshare_library().await
    }

    /// Removes a peer and re-shares the folder without it.
    pub async fn remove_peer(&self, device_id: &str) -> anyhow::Result<()> {
        self.client.remove_device(device_id).await?;
        self.reshare_library().await
    }

    /// Peers (everything but this device) with live connection state.
    pub async fn list_peers(&self) -> anyhow::Result<Vec<PeerInfo>> {
        let devices = self.client.list_devices().await?;
        let connections = self.client.connections().await?;
        Ok(devices
            .into_iter()
            .filter(|d| d.device_id != self.my_id)
            .map(|DeviceInfo { device_id, name }| PeerInfo {
                connected: connections.get(&device_id).copied().unwrap_or(false),
                device_id,
                name,
            })
            .collect())
    }

    /// Shares the library folder with this device plus every configured peer.
    async fn reshare_library(&self) -> anyhow::Result<()> {
        let mut device_ids: Vec<String> = self
            .client
            .list_devices()
            .await?
            .into_iter()
            .map(|d| d.device_id)
            .collect();
        if !device_ids.iter().any(|id| id == &self.my_id) {
            device_ids.push(self.my_id.clone());
        }
        self.client
            .ensure_folder(FolderSpec {
                id: LIBRARY_FOLDER_ID.to_string(),
                label: "FLTS Library".to_string(),
                path: self.library_root.clone(),
                device_ids,
            })
            .await
    }

    /// Pair with a peer: record it in the shared roster (so it propagates to
    /// every node) and add it to this engine immediately. The roster is the
    /// mesh's source of truth; `add_peer` just makes the local effect instant.
    pub async fn pair_device(&self, device_id: &str, name: &str) -> anyhow::Result<()> {
        self.roster.add_device(device_id, name)?;
        self.add_peer(device_id, name).await
    }

    /// Unpair a peer: tombstone it in the roster (propagates the removal) and
    /// drop it locally.
    pub async fn unpair_device(&self, device_id: &str) -> anyhow::Result<()> {
        self.roster.remove_device(device_id)?;
        self.remove_peer(device_id).await
    }

    /// Ensure this device is listed in the roster under `name`, so peers learn
    /// about it and add it back (completing the mutual pairing for the mesh).
    pub fn ensure_self_in_roster(&self, name: &str) -> anyhow::Result<()> {
        self.roster.ensure_self(&self.my_id, name)?;
        Ok(())
    }

    /// One reconcile pass: load the shared roster (merging any conflict
    /// siblings) and bring this engine's device set in line with it — adding
    /// devices others paired and removing tombstoned ones. This is what turns a
    /// single pairing into a full mesh.
    pub async fn reconcile_once(&self) -> anyhow::Result<()> {
        let roster = self.roster.load()?;
        let engine_ids: BTreeSet<String> = self
            .client
            .list_devices()
            .await?
            .into_iter()
            .map(|d| d.device_id)
            .collect();

        let plan = reconcile(&roster, &engine_ids, &self.my_id);
        if plan.is_empty() {
            return Ok(());
        }
        for (id, name) in &plan.to_add {
            if let Err(err) = self.add_peer(id, name).await {
                anyhow::bail!("reconcile: adding {id} failed: {err}");
            }
        }
        for id in &plan.to_remove {
            if let Err(err) = self.remove_peer(id).await {
                anyhow::bail!("reconcile: removing {id} failed: {err}");
            }
        }
        Ok(())
    }

    /// Test-only constructor that injects a control client (e.g. a mock),
    /// bypassing the real engine so peer/share logic is unit-testable without a
    /// running Syncthing or valid device IDs.
    #[cfg(test)]
    fn for_test(client: Arc<dyn SyncthingApi>, my_id: String, library_root: String) -> Self {
        let roster = RosterStore::new(std::path::Path::new(&library_root));
        Self {
            client,
            my_id,
            library_root,
            roster,
        }
    }

    /// The control client, for the daemon and Tauri commands.
    pub fn client(&self) -> Arc<dyn SyncthingApi> {
        self.client.clone()
    }

    /// This device's Syncthing ID (the QR/pairing payload).
    pub fn my_id(&self) -> &str {
        &self.my_id
    }

    /// Stops the engine cleanly. Idempotent on the Go side.
    pub fn stop(&self) -> Result<()> {
        syncthing_sys::stop().map_err(|e| anyhow!("stopping syncthing engine failed: {e}"))
    }
}

/// Polls `my_id()` until the REST API answers or the timeout elapses. `start`
/// already returns once the API is listening; this guards against the brief
/// window before the first successful request.
async fn wait_until_up(client: &dyn SyncthingApi) -> Result<String> {
    let deadline = Instant::now() + REST_READY_TIMEOUT;
    let mut last_err = None;
    loop {
        match client.my_id().await {
            Ok(id) if !id.is_empty() => return Ok(id),
            Ok(_) => {}
            Err(e) => last_err = Some(e),
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "syncthing REST API not ready within {:?}{}",
                REST_READY_TIMEOUT,
                last_err
                    .map(|e| format!(": {e}"))
                    .unwrap_or_default()
            ));
        }
        tokio::time::sleep(REST_POLL_INTERVAL).await;
    }
}

/// 32-hex-char random API key for the localhost REST binding.
fn generate_api_key() -> String {
    use rand::RngExt;
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reserve an ephemeral loopback port, then release it for the engine to bind.
/// A small TOCTOU window exists; acceptable for localhost.
fn pick_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| anyhow!("could not reserve a local port for the GUI: {e}"))?;
    Ok(listener.local_addr()?.port())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::control::MockSyncthing;

    #[tokio::test]
    async fn add_remove_peer_reshares_library_folder() {
        let mock = Arc::new(MockSyncthing::new("SELF"));
        let engine =
            SyncEngine::for_test(mock.clone(), "SELF".into(), "/tmp/flts-lib".into());

        engine.add_peer("PEER1", "Laptop").await.unwrap();

        // Folder is shared with this device plus the new peer.
        let folders = mock.folders();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].path, "/tmp/flts-lib");
        assert!(folders[0].device_ids.contains(&"SELF".to_string()));
        assert!(folders[0].device_ids.contains(&"PEER1".to_string()));

        // list_peers excludes self.
        let peers = engine.list_peers().await.unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].device_id, "PEER1");
        assert!(!peers[0].connected);

        // Removal drops the peer from both the device list and the folder.
        engine.remove_peer("PEER1").await.unwrap();
        assert!(engine.list_peers().await.unwrap().is_empty());
        let folders = mock.folders();
        assert!(!folders.last().unwrap().device_ids.contains(&"PEER1".to_string()));
    }

    fn scratch_root(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("flts-mesh-{tag}-{nanos}"))
    }

    #[tokio::test]
    async fn pair_writes_roster_and_lists_self() {
        let root = scratch_root("pair");
        let mock = Arc::new(MockSyncthing::new("SELF"));
        let engine = SyncEngine::for_test(mock, "SELF".into(), root.to_string_lossy().into());

        engine.ensure_self_in_roster("My Mac").unwrap();
        engine.pair_device("PEER1", "Laptop").await.unwrap();

        let roster = RosterStore::new(&root).load().unwrap();
        assert_eq!(roster.devices.get("SELF").unwrap().name, "My Mac");
        assert!(roster.devices.contains_key("PEER1"), "peer recorded in roster");
        let peers = engine.list_peers().await.unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].device_id, "PEER1");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn reconcile_adds_devices_paired_on_another_node() {
        let root = scratch_root("recadd");
        let mock = Arc::new(MockSyncthing::new("SELF"));
        let engine = SyncEngine::for_test(mock.clone(), "SELF".into(), root.to_string_lossy().into());

        // A peer paired this device on ANOTHER node: it lands in the synced
        // roster, but our engine doesn't know it yet.
        RosterStore::new(&root).add_device("PEERX", "Other").unwrap();
        assert!(engine.list_peers().await.unwrap().is_empty());

        engine.reconcile_once().await.unwrap();

        let peers = engine.list_peers().await.unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].device_id, "PEERX");
        // And the library folder is now shared with the new peer.
        assert!(mock
            .folders()
            .last()
            .unwrap()
            .device_ids
            .contains(&"PEERX".to_string()));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn reconcile_removes_tombstoned_devices() {
        let root = scratch_root("recrm");
        let mock = Arc::new(MockSyncthing::new("SELF"));
        let engine = SyncEngine::for_test(mock, "SELF".into(), root.to_string_lossy().into());

        engine.add_peer("PEER1", "x").await.unwrap();
        assert_eq!(engine.list_peers().await.unwrap().len(), 1);

        // Removed on another node → tombstoned in the synced roster.
        RosterStore::new(&root).remove_device("PEER1").unwrap();
        engine.reconcile_once().await.unwrap();

        assert!(engine.list_peers().await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Full Phase 2 engine path against the real Go engine (hermetic): start,
    /// apply discovery options + ensure the library folder, expose `my_id`, and
    /// stop cleanly. A successful `start` proves the REST config calls
    /// (`defaults/folder`, `PUT folders/{id}`, `options`) all worked.
    #[tokio::test]
    async fn engine_starts_configures_and_stops() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("flts-engine-test-{nanos}"));
        let home = base.join("st-home");
        let library = base.join("library");
        std::fs::create_dir_all(&library).unwrap();

        let engine = SyncEngine::start(EngineConfig {
            home,
            library_root: library.clone(),
            hermetic: true,
        })
        .await
        .expect("engine starts and configures");

        let id = engine.my_id().to_string();
        assert!(id.len() >= 50 && id.contains('-'), "looks like a device ID: {id:?}");

        // The folder we ensured is readable back by ID.
        let devices_self = engine.client().my_id().await.unwrap();
        assert_eq!(devices_self, id, "client talks to the same engine");

        engine.stop().expect("engine stops cleanly");
        let _ = std::fs::remove_dir_all(&base);
    }
}
