//! Embedded Syncthing engine lifecycle.
//!
//! Brings the Go engine up via `syncthing-sys`, waits for its REST API, and
//! configures the single FLTS folder + discovery options. One engine per
//! process (the Go side holds global state), so this is created once by the
//! sync daemon and torn down on shutdown.

use std::{
    net::TcpListener,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};

use super::control::{FolderSpec, HttpSyncthing, OptionsPatch, SyncthingApi};

/// Fixed app folder ID for the synced library. Stable across devices.
pub const LIBRARY_FOLDER_ID: &str = "flts-library";

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
                path: cfg.library_root.to_string_lossy().into_owned(),
                device_ids: vec![my_id.clone()],
            })
            .await?;

        Ok(Self { client, my_id })
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
