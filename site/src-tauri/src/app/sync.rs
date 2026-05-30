//! Tauri commands for native device sync.
//!
//! Phase 2 surface: read status, read this device's identity, toggle sync on/off
//! (which restarts/stops the engine via `eval_config`). Pairing and device-list
//! commands land in Phase 3.

use std::sync::Arc;

use serde::Serialize;

use crate::app::{AppState, sync_daemon::SyncStatus};

/// This device's identity, for display + the pairing QR/code.
#[derive(Debug, Clone, Serialize)]
pub struct ThisDevice {
    #[serde(rename = "deviceId")]
    pub device_id: String,
    pub name: Option<String>,
}

/// A paired peer device for the device-management list.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceEntry {
    #[serde(rename = "deviceId")]
    pub device_id: String,
    pub name: String,
    pub connected: bool,
}

#[tauri::command]
pub async fn get_sync_status(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<SyncStatus, String> {
    Ok(state.sync_status())
}

/// This device's Syncthing ID + configured name. `None` when sync isn't running
/// yet (engine not started), so the UI can prompt the user to enable it.
#[tauri::command]
pub async fn sync_get_this_device(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Option<ThisDevice>, String> {
    let Some(engine) = state.sync_engine().await else {
        return Ok(None);
    };
    let name = state.config_borrow_sync_device_name();
    Ok(Some(ThisDevice {
        device_id: engine.my_id().to_string(),
        name,
    }))
}

/// Enable or disable native sync. Persists the flag and re-evaluates config,
/// which starts or stops the embedded engine.
#[tauri::command]
pub async fn sync_set_enabled(
    state: tauri::State<'_, Arc<AppState>>,
    enabled: bool,
) -> Result<(), String> {
    state
        .set_sync_enabled(enabled)
        .await
        .map_err(|err| err.to_string())
}

/// Paired peer devices with live connection state. Empty when sync isn't
/// running.
#[tauri::command]
pub async fn sync_list_devices(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Vec<DeviceEntry>, String> {
    let Some(engine) = state.sync_engine().await else {
        return Ok(Vec::new());
    };
    let peers = engine.list_peers().await.map_err(|err| err.to_string())?;
    Ok(peers
        .into_iter()
        .map(|p| DeviceEntry {
            device_id: p.device_id,
            name: p.name,
            connected: p.connected,
        })
        .collect())
}

/// Pair with a peer: add its device ID (from a scanned/pasted code) and share
/// the library folder. The peer must add this device too — the roster mesh
/// (Phase 4) propagates that automatically once one side pairs.
#[tauri::command]
pub async fn sync_add_device(
    state: tauri::State<'_, Arc<AppState>>,
    device_id: String,
    name: String,
) -> Result<(), String> {
    let engine = state
        .sync_engine()
        .await
        .ok_or_else(|| "sync is not running; enable it first".to_string())?;
    engine
        .add_peer(device_id.trim(), name.trim())
        .await
        .map_err(|err| err.to_string())
}

/// Unpair a peer device and stop sharing the library with it.
#[tauri::command]
pub async fn sync_remove_device(
    state: tauri::State<'_, Arc<AppState>>,
    device_id: String,
) -> Result<(), String> {
    let engine = state
        .sync_engine()
        .await
        .ok_or_else(|| "sync is not running".to_string())?;
    engine
        .remove_peer(&device_id)
        .await
        .map_err(|err| err.to_string())
}
