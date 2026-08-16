use axum::{Json, Router, routing::get};
use e2e_sims::server::{SimState, serve};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

async fn start() -> String {
    let inner = Router::new().route("/hello", get(|| async { Json(json!({"msg": "hi"})) }));
    let (port, _h) = serve(inner, Arc::new(SimState::default())).await.unwrap();
    format!("http://127.0.0.1:{port}")
}

async fn push_rule(c: &reqwest::Client, base: &str, rule: Value) {
    let r = c
        .post(format!("{base}/_sim/rules"))
        .json(&rule)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().await.unwrap()["count"], 1);
}

#[tokio::test]
async fn passthrough_and_status_rule() {
    let base = start().await;
    let c = reqwest::Client::new();

    let r = c.get(format!("{base}/hello")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().await.unwrap()["msg"], "hi");

    push_rule(
        &c,
        &base,
        json!({"matcher": {}, "action": {"type": "status", "code": 503}, "times": 1}),
    )
    .await;

    let r = c.get(format!("{base}/hello")).send().await.unwrap();
    assert_eq!(r.status(), 503);
    let r = c.get(format!("{base}/hello")).send().await.unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn request_log_records_calls() {
    let base = start().await;
    let c = reqwest::Client::new();
    for _ in 0..2 {
        c.get(format!("{base}/hello")).send().await.unwrap();
    }

    let log: Vec<Value> = c
        .get(format!("{base}/_sim/requests"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(log.len(), 2);
    for rec in &log {
        assert_eq!(rec["method"], "GET");
        assert_eq!(rec["path"], "/hello");
        assert!(rec["tsMs"].as_u64().unwrap() > 0);
    }
}

#[tokio::test]
async fn truncate_yields_invalid_json() {
    let base = start().await;
    let c = reqwest::Client::new();
    push_rule(
        &c,
        &base,
        json!({"matcher": {}, "action": {"type": "truncate", "fraction": 0.5}}),
    )
    .await;

    // Headers promise the whole body; the short one on the wire must read as an
    // incomplete message, not a well-formed short response.
    let full = r#"{"msg":"hi"}"#.len() as u64;
    let err = match c.get(format!("{base}/hello")).send().await {
        Err(e) => format!("{e:?}"),
        Ok(r) => {
            assert_eq!(
                r.content_length(),
                Some(full),
                "truncate must keep the full body's Content-Length"
            );
            format!(
                "{:?}",
                r.text()
                    .await
                    .expect_err("truncated body must not read back")
            )
        }
    };
    // Distinguishes a short-of-Content-Length body from any other failure.
    assert!(err.contains("IncompleteMessage"), "{err}");
}

#[tokio::test]
async fn stall_released_by_reset() {
    let base = start().await;
    let c = reqwest::Client::new();
    push_rule(
        &c,
        &base,
        json!({"matcher": {}, "action": {"type": "stall"}, "times": 1}),
    )
    .await;

    let (c2, u) = (c.clone(), format!("{base}/hello"));
    let mut pending = tokio::spawn(async move { c2.get(u).send().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut pending)
            .await
            .is_err(),
        "stalled request must not complete"
    );

    let r = c.post(format!("{base}/_sim/reset")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let _ = tokio::time::timeout(Duration::from_secs(2), &mut pending)
        .await
        .expect("reset must release the stall")
        .unwrap();

    let r = c.get(format!("{base}/hello")).send().await.unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn drop_closes_connection() {
    let base = start().await;
    let c = reqwest::Client::new();
    push_rule(
        &c,
        &base,
        json!({"matcher": {}, "action": {"type": "drop"}}),
    )
    .await;

    let err = match c.get(format!("{base}/hello")).send().await {
        Err(e) => e.to_string(),
        Ok(r) => r
            .text()
            .await
            .expect_err("body must not complete")
            .to_string(),
    };
    assert!(!err.is_empty());
}
