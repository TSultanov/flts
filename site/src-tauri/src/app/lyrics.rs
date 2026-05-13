use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use directories::ProjectDirs;
use isolang::Language;
use library::{
    lyrics::{
        Lyrics, LyricsTranslation, cache::LyricsCache, lrclib, translator::get_lyrics_translator,
    },
    translator::TranslationModel,
};
use log::{info, warn};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, RwLock};

use crate::app::{AppError, AppState, config::Config};

#[cfg(target_os = "macos")]
use crate::app::spotify::{NowPlaying, SpotifyWatcher};

const PROGRESS_THROTTLE: Duration = Duration::from_millis(400);

pub struct LyricsState {
    cache: tokio::sync::OnceCell<Arc<LyricsCache>>,
    lyrics: RwLock<HashMap<String, Arc<Lyrics>>>,
    /// (track_id, src, tgt, model) → in-flight request id (for dedup).
    inflight: Mutex<HashMap<TranslationKey, usize>>,
    next_request: AtomicUsize,

    #[cfg(target_os = "macos")]
    pub watcher: Arc<SpotifyWatcher>,
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct TranslationKey {
    track_id: String,
    tgt: Language,
    model: TranslationModel,
}

impl Default for LyricsState {
    fn default() -> Self {
        Self::new()
    }
}

impl LyricsState {
    pub fn new() -> Self {
        Self {
            cache: tokio::sync::OnceCell::new(),
            lyrics: RwLock::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            next_request: AtomicUsize::new(0),

            #[cfg(target_os = "macos")]
            watcher: Arc::new(SpotifyWatcher::new()),
        }
    }

    async fn lyrics_cache(&self) -> anyhow::Result<Arc<LyricsCache>> {
        self.cache
            .get_or_try_init(|| async {
                let dirs = ProjectDirs::from("", "TS", "FLTS").ok_or(AppError::ProjectDirsError)?;
                Ok::<_, anyhow::Error>(Arc::new(LyricsCache::new(dirs.cache_dir())))
            })
            .await
            .cloned()
    }
}

/// Translation events carry `trackId` so the frontend can match them against
/// whatever it's currently displaying. We don't expose request_ids: the
/// frontend doesn't need to know whether a given resolution came from cache,
/// from a fresh in-flight task, or from work the backend started on its own.
#[derive(Clone, Serialize)]
pub struct LyricsTranslationProgress {
    #[serde(rename = "trackId")]
    pub track_id: String,
    pub bytes: usize,
}

#[derive(Clone, Serialize)]
pub struct LyricsTranslationDone {
    #[serde(rename = "trackId")]
    pub track_id: String,
    pub translation: LyricsTranslation,
}

#[derive(Clone, Serialize)]
pub struct LyricsTranslationError {
    #[serde(rename = "trackId")]
    pub track_id: String,
    pub error: String,
}

/// Fired by the backend resolver after deciding what the track's lyrics
/// situation is — either fetched lyrics or "LRClib has no lyrics for this
/// track". Frontend filters by track_id and updates its view.
#[derive(Clone, Serialize)]
pub struct LyricsResolved {
    #[serde(rename = "trackId")]
    pub track_id: String,
    pub lyrics: Option<Lyrics>,
}

/// Read-only snapshot of what the backend currently has cached for a track.
/// Used by the frontend at mount / track-change time to bootstrap; the same
/// shape is otherwise delivered asynchronously through `lyrics_resolved` and
/// `lyrics_translation_done` events.
#[derive(Clone, Serialize)]
pub struct TrackLyricsState {
    pub lyrics: Option<Lyrics>,
    pub translation: Option<LyricsTranslation>,
}

// ----- Tauri commands ----------------------------------------------------

#[tauri::command]
pub async fn start_spotify_watcher(
    #[allow(unused_variables)] app: AppHandle,
    #[allow(unused_variables)] state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        state.lyrics_state.watcher.start(app.clone()).await;
        // The resolver runs as long as the watcher does — it owns "resolve
        // current track and any known next tracks" regardless of whether
        // Spotify Web is connected. When Spotify Web isn't connected, the
        // loop still services the current track from the AppleScript signal;
        // the queue fetch just becomes a no-op until auth comes online.
        let watcher = state.lyrics_state.watcher.clone();
        let app_state = state.inner().clone();
        state
            .spotify_web
            .start_polling(app, watcher, app_state)
            .await;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Spotify lyrics mode is macOS only".to_string())
    }
}

#[tauri::command]
pub async fn stop_spotify_watcher(
    #[allow(unused_variables)] state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        state.lyrics_state.watcher.stop().await;
        state.spotify_web.stop_polling().await;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn get_now_playing(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Option<NowPlaying>, String> {
    // If watcher has cached state, return it; otherwise do a one-shot query.
    if let Some(np) = state.lyrics_state.watcher.current() {
        return Ok(Some(np));
    }
    match crate::app::spotify::query_once().await {
        Ok(np) => Ok(Some(np)),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub async fn get_now_playing(
    _state: tauri::State<'_, Arc<AppState>>,
) -> Result<Option<serde_json::Value>, String> {
    Ok(None)
}

pub(crate) async fn fetch_lyrics_inner(
    state: &Arc<AppState>,
    track_id: &str,
    artist: &str,
    title: &str,
    album: Option<&str>,
    duration_s: Option<u32>,
) -> anyhow::Result<Option<Lyrics>> {
    // In-memory cache: hottest path, session-local.
    if let Some(existing) = state.lyrics_state.lyrics.read().await.get(track_id) {
        return Ok(Some((**existing).clone()));
    }

    // Disk cache: skip the LRClib round-trip on songs we've fetched before.
    let cache = state.lyrics_state.lyrics_cache().await?;
    if let Some(cached) = cache.get_raw(track_id).await {
        state
            .lyrics_state
            .lyrics
            .write()
            .await
            .insert(track_id.to_string(), Arc::new(cached.clone()));
        return Ok(Some(cached));
    }

    let fetched = lrclib::fetch(track_id, artist, title, album, duration_s).await?;

    if let Some(l) = &fetched {
        state
            .lyrics_state
            .lyrics
            .write()
            .await
            .insert(track_id.to_string(), Arc::new(l.clone()));
        // Cache-write failure must not fail the user's request — log and continue.
        if let Err(err) = cache.put_raw(l).await {
            warn!("LyricsCache: failed to persist raw lyrics for {track_id}: {err}");
        }
    }
    Ok(fetched)
}

/// Read-only snapshot of what the backend has cached for `track_id`. Pure
/// data fetch — never triggers an LRClib request or a translation. The
/// frontend uses this at mount / track-change time to render whatever the
/// resolver has already produced; further updates arrive via the
/// `lyrics_resolved` and `lyrics_translation_done` events.
#[tauri::command]
pub async fn get_track_lyrics_state(
    state: tauri::State<'_, Arc<AppState>>,
    track_id: String,
    target_lang: String,
    model: usize,
) -> Result<TrackLyricsState, String> {
    let tgt = Language::from_639_3(&target_lang)
        .ok_or_else(|| format!("unknown target lang: {target_lang}"))?;
    let cache = state
        .lyrics_state
        .lyrics_cache()
        .await
        .map_err(|e| e.to_string())?;

    let lyrics = if let Some(arc) = state.lyrics_state.lyrics.read().await.get(&track_id) {
        Some((**arc).clone())
    } else {
        cache.get_raw(&track_id).await
    };
    let translation = cache.get(&track_id, &tgt, model).await;

    Ok(TrackLyricsState {
        lyrics,
        translation,
    })
}

pub(crate) async fn dispatch_translation_inner(
    state: &Arc<AppState>,
    app: &AppHandle,
    track_id: &str,
    target_lang: &str,
    model: usize,
) -> Result<(), String> {
    let tgt = Language::from_639_3(target_lang)
        .ok_or_else(|| format!("unknown target lang: {target_lang}"))?;
    let model_enum = TranslationModel::from(model);
    if matches!(model_enum, TranslationModel::Unknown) {
        return Err(format!("unknown model id: {model}"));
    }

    let key = TranslationKey {
        track_id: track_id.to_string(),
        tgt,
        model: model_enum,
    };

    // Cache hit → emit the same `lyrics_translation_done` event a fresh
    // translation would emit, so the frontend has a single code path that
    // reacts to events keyed on track_id. The previous "return cached inline"
    // shape leaked the cache distinction into the API; the frontend doesn't
    // need to know whether the data came from disk or the network.
    let cache = state
        .lyrics_state
        .lyrics_cache()
        .await
        .map_err(|e| e.to_string())?;
    if let Some(cached) = cache.get(track_id, &tgt, model).await {
        let _ = app.emit(
            "lyrics_translation_done",
            LyricsTranslationDone {
                track_id: track_id.to_string(),
                translation: cached,
            },
        );
        return Ok(());
    }

    // Dedup in-flight request for the same key. If we're already translating
    // this exact track/lang/model, the existing task will emit its event when
    // done; the frontend listener picks it up via track_id match.
    let mut inflight = state.lyrics_state.inflight.lock().await;
    if inflight.contains_key(&key) {
        return Ok(());
    }
    let request_id = state
        .lyrics_state
        .next_request
        .fetch_add(1, Ordering::SeqCst);
    inflight.insert(key.clone(), request_id);
    drop(inflight);

    // Get the lyrics for this track from in-memory cache; bail if not fetched yet.
    let lyrics = state
        .lyrics_state
        .lyrics
        .read()
        .await
        .get(track_id)
        .cloned();
    let lyrics = match lyrics {
        Some(l) => l,
        None => {
            state.lyrics_state.inflight.lock().await.remove(&key);
            return Err(format!(
                "lyrics not loaded for track_id={track_id} — call get_lyrics first"
            ));
        }
    };

    let provider = model_enum
        .provider()
        .ok_or_else(|| "unknown model provider".to_string())?;
    let cfg: Config = state.config.borrow().clone();
    let api_key = match provider {
        library::translator::TranslationProvider::Google => cfg.gemini_api_key,
        library::translator::TranslationProvider::Openai => cfg.openai_api_key,
    }
    .ok_or_else(|| "no API key configured for selected provider".to_string())?;

    let app_for_progress = app.clone();
    let app_for_result = app.clone();
    let state_arc: Arc<AppState> = state.clone();
    let track_id_progress = track_id.to_string();
    let progress: Box<dyn Fn(usize) + Send + Sync> = {
        let throttle = Arc::new(std::sync::Mutex::new((Instant::now(), 0usize)));
        Box::new(move |bytes: usize| {
            let mut s = throttle.lock().unwrap();
            if s.1 == bytes || s.0.elapsed() < PROGRESS_THROTTLE {
                return;
            }
            *s = (Instant::now(), bytes);
            drop(s);
            let _ = app_for_progress.emit(
                "lyrics_translation_progress",
                LyricsTranslationProgress {
                    track_id: track_id_progress.clone(),
                    bytes,
                },
            );
        })
    };

    let track_id_for_task = track_id.to_string();
    tokio::spawn(async move {
        let result = run_translation(&key, lyrics, api_key, cache, progress).await;
        state_arc.lyrics_state.inflight.lock().await.remove(&key);
        match result {
            Ok(translation) => {
                info!(
                    "Lyrics translation done: track={} lines={}",
                    key.track_id,
                    translation.lines.len(),
                );
                let _ = app_for_result.emit(
                    "lyrics_translation_done",
                    LyricsTranslationDone {
                        track_id: track_id_for_task,
                        translation,
                    },
                );
            }
            Err(err) => {
                warn!(
                    "Lyrics translation failed for track={}: {err}",
                    key.track_id
                );
                let _ = app_for_result.emit(
                    "lyrics_translation_error",
                    LyricsTranslationError {
                        track_id: track_id_for_task,
                        error: err.to_string(),
                    },
                );
            }
        }
    });

    Ok(())
}

/// Resolve one track from end to end: fetch lyrics, then translation if any.
/// Emits `lyrics_resolved` with the final lyrics value (Some or None) and,
/// when lyrics exist, `lyrics_translation_done` once translation completes.
/// The frontend never calls this — it's the single entry point the backend
/// resolver uses for every track in the playback list.
pub(crate) async fn resolve_track(
    state: &Arc<AppState>,
    app: &AppHandle,
    track_id: &str,
    artist: &str,
    title: &str,
    album: Option<&str>,
    duration_ms: Option<u32>,
    target_lang: &str,
    model: usize,
) -> anyhow::Result<()> {
    let duration_s = duration_ms.map(|ms| (ms + 500) / 1000);
    let lyrics = fetch_lyrics_inner(state, track_id, artist, title, album, duration_s).await?;

    // Tell the frontend our determination either way: lyrics found, or
    // confirmed absent on LRClib. Either is information the UI uses.
    let _ = app.emit(
        "lyrics_resolved",
        LyricsResolved {
            track_id: track_id.to_string(),
            lyrics: lyrics.clone(),
        },
    );

    if lyrics.is_none() {
        info!("Resolve: no lyrics on LRClib for {title} — {artist} (track_id={track_id})");
        return Ok(());
    }
    info!("Resolve: lyrics fetched, dispatching translation for {title} — {artist}");
    if let Err(err) =
        dispatch_translation_inner(state, app, track_id, target_lang, model).await
    {
        anyhow::bail!("translation dispatch failed: {err}");
    }
    Ok(())
}


async fn run_translation(
    key: &TranslationKey,
    lyrics: Arc<Lyrics>,
    api_key: String,
    cache: Arc<LyricsCache>,
    progress: Box<dyn Fn(usize) + Send + Sync>,
) -> anyhow::Result<LyricsTranslation> {
    let provider = key
        .model
        .provider()
        .expect("provider validated by translate_lyrics");
    let translator = get_lyrics_translator(provider, key.model, api_key, key.tgt)?;

    let lines = translator
        .translate_song(&lyrics.lines, Some(progress))
        .await?;

    let translation = LyricsTranslation {
        track_id: key.track_id.clone(),
        target_lang: key.tgt,
        model: key.model as usize,
        lines,
    };

    cache.put(&translation).await?;

    Ok(translation)
}
