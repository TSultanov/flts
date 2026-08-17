//! Tauri commands for native device sync.

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

/// An unknown device awaiting approval (the other half of a one-sided pairing).
#[derive(Debug, Clone, Serialize)]
pub struct PendingEntry {
    #[serde(rename = "deviceId")]
    pub device_id: String,
    pub name: String,
}

#[tauri::command]
pub async fn get_sync_status(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<SyncStatus, String> {
    Ok(state.sync_status())
}

/// This device's Syncthing ID + name; `None` while the engine isn't running.
#[tauri::command]
pub async fn sync_get_this_device(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Option<ThisDevice>, String> {
    let Some(engine) = state.sync_engine().await? else {
        return Ok(None);
    };
    let my_id = engine.my_id().to_string();
    // Syncthing's own device entry is authoritative; config is the fallback.
    let name = engine
        .client()
        .list_devices()
        .await
        .ok()
        .and_then(|devs| devs.into_iter().find(|d| d.device_id == my_id).map(|d| d.name))
        .filter(|n| !n.trim().is_empty())
        .or_else(|| state.config_borrow_sync_device_name());
    Ok(Some(ThisDevice {
        device_id: my_id,
        name,
    }))
}

/// Loopback URL of Syncthing's dashboard. `Some` only in debug builds with the
/// engine up: release ships `-tags noassets`, so `None` hides the UI button.
#[tauri::command]
pub async fn sync_web_ui_url(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Option<String>, String> {
    if !cfg!(debug_assertions) {
        return Ok(None);
    }
    Ok(state.sync_engine().await?.map(|e| e.gui_url().to_string()))
}

/// Call on mobile foreground: restarts an engine that went unreachable while
/// suspended. No-op when sync is off or healthy.
#[tauri::command]
pub async fn sync_wake(state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    state.wake_sync().await;
    Ok(())
}

/// Renames this device across config, the running engine, and the roster.
#[tauri::command]
pub async fn sync_set_device_name(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
) -> Result<(), String> {
    state
        .set_sync_device_name(name)
        .await
        .map_err(|err| err.to_string())
}

/// Persists the flag and re-evaluates config, starting or stopping the engine.
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
    let Some(engine) = state.sync_engine().await? else {
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

/// Adds a peer's device ID and shares the library folder. The peer must add this
/// device too, which the roster mesh propagates once either side pairs.
#[tauri::command]
pub async fn sync_add_device(
    state: tauri::State<'_, Arc<AppState>>,
    device_id: String,
    name: String,
) -> Result<(), String> {
    let engine = state
        .sync_engine()
        .await?
        .ok_or_else(|| "sync is not running; enable it first".to_string())?;
    engine
        .pair_device(device_id.trim(), name.trim())
        .await
        .map_err(|err| err.to_string())
}

/// Devices that tried to connect but aren't paired; accepting one via
/// `sync_add_device` completes pairing. Empty when sync isn't running.
#[tauri::command]
pub async fn sync_list_pending(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Vec<PendingEntry>, String> {
    let Some(engine) = state.sync_engine().await? else {
        return Ok(Vec::new());
    };
    let pending = engine
        .client()
        .pending_devices()
        .await
        .map_err(|err| err.to_string())?;
    Ok(pending
        .into_iter()
        .map(|p| PendingEntry {
            device_id: p.device_id,
            name: p.name,
        })
        .collect())
}

/// Unpair a peer device and stop sharing the library with it.
#[tauri::command]
pub async fn sync_remove_device(
    state: tauri::State<'_, Arc<AppState>>,
    device_id: String,
) -> Result<(), String> {
    let engine = state
        .sync_engine()
        .await?
        .ok_or_else(|| "sync is not running".to_string())?;
    engine
        .unpair_device(&device_id)
        .await
        .map_err(|err| err.to_string())
}
