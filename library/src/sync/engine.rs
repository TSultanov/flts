//! Embedded Syncthing engine lifecycle.
//!
//! Brings the Go engine up via `syncthing-sys`, waits for its REST API, and
//! configures the FLTS folder + discovery options. The Go side holds global
//! state, so there must be exactly one engine per process.

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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PeerInfo {
    #[serde(rename = "deviceId")]
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
    /// Discovery/relays/listen options applied over REST once the engine is up.
    pub options: OptionsPatch,
    /// Bind loopback only and skip boot-time discovery; `false` binds a
    /// routable address.
    pub loopback_only: bool,
}

/// A running engine plus a control client bound to it.
pub struct SyncEngine {
    /// Loopback origin of the REST/GUI endpoint; debug builds serve
    /// Syncthing's web dashboard here.
    gui_url: String,
    client: Arc<dyn SyncthingApi>,
    my_id: String,
    /// Kept so the folder can be re-shared when the peer set changes.
    library_root: String,
    /// `<library_root>/.flts/devices.json`: the mesh's source of truth.
    roster: RosterStore,
}

impl SyncEngine {
    /// Starts the engine, waits for REST, then applies the discovery options
    /// and shares `flts-library` with this device and every configured peer.
    pub async fn start(cfg: EngineConfig) -> Result<Self> {
        std::fs::create_dir_all(&cfg.home)
            .map_err(|e| anyhow!("creating syncthing home {:?} failed: {e}", cfg.home))?;

        let api_key = generate_api_key();
        let port = pick_free_port()?;
        let addr = format!("127.0.0.1:{port}");

        // Blocking pool: the boot must not pin a tokio worker, and callers'
        // timeouts need an await point. Dropping the await detaches rather than
        // cancels the boot, so callers must run it to completion.
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

        let gui_url = format!("http://{addr}");
        let client: Arc<dyn SyncthingApi> =
            Arc::new(HttpSyncthing::new(gui_url.clone(), api_key));
        let my_id = wait_until_up(client.as_ref()).await?;
        let roster = RosterStore::new(&cfg.library_root, &my_id);
        let library_root = cfg.library_root.to_string_lossy().into_owned();

        // The Go startup flag only governs the pre-REST boot window.
        client.set_options(cfg.options).await?;

        let engine = Self {
            gui_url,
            client,
            my_id,
            library_root,
            roster,
        };

        // `ensure_folder` PUTs the whole device list, so sharing with self
        // alone would un-share persisted peers that reconcile won't re-add.
        engine.reshare_library().await?;

        Ok(engine)
    }

    /// Loopback origin of the engine's REST/GUI endpoint.
    pub fn gui_url(&self) -> &str {
        &self.gui_url
    }

    /// Adds or renames a peer and re-shares the folder. The peer's
    /// `autoAcceptFolders` is set so it accepts the folder once it adds us.
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

    /// Record the peer in the roster (which propagates it mesh-wide) and add it
    /// locally so the effect is immediate.
    pub async fn pair_device(&self, device_id: &str, name: &str) -> anyhow::Result<()> {
        self.roster.add_device(device_id, name)?;
        self.add_peer(device_id, name).await?;
        #[cfg(feature = "tla_trace")]
        self.trace_emit("PairOn", Some(device_id), None).await;
        Ok(())
    }

    /// Tombstone the peer in the roster and drop it locally.
    pub async fn unpair_device(&self, device_id: &str) -> anyhow::Result<()> {
        self.roster.remove_device(device_id)?;
        self.remove_peer(device_id).await?;
        #[cfg(feature = "tla_trace")]
        self.trace_emit("UnpairOn", Some(device_id), None).await;
        Ok(())
    }

    /// Names this device in the roster (for reconciling peers) and in
    /// Syncthing, so the announced name isn't the hostname.
    pub async fn set_device_name(&self, name: &str) -> anyhow::Result<()> {
        self.roster.ensure_self(&self.my_id, name)?;
        self.client.rename_device(&self.my_id, name).await?;
        #[cfg(feature = "tla_trace")]
        self.trace_emit("EnsureSelf", None, None).await;
        Ok(())
    }

    /// Bring this engine's device set in line with the roster. This is what
    /// turns a single pairing into a full mesh.
    pub async fn reconcile_once(&self) -> anyhow::Result<()> {
        // Peek before `load` clears the siblings, so RosterSync can be emitted
        // per sibling that actually changed the roster.
        #[cfg(feature = "tla_trace")]
        let sibling_srcs = self.roster.pending_sibling_sources();
        #[cfg(feature = "tla_trace")]
        let pre_roster = self.roster.snapshot_for_trace();

        let roster = self.roster.load()?;

        #[cfg(feature = "tla_trace")]
        if roster != pre_roster {
            for src in &sibling_srcs {
                self.trace_emit("RosterSync", None, Some(src)).await;
            }
        }

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

        #[cfg(feature = "tla_trace")]
        self.trace_emit("ReconcileNode", None, None).await;
        Ok(())
    }

    /// Emits one roster-mesh event (`spec/roster/`) carrying this node's
    /// post-state. No-op unless a trace sink is installed.
    #[cfg(feature = "tla_trace")]
    async fn trace_emit(&self, name: &str, target: Option<&str>, src: Option<&str>) {
        let roster = self.roster.load().unwrap_or_default();
        let mut rj = serde_json::Map::new();
        let ids: std::collections::BTreeSet<&String> =
            roster.adds.keys().chain(roster.removes.keys()).collect();
        for id in ids {
            let add = roster.adds.get(id).map(|a| &a.vc).cloned().unwrap_or_default();
            let rem = roster.removes.get(id).map(|r| &r.vc).cloned().unwrap_or_default();
            rj.insert(id.clone(), serde_json::json!({ "add": add, "rem": rem }));
        }
        let engine: Vec<String> = self
            .client
            .list_devices()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|d| d.device_id)
            .filter(|id| *id != self.my_id)
            .collect();
        let _ = crate::tla_trace::emit_roster_event(
            name,
            &self.my_id,
            target,
            src,
            serde_json::Value::Object(rj),
            &engine,
        );
    }

    /// Injects a control client so peer/share logic is testable without a
    /// running Syncthing or valid device IDs.
    #[cfg(test)]
    pub(crate) fn for_test(client: Arc<dyn SyncthingApi>, my_id: String, library_root: String) -> Self {
        let roster = RosterStore::new(std::path::Path::new(&library_root), &my_id);
        Self {
            gui_url: "http://127.0.0.1:0".to_string(),
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

    /// Idempotent on the Go side. Runs on the blocking pool so exit-path
    /// timeouts can preempt the teardown.
    pub async fn stop(&self) -> Result<()> {
        tokio::task::spawn_blocking(syncthing_sys::stop)
            .await
            .map_err(|e| anyhow!("syncthing stop task panicked: {e}"))?
            .map_err(|e| anyhow!("stopping syncthing engine failed: {e}"))
    }
}

/// Polls `my_id()` until the REST API answers: it is listening slightly before
/// it serves the first request.
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

/// Reserve an ephemeral loopback port, then release it for the engine. The
/// TOCTOU window is acceptable on localhost.
fn pick_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| anyhow!("could not reserve a local port for the GUI: {e}"))?;
    Ok(listener.local_addr()?.port())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::control::{MockSyncthing, SyncthingApi};

    #[tokio::test]
    async fn add_remove_peer_reshares_library_folder() {
        let mock = Arc::new(MockSyncthing::new("SELF"));
        let engine =
            SyncEngine::for_test(mock.clone(), "SELF".into(), "/tmp/flts-lib".into());

        engine.add_peer("PEER1", "Laptop").await.unwrap();

        let folders = mock.folders();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].path, "/tmp/flts-lib");
        assert!(folders[0].device_ids.contains(&"SELF".to_string()));
        assert!(folders[0].device_ids.contains(&"PEER1".to_string()));

        let peers = engine.list_peers().await.unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].device_id, "PEER1");
        assert!(!peers[0].connected);

        engine.remove_peer("PEER1").await.unwrap();
        assert!(engine.list_peers().await.unwrap().is_empty());
        let folders = mock.folders();
        assert!(!folders.last().unwrap().device_ids.contains(&"PEER1".to_string()));
    }

    #[tokio::test]
    async fn start_time_reshare_covers_persisted_peers() {
        // Post-restart state: peers persisted in the device list, folder not
        // yet shared with them.
        let mock = Arc::new(MockSyncthing::new("SELF"));
        mock.add_device("PEER1", "Laptop").await.unwrap();
        mock.add_device("PEER2", "Phone").await.unwrap();
        let engine =
            SyncEngine::for_test(mock.clone(), "SELF".into(), "/tmp/flts-lib".into());

        engine.reshare_library().await.unwrap();

        let folders = mock.folders();
        let shared = &folders.last().unwrap().device_ids;
        for id in ["SELF", "PEER1", "PEER2"] {
            assert!(shared.contains(&id.to_string()), "folder must be shared with {id}");
        }
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

        engine.set_device_name("My Mac").await.unwrap();
        engine.pair_device("PEER1", "Laptop").await.unwrap();

        let roster = RosterStore::new(&root, "SELF").load().unwrap();
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

        // Paired on another node: in the roster, unknown to this engine.
        RosterStore::new(&root, "PEERX").add_device("PEERX", "Other").unwrap();
        assert!(engine.list_peers().await.unwrap().is_empty());

        engine.reconcile_once().await.unwrap();

        let peers = engine.list_peers().await.unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].device_id, "PEERX");
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

        RosterStore::new(&root, "OTHER").remove_device("PEER1").unwrap();
        engine.reconcile_once().await.unwrap();

        assert!(engine.list_peers().await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Hermetic run against the real Go engine; a successful `start` proves the
    /// REST config calls all worked.
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
            options: OptionsPatch::loopback(),
            loopback_only: true,
        })
        .await
        .expect("engine starts and configures");

        let id = engine.my_id().to_string();
        assert!(id.len() >= 50 && id.contains('-'), "looks like a device ID: {id:?}");

        let devices_self = engine.client().my_id().await.unwrap();
        assert_eq!(devices_self, id, "client talks to the same engine");

        engine.stop().await.expect("engine stops cleanly");
        let _ = std::fs::remove_dir_all(&base);
    }
}
