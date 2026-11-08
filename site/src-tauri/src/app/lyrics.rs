#[cfg(target_os = "macos")]
use crate::app::spotify::applescript::{NowPlaying, SpotifyWatcher};
use crate::app::{AppState, config::Config, spotify::web::TrackMeta, task::ManagedTask};
use futures_util::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use isolang::Language;
use library::{
    cache::WeakLruCache,
    lyrics::{
        Lyrics, LyricsTranslation, cache::LyricsCache, lrclib, translation::get_lyrics_translator,
    },
    translator::TranslationProvider,
};
use log::{info, warn};
use serde::Serialize;
use std::panic::AssertUnwindSafe;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot, watch};
const LYRICS_MEM_CACHE_CAPACITY: usize = 64;
const PROGRESS_THROTTLE: Duration = Duration::from_millis(400);
#[derive(Clone, Hash, PartialEq, Eq)]
struct TranslationKey {
    track_id: String,
    tgt: Language,
    model: String,
    provider: TranslationProvider,
}
type Reply<T> = oneshot::Sender<Result<T, String>>;
enum Request {
    Fetch(TrackMeta, usize, Reply<Option<Lyrics>>),
    Snapshot(String, Language, String, Reply<TrackLyricsState>),
    Translate {
        key: TranslationKey,
        api_key: Option<String>,
        generation: usize,
        reply: Reply<()>,
    },
    Cancel(oneshot::Sender<()>),
    Configure(Config, oneshot::Sender<()>),
}
enum Finished {
    Fetch(Reply<Option<Lyrics>>, anyhow::Result<Option<Lyrics>>),
    Snapshot(Reply<TrackLyricsState>, TrackLyricsState),
    Translation(usize, TranslationKey, anyhow::Result<LyricsTranslation>),
}
#[derive(Clone)]
pub struct LyricsState {
    requests: mpsc::Sender<Request>,
    generation: watch::Receiver<usize>,
    configuration: watch::Receiver<Option<(Config, usize)>>,
    control: library::task::TaskControl,
    #[cfg(target_os = "macos")]
    pub watcher: SpotifyWatcher,
}
impl LyricsState {
    pub fn new(app: AppHandle) -> anyhow::Result<(Self, ManagedTask)> {
        let cache = LyricsCache::new(&super::resolve_cache_dir(Some(&app))?);
        let (requests, receiver) = mpsc::channel(256);
        let (generation, observed) = watch::channel(0);
        let (configuration, configured) = watch::channel(None);
        #[cfg(target_os = "macos")]
        let (watcher, watcher_owner) = SpotifyWatcher::new();
        let owner = ManagedTask::spawn(move |stop| async move {
            serve(cache, app, receiver, generation, configuration, stop).await;
            #[cfg(target_os = "macos")]
            let _ = watcher_owner.shutdown().await;
        });
        Ok((
            Self {
                requests,
                generation: observed,
                configuration: configured,
                control: owner.control(),
                #[cfg(target_os = "macos")]
                watcher,
            },
            owner,
        ))
    }
    pub(super) fn generation(&self) -> usize {
        *self.generation.borrow()
    }
    pub(crate) fn configuration(&self) -> Option<(Config, usize)> {
        self.configuration.borrow().clone()
    }
    pub(crate) async fn configure(&self, config: Config) {
        let (reply, result) = oneshot::channel();
        if self
            .requests
            .send(Request::Configure(config, reply))
            .await
            .is_ok()
        {
            let _ = result.await;
        }
    }
    pub async fn cancel_all(&self) {
        let (reply, result) = oneshot::channel();
        if self.requests.send(Request::Cancel(reply)).await.is_ok() {
            let _ = result.await;
        }
    }
    pub async fn shutdown(&self) {
        let _ = self.control.shutdown().await;
    }
    async fn fetch(&self, track: &TrackMeta, generation: usize) -> anyhow::Result<Option<Lyrics>> {
        let (reply, result) = oneshot::channel();
        self.requests
            .send(Request::Fetch(track.clone(), generation, reply))
            .await
            .map_err(|_| anyhow::anyhow!("lyrics service closed"))?;
        result.await?.map_err(anyhow::Error::msg)
    }
    async fn snapshot(
        &self,
        id: String,
        tgt: Language,
        model: String,
    ) -> Result<TrackLyricsState, String> {
        let (reply, result) = oneshot::channel();
        self.requests
            .send(Request::Snapshot(id, tgt, model, reply))
            .await
            .map_err(|_| "lyrics service closed".to_string())?;
        result
            .await
            .map_err(|_| "lyrics service closed".to_string())?
    }
    async fn translate(
        &self,
        cfg: &Config,
        generation: usize,
        id: &str,
        target: &str,
        model: String,
    ) -> Result<(), String> {
        let tgt =
            Language::from_639_3(target).ok_or_else(|| format!("unknown target lang: {target}"))?;
        let key = TranslationKey {
            track_id: id.into(),
            tgt,
            model: cfg.resolved_model_id(&model),
            provider: cfg.translation_provider,
        };
        let api_key = cfg.api_keys().for_provider(key.provider).map(str::to_owned);
        let (reply, result) = oneshot::channel();
        self.requests
            .send(Request::Translate {
                key,
                api_key,
                generation,
                reply,
            })
            .await
            .map_err(|_| "lyrics service closed".to_string())?;
        result
            .await
            .map_err(|_| "lyrics service closed".to_string())?
    }
    pub(crate) async fn resolve(
        &self,
        cfg: &Config,
        generation: usize,
        app: &AppHandle,
        track: &TrackMeta,
        target: &str,
        model: String,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(self.generation() == generation, "lyrics session changed");
        let lyrics = self.fetch(track, generation).await?;
        if self.generation() != generation {
            return Ok(());
        }
        let _ = app.emit(
            "lyrics_resolved",
            LyricsResolved {
                track_id: track.id.clone(),
                lyrics: lyrics.clone(),
            },
        );
        if lyrics.is_some() {
            self.translate(cfg, generation, &track.id, target, model)
                .await
                .map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }
}
async fn serve(
    cache: LyricsCache,
    app: AppHandle,
    mut requests: mpsc::Receiver<Request>,
    generation: watch::Sender<usize>,
    configuration: watch::Sender<Option<(Config, usize)>>,
    mut stop: watch::Receiver<bool>,
) {
    let memory = WeakLruCache::new(LYRICS_MEM_CACHE_CAPACITY);
    let mut jobs: FuturesUnordered<BoxFuture<'_, Finished>> = FuturesUnordered::new();
    let mut inflight: HashMap<TranslationKey, usize> = HashMap::new();
    let mut next_request = 0usize;
    loop {
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            request = requests.recv() => match request {
                None => break,
                Some(Request::Cancel(reply)) => { configuration.send_replace(None); jobs.clear(); inflight.clear(); generation.send_modify(|g| *g = g.wrapping_add(1)); let _ = reply.send(()); }
                Some(Request::Configure(config, reply)) => { configuration.send_replace(Some((config, *generation.borrow()))); let _ = reply.send(()); }
                Some(Request::Fetch(track, captured, reply)) => {
                    if captured != *generation.borrow() || configuration.borrow().is_none() { let _ = reply.send(Err("lyrics session changed".into())); continue; }
                    if jobs.len() >= 256 { let _ = reply.send(Err("lyrics queue busy".into())); continue; }
                    let cache = &cache; let memory = &memory;
                    jobs.push(async move { Finished::Fetch(reply, AssertUnwindSafe(fetch_lyrics(memory, cache, &track)).catch_unwind().await.unwrap_or_else(|_| Err(anyhow::anyhow!("lyrics fetch panicked")))) }.boxed());
                }
                Some(Request::Snapshot(id, tgt, model, reply)) => {
                    let cache = &cache; let memory = &memory;
                    jobs.push(async move {
                        let lyrics = match memory.get(&id).await { Some(lyrics) => Some((*lyrics).clone()), None => cache.get_raw(&id).await };
                        Finished::Snapshot(reply, TrackLyricsState { lyrics, translation: cache.get(&id, &tgt, &model).await })
                    }.boxed());
                }
                Some(Request::Translate { key, api_key, generation: captured, reply }) => {
                    if captured != *generation.borrow() || configuration.borrow().is_none() { let _ = reply.send(Err("lyrics session changed".into())); continue; }
                    if inflight.contains_key(&key) { let _ = reply.send(Ok(())); continue; }
                    if let Some(translation) = cache.get(&key.track_id, &key.tgt, &key.model).await {
                        let _ = app.emit("lyrics_translation_done", LyricsTranslationDone { track_id: key.track_id, translation });
                        let _ = reply.send(Ok(())); continue;
                    }
                    let Some(lyrics) = memory.get(&key.track_id).await else { let _ = reply.send(Err(format!("lyrics not loaded for track_id={} — call get_lyrics first", key.track_id))); continue; };
                    let Some(api_key) = api_key else { let _ = reply.send(Err("no API key configured for selected provider".into())); continue; };
                    if jobs.len() >= 256 { let _ = reply.send(Err("lyrics queue busy".into())); continue; }
                    let id = next_request; next_request += 1; inflight.insert(key.clone(), id);
                    let cache = &cache; let app = &app;
                    jobs.push(async move {
                        let result = AssertUnwindSafe(async {
                            let throttle = std::sync::Mutex::new((Instant::now(), 0usize));
                            let progress = |bytes| {
                                let mut last = throttle.lock().unwrap();
                                if last.1 == bytes || last.0.elapsed() < PROGRESS_THROTTLE { return; }
                                *last = (Instant::now(), bytes);
                                let _ = app.emit("lyrics_translation_progress", LyricsTranslationProgress { track_id: key.track_id.clone(), bytes });
                            };
                            run_translation(&key, &lyrics, api_key, cache, Box::new(progress)).await
                        }).catch_unwind().await.unwrap_or_else(|_| Err(anyhow::anyhow!("lyrics translation panicked")));
                        Finished::Translation(id, key, result)
                    }.boxed());
                    let _ = reply.send(Ok(()));
                }
            },
            Some(finished) = jobs.next(), if !jobs.is_empty() => match finished {
                Finished::Fetch(reply, result) => { let _ = reply.send(result.map_err(|e| e.to_string())); }
                Finished::Snapshot(reply, snapshot) => { let _ = reply.send(Ok(snapshot)); }
                Finished::Translation(id, key, result) => {
                    if inflight.get(&key) != Some(&id) { continue; } inflight.remove(&key);
                    match result {
                        Ok(translation) => { let _ = app.emit("lyrics_translation_done", LyricsTranslationDone { track_id: key.track_id, translation }); }
                        Err(err) => { let _ = app.emit("lyrics_translation_error", LyricsTranslationError { track_id: key.track_id, error: err.to_string() }); }
                    }
                }
            }
        }
    }
    requests.close();
    jobs.clear();
    generation.send_modify(|g| *g = g.wrapping_add(1));
}
/// Translation events carry only `trackId`, which the frontend matches against
/// what it displays; request ids stay internal so cache vs. network is opaque.
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

/// Resolver verdict for a track: lyrics fetched, or `None` for "LRClib has
/// none".
#[derive(Clone, Serialize)]
pub struct LyricsResolved {
    #[serde(rename = "trackId")]
    pub track_id: String,
    pub lyrics: Option<Lyrics>,
}

/// Bootstrap snapshot for mount / track change; later updates arrive as
/// `lyrics_resolved` and `lyrics_translation_done` events.
#[derive(Clone, Serialize)]
pub struct TrackLyricsState {
    pub lyrics: Option<Lyrics>,
    pub translation: Option<LyricsTranslation>,
}

#[tauri::command]
pub async fn start_spotify_watcher(
    #[allow(unused_variables)] app: AppHandle,
    #[allow(unused_variables)] state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        state.lyrics_state.watcher.start(app.clone()).await;
        // The resolver lives as long as the watcher: without Spotify Web auth it
        // still serves the current track from AppleScript, only the queue fetch
        // no-ops.
        state.spotify_web.start_polling().await;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Spotify lyrics mode is macOS only".to_string())
    }
}

#[tauri::command]
pub async fn stop_spotify_watcher(
    #[allow(unused_variables)] state: tauri::State<'_, AppState>,
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
    state: tauri::State<'_, AppState>,
) -> Result<Option<NowPlaying>, String> {
    if let Some(np) = state.lyrics_state.watcher.current() {
        return Ok(Some(np));
    }
    match crate::app::spotify::applescript::query_once().await {
        Ok(np) => Ok(Some(np)),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub async fn get_now_playing(
    _state: tauri::State<'_, AppState>,
) -> Result<Option<serde_json::Value>, String> {
    Ok(None)
}

async fn fetch_lyrics(
    memory: &WeakLruCache<String, Lyrics>,
    cache: &LyricsCache,
    track: &TrackMeta,
) -> anyhow::Result<Option<Lyrics>> {
    let track_id = track.id.as_str();
    if let Some(existing) = memory.get(&track_id.to_string()).await {
        return Ok(Some((*existing).clone()));
    }

    let disk = cache.get_raw(track_id).await;
    if let Some(cached) = &disk
        && cached.synced
    {
        memory
            .insert(track_id.to_string(), Arc::new(cached.clone()))
            .await;
        return Ok(Some(cached.clone()));
    }

    let duration_s = (track.duration_ms + 500) / 1000;
    let mut fetched = lrclib::fetch(
        track_id,
        &track.artist,
        &track.name,
        track.album.as_deref(),
        Some(duration_s),
    )
    .await?;

    // Spotify's own lyrics are the second opinion: tried only when LRClib has
    // nothing, so LRClib-first behavior (and its tests) are unchanged. The
    // fetch runs inside the running Spotify desktop app's webview over its
    // DevTools bridge (`spotify::cdp`), so it needs no credentials of our own
    // and is invisible to Spotify's bot defenses. `Ok(None)` covers
    // "bridge off" and "Spotify has no lyrics for this track".
    #[cfg(target_os = "macos")]
    if fetched.is_none() {
        match crate::app::spotify::cdp::lyrics_for_track(track_id).await {
            Ok(Some(l)) => {
                info!(
                    "Spotify lyrics fallback hit for {track_id} (synced={})",
                    l.synced
                );
                fetched = Some(l);
            }
            Ok(None) => {}
            Err(err) => warn!("Spotify lyrics fallback failed for {track_id}: {err}"),
        }
    }

    // Synced network wins (including an upgrade over unsynced disk). Unsynced
    // disk is kept when the network is no better, but still enters memory so
    // this session does not search again. Misses stay uncached.
    let (chosen, persist) = match (disk, fetched) {
        (_, Some(l)) if l.synced => (Some(l), true),
        (Some(cached), _) => (Some(cached), false),
        (None, Some(l)) => (Some(l), true),
        (None, None) => (None, false),
    };

    if let Some(l) = &chosen {
        memory
            .insert(track_id.to_string(), Arc::new(l.clone()))
            .await;
        if persist && let Err(err) = cache.put_raw(l).await {
            warn!("LyricsCache: failed to persist raw lyrics for {track_id}: {err}");
        }
    }
    Ok(chosen)
}
#[tauri::command]
pub async fn get_track_lyrics_state(
    state: tauri::State<'_, AppState>,
    track_id: String,
    target_lang: String,
    model: String,
) -> Result<TrackLyricsState, String> {
    let tgt = Language::from_639_3(&target_lang)
        .ok_or_else(|| format!("unknown target lang: {target_lang}"))?;
    let model = state.config.borrow().resolved_model_id(&model);
    state.lyrics_state.snapshot(track_id, tgt, model).await
}
#[cfg(feature = "e2e-bridge")]
pub(crate) async fn resolve_track(
    state: &AppState,
    app: &AppHandle,
    track: &TrackMeta,
    target_lang: &str,
    model: String,
) -> anyhow::Result<()> {
    let (cfg, generation) = state.configuration().await?;
    state
        .lyrics_state
        .resolve(&cfg, generation, app, track, target_lang, model)
        .await
}
async fn run_translation<'a>(
    key: &TranslationKey,
    lyrics: &Lyrics,
    api_key: String,
    cache: &LyricsCache,
    progress: Box<library::translator::ProgressCallback<'a>>,
) -> anyhow::Result<LyricsTranslation> {
    let translator = get_lyrics_translator(key.provider, &key.model, api_key, key.tgt)?;
    let lines = translator
        .translate_song(&lyrics.lines, Some(progress))
        .await?;
    let translation = LyricsTranslation {
        track_id: key.track_id.clone(),
        target_lang: key.tgt,
        model: key.model.clone(),
        lines,
    };
    cache.put(&translation).await?;
    Ok(translation)
}
