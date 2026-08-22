//! LRClib simulator: a seeded catalog behind `GET /api/get` and `GET /api/search`.
//! GET is the first exact (artist, title) match; search is case-insensitive contains.

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
    artist: String,
    title: String,
    album: Option<String>,
    duration: f64,
    synced: Option<String>,
    plain: Option<String>,
}

#[derive(Debug, Default)]
pub struct LrclibSimState {
    catalog: Mutex<Vec<Track>>,
}

impl LrclibSimState {
    pub fn reset(&self) {
        self.catalog.lock().unwrap().clear();
    }

    /// `[{artist, title, album?, duration?, syncedLyrics?, plainLyrics?}]`; lyrics fields nullable.
    pub fn seed(&self, v: Value) -> Result<(), String> {
        let tracks = v.as_array().ok_or("seed: expected an array")?;
        let mut parsed = Vec::with_capacity(tracks.len());
        for t in tracks {
            parsed.push(Track {
                artist: req_str(t, "artist")?,
                title: req_str(t, "title")?,
                album: opt_str(t, "album")?,
                duration: opt_f64(t, "duration")?.unwrap_or(0.0),
                synced: opt_str(t, "syncedLyrics")?,
                plain: opt_str(t, "plainLyrics")?,
            });
        }
        // Append only after the whole batch validates.
        self.catalog.lock().unwrap().extend(parsed);
        Ok(())
    }

    fn lookup(&self, artist: &str, title: &str) -> Option<Track> {
        self.catalog
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.artist == artist && t.title == title)
            .cloned()
    }

    fn search(&self, q: &HashMap<String, String>) -> Vec<Track> {
        let artist_q = q.get("artist_name").map(String::as_str).unwrap_or("");
        let title_q = q.get("track_name").map(String::as_str).unwrap_or("");
        let q_kw = q.get("q").map(String::as_str).unwrap_or("");
        self.catalog
            .lock()
            .unwrap()
            .iter()
            .filter(|t| matches_search(t, artist_q, title_q, q_kw))
            .cloned()
            .collect()
    }
}

fn matches_search(track: &Track, artist_q: &str, title_q: &str, q_kw: &str) -> bool {
    if !q_kw.is_empty() {
        let kw = q_kw.to_lowercase();
        return contains_ci(&track.artist, &kw)
            || contains_ci(&track.title, &kw)
            || track.album.as_deref().is_some_and(|a| contains_ci(a, &kw));
    }
    if artist_q.is_empty() && title_q.is_empty() {
        return false;
    }
    let artist_ok = artist_q.is_empty() || contains_ci(&track.artist, &artist_q.to_lowercase());
    let title_ok = title_q.is_empty() || contains_ci(&track.title, &title_q.to_lowercase());
    artist_ok && title_ok
}

fn contains_ci(haystack: &str, needle_lower: &str) -> bool {
    haystack.to_lowercase().contains(needle_lower)
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

fn opt_f64(t: &Value, key: &str) -> Result<Option<f64>, String> {
    match t.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .or_else(|| v.as_i64().map(|n| n as f64))
            .map(Some)
            .ok_or_else(|| format!("seed: `{key}` must be a number or null")),
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

fn track_json(id: usize, track: &Track) -> Value {
    let duration = if track.duration.fract() == 0.0 {
        json!(track.duration as i64)
    } else {
        json!(track.duration)
    };
    json!({
        "id": id,
        "trackName": track.title,
        "artistName": track.artist,
        "albumName": track.album,
        "duration": duration,
        "instrumental": false,
        "plainLyrics": track.plain,
        "syncedLyrics": track.synced,
    })
}

pub fn lrclib_router() -> (Router, Arc<LrclibSimState>) {
    let sim = Arc::new(LrclibSimState::default());
    let get_sim = sim.clone();
    let search_sim = sim.clone();
    let router = Router::new()
        .route(
            "/api/get",
            get(move |Query(q): Query<HashMap<String, String>>| {
                let sim = get_sim.clone();
                async move {
                    let artist = q.get("artist_name").map(String::as_str).unwrap_or_default();
                    let title = q.get("track_name").map(String::as_str).unwrap_or_default();
                    let Some(track) = sim.lookup(artist, title) else {
                        return not_found();
                    };
                    Json(track_json(1, &track)).into_response()
                }
            }),
        )
        .route(
            "/api/search",
            get(move |Query(q): Query<HashMap<String, String>>| {
                let sim = search_sim.clone();
                async move {
                    let hits: Vec<Value> = sim
                        .search(&q)
                        .iter()
                        .enumerate()
                        .map(|(i, t)| track_json(i + 1, t))
                        .collect();
                    Json(hits).into_response()
                }
            }),
        );
    (router, sim)
}
