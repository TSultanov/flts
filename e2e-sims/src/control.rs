//! `/_sim/*` control API. Never faulted and never logged.

use crate::{
    rules::{Rule, RuleSet},
    server::{RequestRecord, SimState},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use std::sync::Arc;

pub fn router(state: Arc<SimState>) -> Router {
    Router::new()
        .route("/_sim/rules", post(add_rules).delete(clear_rules))
        .route("/_sim/reset", post(reset))
        .route("/_sim/seed", post(seed))
        .route("/_sim/requests", get(requests))
        .with_state(state)
}

async fn add_rules(State(state): State<Arc<SimState>>, body: Bytes) -> Response {
    let parsed = serde_json::from_slice::<Vec<Rule>>(&body)
        .or_else(|_| serde_json::from_slice::<Rule>(&body).map(|r| vec![r]));
    let rules = match parsed {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    let mut rs = state.rules.lock().unwrap();
    for r in rules {
        rs.push(r);
    }
    Json(serde_json::json!({ "count": rs.len() })).into_response()
}

async fn clear_rules(State(state): State<Arc<SimState>>) -> StatusCode {
    state.rules.lock().unwrap().clear();
    StatusCode::OK
}

/// Full reset: rules, call counter, log, stalled handlers, sim core.
async fn reset(State(state): State<Arc<SimState>>) -> StatusCode {
    *state.rules.lock().unwrap() = RuleSet::default();
    state.log.lock().unwrap().clear();
    state.stall_abort.notify_waiters();
    (state.seed_reset)();
    StatusCode::OK
}

async fn seed(State(state): State<Arc<SimState>>, Json(body): Json<serde_json::Value>) -> Response {
    match (state.seed)(body) {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

async fn requests(State(state): State<Arc<SimState>>) -> Json<Vec<RequestRecord>> {
    Json(state.log.lock().unwrap().clone())
}
