use e2e_sims::anki::anki_router;
use e2e_sims::server::{SimState, serve};
use serde_json::{Value, json};
use std::sync::Arc;

async fn start() -> (String, reqwest::Client) {
    let (inner, sim) = anki_router();
    let (reset, seed) = (sim.clone(), sim.clone());
    let state = Arc::new(SimState::new(
        Box::new(move || reset.reset()),
        Box::new(move |v| seed.seed(v)),
    ));
    let (port, _h) = serve(inner, state).await.unwrap();
    (format!("http://127.0.0.1:{port}"), reqwest::Client::new())
}

async fn call(c: &reqwest::Client, base: &str, action: &str, params: Value) -> Value {
    let mut env = json!({"action": action, "version": 6});
    if !params.is_null() {
        env["params"] = params;
    }
    let r = c.post(base).json(&env).send().await.unwrap();
    assert_eq!(r.status(), 200);
    r.json().await.unwrap()
}

async fn ok(c: &reqwest::Client, base: &str, action: &str, params: Value) -> Value {
    let v = call(c, base, action, params).await;
    assert!(v["error"].is_null(), "{action} failed: {v}");
    v["result"].clone()
}

fn note(deck: &str, tag: &str) -> Value {
    json!({
        "deckName": deck,
        "modelName": "FLTS Bilingual v1",
        "fields": {"Source": "poder", "Target": "мочь", "Example": ""},
        "tags": [tag],
    })
}

async fn seed(c: &reqwest::Client, base: &str, body: Value) {
    let r = c
        .post(format!("{base}/_sim/seed"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn version_handshake() {
    let (base, c) = start().await;
    let v = call(&c, &base, "version", Value::Null).await;
    assert_eq!(v["result"], 6);
    assert!(v["error"].is_null());

    let v = call(&c, &base, "bogusAction", Value::Null).await;
    assert!(v["result"].is_null());
    assert_eq!(v["error"], "unsupported action: bogusAction");
}

#[tokio::test]
async fn add_then_find_then_info_roundtrip() {
    let (base, c) = start().await;
    seed(&c, &base, json!({"decks": ["FLTS"]})).await;

    let decks = ok(&c, &base, "deckNamesAndIds", Value::Null).await;
    assert!(decks["FLTS"].is_i64());

    let id = ok(
        &c,
        &base,
        "addNote",
        json!({"note": note("FLTS", "flts-test")}),
    )
    .await
    .as_i64()
    .unwrap();

    let hits = ok(&c, &base, "findNotes", json!({"query": "tag:flts-test"})).await;
    assert_eq!(hits, json!([id]));

    let infos = ok(&c, &base, "notesInfo", json!({"notes": [id]})).await;
    assert_eq!(infos[0]["noteId"], id);
    assert_eq!(infos[0]["tags"], json!(["flts-test"]));
    let cards = infos[0]["cards"].as_array().unwrap().clone();
    assert_eq!(cards.len(), 2);

    let card_id = cards[0].as_i64().unwrap();
    let ci = ok(&c, &base, "cardsInfo", json!({"cards": [card_id]})).await;
    assert_eq!(ci[0]["cardId"], card_id);
    assert_eq!(ci[0]["note"], id);
    assert_eq!(ci[0]["queue"], 0);

    ok(
        &c,
        &base,
        "updateNoteFields",
        json!({"note": {"id": id, "fields": {"Target": "уметь"}}}),
    )
    .await;
}

#[tokio::test]
async fn multi_batches_and_isolates_errors() {
    let (base, c) = start().await;
    seed(&c, &base, json!({"decks": ["FLTS"]})).await;

    let res = ok(
        &c,
        &base,
        "multi",
        json!({"actions": [
            {"action": "addNote", "params": {"note": note("FLTS", "good")}},
            {"action": "addNote", "params": {"note": note("Gone", "bad")}},
        ]}),
    )
    .await;
    let res = res.as_array().unwrap();
    assert_eq!(res.len(), 2);
    let good_id = res[0].as_i64().expect("bare success value");
    assert!(res[1]["result"].is_null());
    assert_eq!(res[1]["error"], "deck was not found: Gone");

    let hits = ok(&c, &base, "findNotes", json!({"query": "tag:good"})).await;
    assert_eq!(hits, json!([good_id]));
    let hits = ok(&c, &base, "findNotes", json!({"query": "tag:bad"})).await;
    assert_eq!(hits, json!([]));
}

#[tokio::test]
async fn state_survives_across_requests_and_resets() {
    let (base, c) = start().await;
    seed(
        &c,
        &base,
        json!({"decks": ["FLTS"], "notes": [{"deck": "FLTS", "model": "M", "fields": {"Source": "s"}, "tags": ["seeded"]}]}),
    )
    .await;

    let seeded = ok(&c, &base, "findNotes", json!({"query": "tag:seeded"})).await;
    assert_eq!(seeded.as_array().unwrap().len(), 1);
    let seeded_id = seeded[0].as_i64().unwrap();
    let infos = ok(&c, &base, "notesInfo", json!({"notes": [seeded_id]})).await;
    assert_eq!(infos[0]["cards"].as_array().unwrap().len(), 2);

    ok(&c, &base, "addNote", json!({"note": note("FLTS", "live")})).await;
    let hits = ok(&c, &base, "findNotes", json!({"query": "tag:live"})).await;
    assert_eq!(hits.as_array().unwrap().len(), 1);

    let r = c.post(format!("{base}/_sim/reset")).send().await.unwrap();
    assert_eq!(r.status(), 200);

    for tag in ["seeded", "live"] {
        let hits = ok(
            &c,
            &base,
            "findNotes",
            json!({"query": format!("tag:{tag}")}),
        )
        .await;
        assert_eq!(hits, json!([]), "tag:{tag} survived reset");
    }
    assert_eq!(
        ok(&c, &base, "deckNamesAndIds", Value::Null).await,
        json!({})
    );
    // Reset restores the id counter: deck takes 1, so the next note is 2.
    seed(&c, &base, json!({"decks": ["FLTS"]})).await;
    let id = ok(&c, &base, "addNote", json!({"note": note("FLTS", "again")})).await;
    assert_eq!(id, 2);
}
