use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use tauri::Emitter;

static EVENT_VERSION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Serialize)]
pub struct VersionedPayload<T: Serialize> {
    pub version: u64,
    pub data: T,
}

pub fn emit_versioned<T: Serialize + Clone>(
    app: &tauri::AppHandle,
    event: &str,
    data: T,
) -> Result<(), tauri::Error> {
    let version = EVENT_VERSION.fetch_add(1, Ordering::SeqCst);
    app.emit(event, VersionedPayload { version, data })
}
