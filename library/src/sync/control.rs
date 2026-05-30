//! Syncthing REST control client.
//!
//! Mirrors the [`AnkiConnect`](crate::anki::connect) shape: one async trait, a
//! reqwest HTTP implementation (`X-API-Key`, timeout, send-retry), and an
//! in-memory mock for unit tests. Higher layers (`sync::engine`, the sync
//! daemon) program against the trait.
//!
//! We model only the few fields we read as typed structs; the engine's *config*
//! mutations (folders, options, devices) go through `serde_json::Value` so we
//! don't have to track Syncthing's large, version-drifting config schema — we
//! fetch a defaults blob, tweak the handful of fields we care about, and PUT it
//! back.

use std::{collections::HashMap, sync::Mutex, time::Duration};

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_RETRY_ATTEMPTS: u32 = 3;
const HTTP_RETRY_DELAYS_MS: [u64; 2] = [100, 300];

/// This-device identity reported by `GET /rest/system/status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SystemStatus {
    #[serde(rename = "myID")]
    pub my_id: String,
}

/// A device entry as carried in the Syncthing config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceInfo {
    #[serde(rename = "deviceID")]
    pub device_id: String,
    #[serde(default)]
    pub name: String,
}

/// A Syncthing folder to create-or-update (`ensure_folder`). The peer list must
/// include this device; the engine passes the full membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderSpec {
    pub id: String,
    pub label: String,
    pub path: String,
    pub device_ids: Vec<String>,
}

/// The discovery/connectivity options we toggle. Maps onto fields of
/// `GET/PUT /rest/config/options`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionsPatch {
    pub global_discovery: bool,
    pub local_discovery: bool,
    pub relays: bool,
    pub nat: bool,
    /// BEP listen addresses (the `listenAddresses` option). FLTS uses dynamic
    /// ports (`:0`) instead of Syncthing's default `22000` so the embedded
    /// engine **coexists with a user's own Syncthing install** rather than
    /// fighting it for the port. Empty = leave the engine default untouched.
    pub listen_addresses: Vec<String>,
}

impl Default for OptionsPatch {
    /// Production default: reach peers anywhere (needed for iPad off-LAN), on
    /// dynamic ports so we never collide with another Syncthing on this host.
    fn default() -> Self {
        Self {
            global_discovery: true,
            local_discovery: true,
            relays: true,
            nat: true,
            listen_addresses: vec!["tcp://0.0.0.0:0".into(), "quic://0.0.0.0:0".into()],
        }
    }
}

impl OptionsPatch {
    /// Fully local: no discovery, relays, or NAT, BEP bound to loopback only.
    /// For unit/integration tests that must not touch the network.
    pub fn loopback() -> Self {
        Self {
            global_discovery: false,
            local_discovery: false,
            relays: false,
            nat: false,
            listen_addresses: vec!["tcp://127.0.0.1:0".into()],
        }
    }
}

#[async_trait]
pub trait SyncthingApi: Send + Sync {
    /// This device's Syncthing ID. Also the natural "is the engine up?" probe.
    async fn my_id(&self) -> Result<String>;

    /// Devices currently in the config (excluding or including self per
    /// Syncthing — callers filter against `my_id` when they need peers only).
    async fn list_devices(&self) -> Result<Vec<DeviceInfo>>;

    /// Add (or update) a peer device. Sets `autoAcceptFolders` so a folder the
    /// peer shares with us is accepted without manual approval.
    async fn add_device(&self, device_id: &str, name: &str) -> Result<()>;

    /// Remove a peer device from the config.
    async fn remove_device(&self, device_id: &str) -> Result<()>;

    /// Pin a peer's connection addresses (e.g. `tcp://host:22000`). Used by the
    /// test harness to wire static topology in lieu of discovery; production
    /// leaves devices on `dynamic`.
    async fn set_device_addresses(&self, device_id: &str, addresses: Vec<String>) -> Result<()>;

    /// Per-device connection state, keyed by device ID (`connected` flag from
    /// `GET /rest/system/connections`).
    async fn connections(&self) -> Result<HashMap<String, bool>>;

    /// Create or update a folder, pointing it at `spec.path` with the given
    /// peer membership.
    async fn ensure_folder(&self, spec: FolderSpec) -> Result<()>;

    /// Toggle global/local discovery, relays, and NAT traversal.
    async fn set_options(&self, opts: OptionsPatch) -> Result<()>;
}

// ---------- HTTP implementation ----------

/// Talks to a running engine's localhost REST API.
pub struct HttpSyncthing {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl HttpSyncthing {
    /// `base_url` is the GUI/REST origin, e.g. `http://127.0.0.1:8384`.
    pub fn new(base_url: String, api_key: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .expect("reqwest client builds");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            client,
        }
    }

    async fn get(&self, path: &str) -> Result<serde_json::Value> {
        let text = self.send(reqwest::Method::GET, path, None).await?;
        serde_json::from_str(&text)
            .map_err(|e| anyhow!("syncthing: decoding GET {path} failed: {e}"))
    }

    async fn put(&self, path: &str, body: &serde_json::Value) -> Result<()> {
        self.send(reqwest::Method::PUT, path, Some(body)).await?;
        Ok(())
    }

    async fn delete(&self, path: &str) -> Result<()> {
        self.send(reqwest::Method::DELETE, path, None).await?;
        Ok(())
    }

    /// One REST round-trip. Retries only connection (`send`) failures — the
    /// request never reached the engine, so even non-idempotent verbs are safe
    /// to retry. Once a response arrives we commit to it.
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<String> {
        let url = format!("{}{}", self.base_url, path);
        let mut last_err: Option<reqwest::Error> = None;
        let mut resp = None;
        for attempt in 0..HTTP_RETRY_ATTEMPTS {
            let mut req = self
                .client
                .request(method.clone(), &url)
                .header("X-API-Key", &self.api_key);
            if let Some(body) = body {
                req = req.json(body);
            }
            match req.send().await {
                Ok(r) => {
                    resp = Some(r);
                    break;
                }
                Err(e) => {
                    if attempt + 1 < HTTP_RETRY_ATTEMPTS {
                        let delay = HTTP_RETRY_DELAYS_MS[attempt as usize];
                        log::debug!(
                            "syncthing: transient send error on {method} {path} \
                             (attempt {}/{HTTP_RETRY_ATTEMPTS}): {e}; retrying in {delay}ms",
                            attempt + 1,
                        );
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    last_err = Some(e);
                }
            }
        }
        let resp = match resp {
            Some(r) => r,
            None => bail!(
                "syncthing: {method} {path} failed: {}",
                last_err.expect("a send error when resp is None")
            ),
        };
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| anyhow!("syncthing: reading {method} {path} body failed: {e}"))?;
        if !status.is_success() {
            bail!("syncthing: {method} {path} → HTTP {status}: {text}");
        }
        Ok(text)
    }
}

#[async_trait]
impl SyncthingApi for HttpSyncthing {
    async fn my_id(&self) -> Result<String> {
        let status: SystemStatus = serde_json::from_value(self.get("/rest/system/status").await?)
            .map_err(|e| anyhow!("syncthing: decoding system status failed: {e}"))?;
        Ok(status.my_id)
    }

    async fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
        let devices = self.get("/rest/config/devices").await?;
        serde_json::from_value(devices)
            .map_err(|e| anyhow!("syncthing: decoding devices failed: {e}"))
    }

    async fn add_device(&self, device_id: &str, name: &str) -> Result<()> {
        // Start from the engine's device defaults so required fields are sane,
        // then set identity + auto-accept.
        let mut device = self.get("/rest/config/defaults/device").await?;
        device["deviceID"] = serde_json::Value::String(device_id.to_string());
        device["name"] = serde_json::Value::String(name.to_string());
        device["autoAcceptFolders"] = serde_json::Value::Bool(true);
        self.put(&format!("/rest/config/devices/{device_id}"), &device)
            .await
    }

    async fn remove_device(&self, device_id: &str) -> Result<()> {
        self.delete(&format!("/rest/config/devices/{device_id}"))
            .await
    }

    async fn set_device_addresses(&self, device_id: &str, addresses: Vec<String>) -> Result<()> {
        let mut device = self.get(&format!("/rest/config/devices/{device_id}")).await?;
        device["addresses"] = serde_json::Value::Array(
            addresses
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        );
        self.put(&format!("/rest/config/devices/{device_id}"), &device)
            .await
    }

    async fn connections(&self) -> Result<HashMap<String, bool>> {
        let value = self.get("/rest/system/connections").await?;
        let mut out = HashMap::new();
        if let Some(conns) = value.get("connections").and_then(|c| c.as_object()) {
            for (id, info) in conns {
                let connected = info
                    .get("connected")
                    .and_then(|c| c.as_bool())
                    .unwrap_or(false);
                out.insert(id.clone(), connected);
            }
        }
        Ok(out)
    }

    async fn ensure_folder(&self, spec: FolderSpec) -> Result<()> {
        // Start from folder defaults, then set identity, path, and membership.
        let mut folder = self.get("/rest/config/defaults/folder").await?;
        folder["id"] = serde_json::Value::String(spec.id.clone());
        folder["label"] = serde_json::Value::String(spec.label);
        folder["path"] = serde_json::Value::String(spec.path);
        folder["devices"] = serde_json::Value::Array(
            spec.device_ids
                .iter()
                .map(|id| serde_json::json!({ "deviceID": id }))
                .collect(),
        );
        self.put(&format!("/rest/config/folders/{}", spec.id), &folder)
            .await
    }

    async fn set_options(&self, opts: OptionsPatch) -> Result<()> {
        let mut options = self.get("/rest/config/options").await?;
        options["globalAnnounceEnabled"] = serde_json::Value::Bool(opts.global_discovery);
        options["localAnnounceEnabled"] = serde_json::Value::Bool(opts.local_discovery);
        options["relaysEnabled"] = serde_json::Value::Bool(opts.relays);
        options["natEnabled"] = serde_json::Value::Bool(opts.nat);
        if !opts.listen_addresses.is_empty() {
            options["listenAddresses"] = serde_json::Value::Array(
                opts.listen_addresses
                    .iter()
                    .map(|a| serde_json::Value::String(a.clone()))
                    .collect(),
            );
        }
        self.put("/rest/config/options", &options).await
    }
}

// ---------- In-memory mock ----------

#[derive(Default)]
struct MockState {
    my_id: String,
    devices: Vec<DeviceInfo>,
    folders: Vec<FolderSpec>,
    options: Option<OptionsPatch>,
    connected: HashMap<String, bool>,
    addresses: HashMap<String, Vec<String>>,
}

/// In-memory `SyncthingApi` for unit tests — records mutations and serves back
/// configured state without a running engine.
pub struct MockSyncthing {
    state: Mutex<MockState>,
}

impl MockSyncthing {
    pub fn new(my_id: &str) -> Self {
        Self {
            state: Mutex::new(MockState {
                my_id: my_id.to_string(),
                ..Default::default()
            }),
        }
    }

    /// Mark a peer connected/disconnected (drives `connections()` in tests).
    pub fn set_connected(&self, device_id: &str, connected: bool) {
        self.state
            .lock()
            .unwrap()
            .connected
            .insert(device_id.to_string(), connected);
    }

    /// Snapshot of the folders the engine was asked to ensure.
    pub fn folders(&self) -> Vec<FolderSpec> {
        self.state.lock().unwrap().folders.clone()
    }

    /// The last options patch applied, if any.
    pub fn options(&self) -> Option<OptionsPatch> {
        self.state.lock().unwrap().options.clone()
    }
}

#[async_trait]
impl SyncthingApi for MockSyncthing {
    async fn my_id(&self) -> Result<String> {
        Ok(self.state.lock().unwrap().my_id.clone())
    }

    async fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
        Ok(self.state.lock().unwrap().devices.clone())
    }

    async fn add_device(&self, device_id: &str, name: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if let Some(existing) = state.devices.iter_mut().find(|d| d.device_id == device_id) {
            existing.name = name.to_string();
        } else {
            state.devices.push(DeviceInfo {
                device_id: device_id.to_string(),
                name: name.to_string(),
            });
        }
        Ok(())
    }

    async fn remove_device(&self, device_id: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        state.devices.retain(|d| d.device_id != device_id);
        state.connected.remove(device_id);
        state.addresses.remove(device_id);
        Ok(())
    }

    async fn set_device_addresses(&self, device_id: &str, addresses: Vec<String>) -> Result<()> {
        self.state
            .lock()
            .unwrap()
            .addresses
            .insert(device_id.to_string(), addresses);
        Ok(())
    }

    async fn connections(&self) -> Result<HashMap<String, bool>> {
        Ok(self.state.lock().unwrap().connected.clone())
    }

    async fn ensure_folder(&self, spec: FolderSpec) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        state.folders.retain(|f| f.id != spec.id);
        state.folders.push(spec);
        Ok(())
    }

    async fn set_options(&self, opts: OptionsPatch) -> Result<()> {
        self.state.lock().unwrap().options = Some(opts);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_records_devices_folders_and_options() {
        let api = MockSyncthing::new("THIS-DEVICE");
        assert_eq!(api.my_id().await.unwrap(), "THIS-DEVICE");

        api.add_device("PEER-ONE", "Laptop").await.unwrap();
        api.add_device("PEER-ONE", "Laptop Renamed").await.unwrap();
        api.add_device("PEER-TWO", "Phone").await.unwrap();
        let devices = api.list_devices().await.unwrap();
        assert_eq!(devices.len(), 2, "duplicate add updates in place");
        assert_eq!(devices[0].name, "Laptop Renamed");

        api.remove_device("PEER-ONE").await.unwrap();
        assert_eq!(api.list_devices().await.unwrap().len(), 1);

        api.ensure_folder(FolderSpec {
            id: "flts-library".into(),
            label: "FLTS".into(),
            path: "/tmp/lib".into(),
            device_ids: vec!["THIS-DEVICE".into(), "PEER-TWO".into()],
        })
        .await
        .unwrap();
        assert_eq!(api.folders().len(), 1);
        assert_eq!(api.folders()[0].device_ids.len(), 2);

        api.set_options(OptionsPatch::default()).await.unwrap();
        assert_eq!(api.options(), Some(OptionsPatch::default()));
    }

    #[tokio::test]
    async fn mock_tracks_connection_state() {
        let api = MockSyncthing::new("ME");
        api.set_connected("PEER", true);
        assert_eq!(api.connections().await.unwrap().get("PEER"), Some(&true));
        api.set_connected("PEER", false);
        assert_eq!(api.connections().await.unwrap().get("PEER"), Some(&false));
    }
}
