//! Syncthing REST control client.
//!
//! One async trait, a reqwest HTTP implementation, and an in-memory mock.
//!
//! Only the fields we read are typed. Config mutations go through
//! `serde_json::Value` — fetch a defaults blob, tweak, PUT it back — so
//! Syncthing's large, version-drifting config schema needn't be tracked.

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

/// An unknown device that tried to connect, surfaced for user approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDevice {
    pub device_id: String,
    pub name: String,
}

/// A folder to create-or-update. `device_ids` must include this device.
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
    /// BEP listen addresses. Dynamic ports (`:0`) keep the embedded engine off
    /// `22000`, so a user's own Syncthing install can coexist. Empty leaves the
    /// engine default untouched.
    pub listen_addresses: Vec<String>,
}

impl Default for OptionsPatch {
    /// Reach peers anywhere, on a dynamic port.
    ///
    /// TCP only: quic-go panics in its TLS handshake on newer Go toolchains,
    /// and the in-process engine would take the app down with it. Relays plus
    /// global discovery still cover off-LAN reach.
    fn default() -> Self {
        Self {
            global_discovery: true,
            local_discovery: true,
            relays: true,
            nat: true,
            listen_addresses: vec!["tcp://0.0.0.0:0".into()],
        }
    }
}

impl OptionsPatch {
    /// Loopback-only, for tests that must not touch the network.
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
    /// This device's Syncthing ID; also the "is the engine up?" probe.
    async fn my_id(&self) -> Result<String>;

    /// Devices in the config; filter against `my_id` for peers only.
    async fn list_devices(&self) -> Result<Vec<DeviceInfo>>;

    /// Adds or updates a peer, setting `autoAcceptFolders` so folders it shares
    /// need no manual approval.
    async fn add_device(&self, device_id: &str, name: &str) -> Result<()>;

    async fn remove_device(&self, device_id: &str) -> Result<()>;

    /// Renames a device; sets the announced name, which otherwise is the
    /// hostname.
    async fn rename_device(&self, device_id: &str, name: &str) -> Result<()>;

    /// Pins a peer's addresses, wiring static topology in lieu of discovery.
    /// Production leaves devices on `dynamic`.
    async fn set_device_addresses(&self, device_id: &str, addresses: Vec<String>) -> Result<()>;

    /// Per-device `connected` flag, keyed by device ID.
    async fn connections(&self) -> Result<HashMap<String, bool>>;

    async fn ensure_folder(&self, spec: FolderSpec) -> Result<()>;

    /// Toggle global/local discovery, relays, and NAT traversal.
    async fn set_options(&self, opts: OptionsPatch) -> Result<()>;

    /// Devices that tried to connect; accept one via `add_device`.
    async fn pending_devices(&self) -> Result<Vec<PendingDevice>>;

    /// This device's folder completion, 0–100; below 100 means still pulling.
    async fn folder_completion(&self, folder_id: &str) -> Result<f64>;
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

    /// Retries only connection failures: the request never reached the engine,
    /// so even non-idempotent verbs are safe. A response is committed to.
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
        // Start from the engine's defaults so required fields stay sane.
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

    async fn rename_device(&self, device_id: &str, name: &str) -> Result<()> {
        let mut device = self.get(&format!("/rest/config/devices/{device_id}")).await?;
        device["name"] = serde_json::Value::String(name.to_string());
        self.put(&format!("/rest/config/devices/{device_id}"), &device)
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

    async fn folder_completion(&self, folder_id: &str) -> Result<f64> {
        let value = self
            .get(&format!("/rest/db/completion?folder={folder_id}"))
            .await?;
        Ok(value
            .get("completion")
            .and_then(|c| c.as_f64())
            .unwrap_or(100.0))
    }

    async fn pending_devices(&self) -> Result<Vec<PendingDevice>> {
        // `{ "<deviceID>": { "time": "...", "name": "...", "address": "..." } }`
        let value = self.get("/rest/cluster/pending/devices").await?;
        let mut out = Vec::new();
        if let Some(map) = value.as_object() {
            for (device_id, info) in map {
                let name = info
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push(PendingDevice {
                    device_id: device_id.clone(),
                    name,
                });
            }
        }
        Ok(out)
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
    pending: Vec<PendingDevice>,
    completion: f64,
}

/// In-memory `SyncthingApi`: records mutations, serves configured state.
pub struct MockSyncthing {
    state: Mutex<MockState>,
}

impl MockSyncthing {
    pub fn new(my_id: &str) -> Self {
        Self {
            state: Mutex::new(MockState {
                my_id: my_id.to_string(),
                completion: 100.0,
                ..Default::default()
            }),
        }
    }

    pub fn set_completion(&self, pct: f64) {
        self.state.lock().unwrap().completion = pct;
    }

    /// Marks a peer connected/disconnected, driving `connections()`.
    pub fn set_connected(&self, device_id: &str, connected: bool) {
        self.state
            .lock()
            .unwrap()
            .connected
            .insert(device_id.to_string(), connected);
    }

    pub fn folders(&self) -> Vec<FolderSpec> {
        self.state.lock().unwrap().folders.clone()
    }

    pub fn options(&self) -> Option<OptionsPatch> {
        self.state.lock().unwrap().options.clone()
    }

    pub fn set_pending(&self, device_id: &str, name: &str) {
        self.state.lock().unwrap().pending.push(PendingDevice {
            device_id: device_id.to_string(),
            name: name.to_string(),
        });
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

    async fn rename_device(&self, device_id: &str, name: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if let Some(d) = state.devices.iter_mut().find(|d| d.device_id == device_id) {
            d.name = name.to_string();
        } else {
            state.devices.push(DeviceInfo {
                device_id: device_id.to_string(),
                name: name.to_string(),
            });
        }
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

    async fn pending_devices(&self) -> Result<Vec<PendingDevice>> {
        Ok(self.state.lock().unwrap().pending.clone())
    }

    async fn folder_completion(&self, _folder_id: &str) -> Result<f64> {
        Ok(self.state.lock().unwrap().completion)
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
    async fn mock_surfaces_pending_devices() {
        let api = MockSyncthing::new("ME");
        assert!(api.pending_devices().await.unwrap().is_empty());
        api.set_pending("PEER-X", "iPad");
        let pending = api.pending_devices().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].device_id, "PEER-X");
        assert_eq!(pending[0].name, "iPad");
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
