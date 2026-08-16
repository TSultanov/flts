use e2e_sims::lrclib::lrclib_router;
use e2e_sims::server::{SimState, serve};
use serde_json::{Value, json};
use std::sync::Arc;

async fn start() -> (String, reqwest::Client) {
    let (inner, sim) = lrclib_router();
    let (reset, seed) = (sim.clone(), sim.clone());
    let state = Arc::new(SimState::new(
        Box::new(move || reset.reset()),
        Box::new(move |v| seed.seed(v)),
    ));
    let (port, _h) = serve(inner, state).await.unwrap();
    (format!("http://127.0.0.1:{port}"), reqwest::Client::new())
}

async fn seed(c: &reqwest::Client, base: &str, body: Value) {
    let r = c
        .post(format!("{base}/_sim/seed"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
}

async fn get(
    c: &reqwest::Client,
    base: &str,
    query: &[(&str, &str)],
) -> (reqwest::StatusCode, Value) {
    let r = c
        .get(format!("{base}/api/get"))
        .query(query)
        .send()
        .await
        .unwrap();
    (r.status(), r.json().await.unwrap())
}

fn catalog() -> Value {
    json!([{
        "artist": "Mecano",
        "title": "Hijo de la Luna",
        "album": "Entre el Cielo y el Suelo",
        "syncedLyrics": "[00:01.00] Tonto el que no entienda",
        "plainLyrics": "Tonto el que no entienda",
    }])
}

#[tokio::test]
async fn seeded_track_returns_synced_lyrics() {
    let (base, c) = start().await;
    seed(&c, &base, catalog()).await;

    let (status, body) = get(
        &c,
        &base,
        &[("artist_name", "Mecano"), ("track_name", "Hijo de la Luna")],
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["syncedLyrics"], "[00:01.00] Tonto el que no entienda");
    assert_eq!(body["plainLyrics"], "Tonto el que no entienda");
    assert_eq!(body["trackName"], "Hijo de la Luna");
    assert_eq!(body["artistName"], "Mecano");
    assert_eq!(body["albumName"], "Entre el Cielo y el Suelo");
    assert_eq!(body["id"], 1);
    assert_eq!(body["duration"], 0);
    assert_eq!(body["instrumental"], false);
}

#[tokio::test]
async fn missing_lyrics_fields_are_null_and_album_defaults_to_null() {
    let (base, c) = start().await;
    seed(
        &c,
        &base,
        json!([{"artist": "A", "title": "T", "plainLyrics": "just plain"}]),
    )
    .await;

    let (status, body) = get(&c, &base, &[("artist_name", "A"), ("track_name", "T")]).await;
    assert_eq!(status, 200);
    assert!(body["syncedLyrics"].is_null());
    assert_eq!(body["plainLyrics"], "just plain");
    assert!(body["albumName"].is_null());
}

#[tokio::test]
async fn unseeded_track_returns_lrclib_404_body() {
    let (base, c) = start().await;
    seed(&c, &base, catalog()).await;

    let (status, body) = get(
        &c,
        &base,
        &[("artist_name", "Mecano"), ("track_name", "Nowhere")],
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(
        body,
        json!({
            "statusCode": 404,
            "name": "TrackNotFound",
            "message": "Failed to find specified track",
        })
    );
}

#[tokio::test]
async fn album_and_duration_are_ignored_for_lookup_but_logged() {
    let (base, c) = start().await;
    seed(&c, &base, catalog()).await;

    let (status, _) = get(
        &c,
        &base,
        &[
            ("artist_name", "Mecano"),
            ("track_name", "Hijo de la Luna"),
            ("album_name", "Some Other Album"),
            ("duration", "226"),
        ],
    )
    .await;
    assert_eq!(status, 200);

    let log: Value = c
        .get(format!("{base}/_sim/requests"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(log[0]["path"], "/api/get");
    let query = log[0]["query"].as_str().unwrap();
    assert!(query.contains("album_name=Some"), "{query}");
    assert!(query.contains("duration=226"), "{query}");
}

#[tokio::test]
async fn reset_empties_the_catalog() {
    let (base, c) = start().await;
    seed(&c, &base, catalog()).await;
    let (status, _) = get(
        &c,
        &base,
        &[("artist_name", "Mecano"), ("track_name", "Hijo de la Luna")],
    )
    .await;
    assert_eq!(status, 200);

    let r = c.post(format!("{base}/_sim/reset")).send().await.unwrap();
    assert_eq!(r.status(), 200);

    let (status, body) = get(
        &c,
        &base,
        &[("artist_name", "Mecano"), ("track_name", "Hijo de la Luna")],
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(body["name"], "TrackNotFound");
}

#[tokio::test]
async fn seed_rejects_malformed_input() {
    let (base, c) = start().await;
    let r = c
        .post(format!("{base}/_sim/seed"))
        .json(&json!({"artist": "A"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    let r = c
        .post(format!("{base}/_sim/seed"))
        .json(&json!([{"title": "T"}]))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}
