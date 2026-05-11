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
        Lyrics, LyricsTranslation, cache::LyricsCache, lrclib,
        translator::get_lyrics_translator,
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
    watcher: SpotifyWatcher,
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct TranslationKey {
    track_id: String,
    tgt: String,
    model: usize,
}

impl LyricsState {
    pub fn new() -> Self {
        Self {
            cache: tokio::sync::OnceCell::new(),
            lyrics: RwLock::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            next_request: AtomicUsize::new(0),

            #[cfg(target_os = "macos")]
            watcher: SpotifyWatcher::new(),
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

#[derive(Clone, Serialize)]
pub struct LyricsTranslationProgress {
    #[serde(rename = "requestId")]
    pub request_id: usize,
    pub bytes: usize,
}

#[derive(Clone, Serialize)]
pub struct LyricsTranslationDone {
    #[serde(rename = "requestId")]
    pub request_id: usize,
    pub translation: LyricsTranslation,
}

#[derive(Clone, Serialize)]
pub struct LyricsTranslationError {
    #[serde(rename = "requestId")]
    pub request_id: usize,
    pub error: String,
}

// ----- Tauri commands ----------------------------------------------------

#[tauri::command]
pub async fn start_spotify_watcher(
    #[allow(unused_variables)] app: AppHandle,
    #[allow(unused_variables)] state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        state.lyrics_state.watcher.start(app).await;
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
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn get_now_playing(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Option<NowPlaying>, String> {
    // If watcher has cached state, return it; otherwise do a one-shot query.
    if let Some(np) = state.lyrics_state.watcher.current().await {
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

#[tauri::command]
pub async fn get_lyrics(
    state: tauri::State<'_, Arc<AppState>>,
    track_id: String,
    artist: String,
    title: String,
    album: Option<String>,
    duration_s: Option<u32>,
) -> Result<Option<Lyrics>, String> {
    if let Some(existing) = state.lyrics_state.lyrics.read().await.get(&track_id) {
        return Ok(Some((**existing).clone()));
    }

    let fetched = lrclib::fetch(&track_id, &artist, &title, album.as_deref(), duration_s)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(l) = fetched.clone() {
        state
            .lyrics_state
            .lyrics
            .write()
            .await
            .insert(track_id.clone(), Arc::new(l));
    }
    Ok(fetched)
}

#[tauri::command]
pub async fn translate_lyrics(
    state: tauri::State<'_, Arc<AppState>>,
    app: AppHandle,
    track_id: String,
    target_lang: String,
    model: usize,
) -> Result<usize, String> {
    let tgt = Language::from_639_3(&target_lang)
        .ok_or_else(|| format!("unknown target lang: {target_lang}"))?;
    let model_enum = TranslationModel::from(model);
    if matches!(model_enum, TranslationModel::Unknown) {
        return Err(format!("unknown model id: {model}"));
    }

    let key = TranslationKey {
        track_id: track_id.clone(),
        tgt: tgt.to_639_3().to_string(),
        model,
    };

    // Cache hit → emit done immediately and return new request id.
    let cache = state.lyrics_state.lyrics_cache().await.map_err(|e| e.to_string())?;
    if let Some(cached) = cache.get(&track_id, &tgt, model).await {
        let request_id = state
            .lyrics_state
            .next_request
            .fetch_add(1, Ordering::SeqCst);
        let _ = app.emit(
            "lyrics_translation_done",
            LyricsTranslationDone {
                request_id,
                translation: cached,
            },
        );
        return Ok(request_id);
    }

    // Dedup in-flight request for the same key.
    {
        let mut inflight = state.lyrics_state.inflight.lock().await;
        if let Some(&existing) = inflight.get(&key) {
            return Ok(existing);
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
            .get(&track_id)
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
        let cfg: Config = state.config.read().await.clone();
        let api_key = match provider {
            library::translator::TranslationProvider::Google => cfg.gemini_api_key,
            library::translator::TranslationProvider::Openai => cfg.openai_api_key,
        }
        .ok_or_else(|| "no API key configured for selected provider".to_string())?;

        let app_clone = app.clone();
        let state_arc: Arc<AppState> = state.inner().clone();
        tokio::spawn(async move {
            run_translation(
                state_arc,
                app_clone,
                request_id,
                key,
                track_id,
                lyrics,
                tgt,
                model,
                model_enum,
                provider,
                api_key,
            )
            .await;
        });

        Ok(request_id)
    }
}

#[tauri::command]
pub async fn get_lyrics_translation(
    state: tauri::State<'_, Arc<AppState>>,
    track_id: String,
    target_lang: String,
    model: usize,
) -> Result<Option<LyricsTranslation>, String> {
    let tgt = Language::from_639_3(&target_lang)
        .ok_or_else(|| format!("unknown target lang: {target_lang}"))?;
    let cache = state.lyrics_state.lyrics_cache().await.map_err(|e| e.to_string())?;
    Ok(cache.get(&track_id, &tgt, model).await)
}

#[allow(clippy::too_many_arguments)]
async fn run_translation(
    state: Arc<AppState>,
    app: AppHandle,
    request_id: usize,
    key: TranslationKey,
    track_id: String,
    lyrics: Arc<Lyrics>,
    tgt: Language,
    model: usize,
    model_enum: TranslationModel,
    provider: library::translator::TranslationProvider,
    api_key: String,
) {
    let result = async {
        let translator = get_lyrics_translator(provider, model_enum, api_key, tgt)?;

        let progress_app = app.clone();
        let progress_state = Arc::new(std::sync::Mutex::new((Instant::now(), 0usize)));
        let callback: Box<dyn Fn(usize) + Send + Sync> = Box::new(move |bytes| {
            let mut s = progress_state.lock().unwrap();
            if s.1 == bytes {
                return;
            }
            if s.0.elapsed() < PROGRESS_THROTTLE {
                return;
            }
            *s = (Instant::now(), bytes);
            drop(s);
            let _ = progress_app.emit(
                "lyrics_translation_progress",
                LyricsTranslationProgress { request_id, bytes },
            );
        });

        let lines = translator
            .translate_song(&lyrics.lines, Some(callback))
            .await?;

        let translation = LyricsTranslation {
            track_id: track_id.clone(),
            target_lang: tgt,
            model,
            lines,
        };

        // Persist to disk cache.
        let cache = state.lyrics_state.lyrics_cache().await?;
        cache.put(&translation).await?;

        Ok::<_, anyhow::Error>(translation)
    }
    .await;

    state.lyrics_state.inflight.lock().await.remove(&key);

    match result {
        Ok(translation) => {
            info!(
                "Lyrics translation done: track={} lines={} req={}",
                track_id,
                translation.lines.len(),
                request_id
            );
            let _ = app.emit(
                "lyrics_translation_done",
                LyricsTranslationDone {
                    request_id,
                    translation,
                },
            );
        }
        Err(err) => {
            warn!("Lyrics translation failed for track={track_id}: {err}");
            let _ = app.emit(
                "lyrics_translation_error",
                LyricsTranslationError {
                    request_id,
                    error: err.to_string(),
                },
            );
        }
    }
}
