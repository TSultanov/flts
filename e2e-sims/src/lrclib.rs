//! LRClib simulator: a seeded catalog behind `GET /api/get`.
//! Lookup is an exact (artist, title) match; `album_name`/`duration` only reach the request log.

use axum::{
    Json, Router,
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone)]
struct Track {
    album: Option<String>,
    synced: Option<String>,
    plain: Option<String>,
}

#[derive(Debug, Default)]
pub struct LrclibSimState {
    catalog: Mutex<HashMap<(String, String), Track>>,
}

impl LrclibSimState {
    pub fn reset(&self) {
        self.catalog.lock().unwrap().clear();
    }

    /// `[{artist, title, album?, syncedLyrics?, plainLyrics?}]`; lyrics fields nullable.
    pub fn seed(&self, v: Value) -> Result<(), String> {
        let tracks = v.as_array().ok_or("seed: expected an array")?;
        let mut parsed = Vec::with_capacity(tracks.len());
        for t in tracks {
            let artist = req_str(t, "artist")?;
            let title = req_str(t, "title")?;
            parsed.push((
                (artist, title),
                Track {
                    album: opt_str(t, "album")?,
                    synced: opt_str(t, "syncedLyrics")?,
                    plain: opt_str(t, "plainLyrics")?,
                },
            ));
        }
        // Insert only after the whole batch validates.
        self.catalog.lock().unwrap().extend(parsed);
        Ok(())
    }

    fn lookup(&self, artist: &str, title: &str) -> Option<Track> {
        self.catalog
            .lock()
            .unwrap()
            .get(&(artist.to_owned(), title.to_owned()))
            .cloned()
    }
}

fn req_str(t: &Value, key: &str) -> Result<String, String> {
    t.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("seed: `{key}` must be a string"))
}

fn opt_str(t: &Value, key: &str) -> Result<Option<String>, String> {
    match t.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("seed: `{key}` must be a string or null")),
    }
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "statusCode": 404,
            "name": "TrackNotFound",
            "message": "Failed to find specified track",
        })),
    )
        .into_response()
}

pub fn lrclib_router() -> (Router, Arc<LrclibSimState>) {
    let sim = Arc::new(LrclibSimState::default());
    let handler_sim = sim.clone();
    let router = Router::new().route(
        "/api/get",
        get(move |Query(q): Query<HashMap<String, String>>| {
            let sim = handler_sim.clone();
            async move {
                let artist = q.get("artist_name").map(String::as_str).unwrap_or_default();
                let title = q.get("track_name").map(String::as_str).unwrap_or_default();
                let Some(track) = sim.lookup(artist, title) else {
                    return not_found();
                };
                Json(json!({
                    "id": 1,
                    "trackName": title,
                    "artistName": artist,
                    "albumName": track.album,
                    "duration": 0,
                    "instrumental": false,
                    "plainLyrics": track.plain,
                    "syncedLyrics": track.synced,
                }))
                .into_response()
            }
        }),
    );
    (router, sim)
}
