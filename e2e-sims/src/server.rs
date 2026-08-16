//! Sim state shared by the fault layer and the control API, plus composition.

use crate::{control, fault, rules::RuleSet};
use axum::Router;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::{net::TcpListener, sync::Notify, task::JoinHandle};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRecord {
    pub method: String,
    pub path: String,
    pub body: String,
    pub ts_ms: u128,
}

pub struct SimState {
    pub rules: Mutex<RuleSet>,
    pub log: Mutex<Vec<RequestRecord>>,
    /// Resets the sim's stateful core.
    pub seed_reset: Box<dyn Fn() + Send + Sync>,
    pub seed: Box<dyn Fn(serde_json::Value) -> Result<(), String> + Send + Sync>,
    /// Wakes stalled handlers on reset/teardown.
    pub stall_abort: Notify,
}

impl SimState {
    pub fn new(
        seed_reset: Box<dyn Fn() + Send + Sync>,
        seed: Box<dyn Fn(serde_json::Value) -> Result<(), String> + Send + Sync>,
    ) -> Self {
        Self {
            rules: Mutex::new(RuleSet::default()),
            log: Mutex::new(Vec::new()),
            seed_reset,
            seed,
            stall_abort: Notify::new(),
        }
    }
}

impl Default for SimState {
    fn default() -> Self {
        Self::new(Box::new(|| {}), Box::new(|_| Ok(())))
    }
}

/// Wraps `inner` with the fault layer, adds `/_sim/*` (never faulted or logged),
/// binds 127.0.0.1:0.
pub async fn serve(inner: Router, state: Arc<SimState>) -> anyhow::Result<(u16, JoinHandle<()>)> {
    let app = control::router(state.clone()).merge(fault::wrap(inner, state));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            log::error!("sim server exited: {e}");
        }
    });
    Ok((port, handle))
}
