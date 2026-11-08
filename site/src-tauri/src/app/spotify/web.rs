//! Optional Spotify Web API enrichment layer.
//!
//! Adds a queue lookahead source on top of the AppleScript-based current-track
//! watcher (`crate::app::spotify`). Used to preload lyrics+translation for the
//! next track in a playlist or album, and to display "Up next" in the UI.
//!
//! All Web API access is gated behind a user-initiated PKCE OAuth flow. Refresh
//! tokens are persisted to the OS keychain via `keyring`. If the user never
//! connects, this module is dormant and the rest of the app behaves as before.
//!
//! Polling cadence: when AppleScript reports `Playing`, we poll
//! `GET /me/player/queue` every ~10s; we stop while paused/stopped.

use crate::app::{config::Config, lyrics::LyricsState, task::ManagedTask};
use futures_util::{
    FutureExt, StreamExt,
    future::{AbortHandle, Abortable, BoxFuture},
    stream::FuturesUnordered,
};
use std::collections::{HashMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use base64::Engine;
use log::{debug, warn};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio::time;

#[cfg(target_os = "macos")]
use crate::app::spotify::applescript::PlayerState;

const SPOTIFY_AUTH_URL: &str = "https://accounts.spotify.com/authorize";
const SPOTIFY_TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const SPOTIFY_API_BASE: &str = "https://api.spotify.com/v1";

/// Loopback listener bind address. Spotify's OAuth allows http loopback in dev
/// mode; the user must add this exact URI to their Spotify app's redirect URIs.
const REDIRECT_URI: &str = "http://127.0.0.1:53682/callback";
const REDIRECT_BIND: &str = "127.0.0.1:53682";

const SCOPES: &str = "user-read-playback-state";

const QUEUE_POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Spotify's queue API can lag a couple of seconds behind the AppleScript
/// track-change signal because the desktop client tells the API "advancing"
/// before the API state actually flips. Wait briefly before the immediate
/// fetch so we don't snapshot the previous track's queue.
#[cfg(target_os = "macos")]
const QUEUE_REFRESH_DELAY: Duration = Duration::from_millis(800);
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const TOKEN_SLACK: Duration = Duration::from_secs(30);

const KEYRING_SERVICE: &str = "FLTS-Spotify";
const KEYRING_ACCOUNT: &str = "refresh-token";

// ----- Public types ------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct TrackMeta {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub album: Option<String>,
    #[serde(rename = "durationMs")]
    pub duration_ms: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueSnapshot {
    /// One of "playlist", "album", "artist", "show", or None when there's no
    /// context (single-track playback, autoplay). Preload should only fire for
    /// "playlist" / "album" — for the others, the next track is undefined or
    /// not a song.
    #[serde(rename = "contextType")]
    pub context_type: Option<String>,
    #[serde(rename = "currentlyPlayingId")]
    pub currently_playing_id: Option<String>,
    pub upcoming: Vec<TrackMeta>,
}

/// Single source of truth for "what tracks should have lyrics+translation
/// resolved soon". Combines the AppleScript watcher's authoritative
/// current-track signal with Spotify Web's queue (next tracks + context).
/// Whenever the set of track IDs in this list changes, the resolver runs.
#[derive(Debug, Clone, Default)]
struct PlaybackList {
    current: Option<TrackMeta>,
    next: Vec<TrackMeta>,
    context_type: Option<String>,
}

impl PlaybackList {
    /// Emit-friendly view for the frontend's "Up next" UI and the
    /// `spotify_web_get_queue` Tauri command.
    fn as_snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            context_type: self.context_type.clone(),
            currently_playing_id: self.current.as_ref().map(|c| c.id.clone()),
            upcoming: self.next.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SpotifyWebStatus {
    pub connected: bool,
    /// True if the last `/me/player/queue` call returned 403 (Premium only).
    #[serde(rename = "premiumRequired")]
    pub premium_required: bool,
    #[serde(rename = "lastError")]
    pub last_error: Option<String>,
}

/// Distinguishes "refresh token is permanently dead" from "transient error
/// (network, 5xx, parse failure)". Only the permanent variant clears the
/// keychain entry.
#[derive(Debug)]
enum RefreshError {
    InvalidGrant(String),
    Transient(String),
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefreshError::InvalidGrant(body) => write!(f, "invalid_grant: {body}"),
            RefreshError::Transient(msg) => write!(f, "{msg}"),
        }
    }
}

#[derive(Debug, Clone)]
struct TokenInfo {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Instant,
}

#[derive(Default)]
struct Inner {
    token: Option<TokenInfo>,
    /// User-provided Spotify Dashboard client_id. We can't bundle one because
    /// each public client_id is tied to a specific developer's quota; the user
    /// supplies their own from https://developer.spotify.com/dashboard.
    client_id: Option<String>,
    premium_required: bool,
    last_error: Option<String>,
    /// Bumped by connect()/disconnect() in the controller. A token
    /// refresh in `access_token` captures this before awaiting the network and
    /// re-checks it before writing back, so a refresh that races a disconnect
    /// can't resurrect a torn-down session. See `accepts_generation`.
    generation: u64,
}

impl Inner {
    /// A refreshed access token may only be written back if the session
    /// generation hasn't advanced since the refresh started. connect() and
    /// disconnect() bump `generation`, so a refresh that completes after either
    /// ran must be discarded rather than resurrecting the old session.
    fn accepts_generation(&self, captured: u64) -> bool {
        self.generation == captured
    }
}

type Reply<T> = oneshot::Sender<Result<T, String>>;
enum WebRequest {
    Resume(Option<String>, oneshot::Sender<()>),
    Connect(String, Reply<()>),
    Disconnect(oneshot::Sender<()>),
    Start(oneshot::Sender<()>),
    Stop(oneshot::Sender<()>),
}
#[derive(Default)]
struct AuthQueue {
    pending: VecDeque<(String, Reply<()>)>,
    active: bool,
}
impl AuthQueue {
    fn push(&mut self, id: String, reply: Reply<()>) {
        if self.pending.len() >= 256 {
            let _ = reply.send(Err("Spotify authentication queue busy".into()));
            return;
        }
        self.pending.push_back((id, reply));
    }
    fn start_next(&mut self) -> Option<(String, Reply<()>)> {
        if self.active {
            return None;
        }
        let next = self.pending.pop_front()?;
        self.active = true;
        Some(next)
    }
    fn start_resume(&mut self) -> bool {
        if self.active {
            return false;
        }
        self.active = true;
        true
    }
    fn finish(&mut self) {
        self.active = false;
    }
}
#[derive(Clone)]
pub struct SpotifyWebState {
    requests: mpsc::Sender<WebRequest>,
    status: watch::Receiver<SpotifyWebStatus>,
    tx: watch::Receiver<Option<QueueSnapshot>>,
}
impl SpotifyWebState {
    pub fn new(
        app: AppHandle,
        config: watch::Receiver<Config>,
        lyrics: LyricsState,
    ) -> (Self, ManagedTask) {
        let (requests, receiver) = mpsc::channel(256);
        let (status, observed) = watch::channel(SpotifyWebStatus {
            connected: false,
            premium_required: false,
            last_error: None,
        });
        let (tx, queue) = watch::channel(None);
        let keepalive = requests.clone();
        let owner = ManagedTask::spawn(move |stop| async move {
            let _keepalive = keepalive;
            serve_web(app, config, lyrics, receiver, status, tx, stop).await;
        });
        (
            Self {
                requests,
                status: observed,
                tx: queue,
            },
            owner,
        )
    }
    pub async fn status(&self) -> SpotifyWebStatus {
        self.status.borrow().clone()
    }
    pub async fn try_resume(&self, id: Option<String>) {
        let (reply, result) = oneshot::channel();
        if self
            .requests
            .send(WebRequest::Resume(id, reply))
            .await
            .is_ok()
        {
            let _ = result.await;
        }
    }
    pub async fn connect(&self, id: String) -> Result<(), String> {
        let (reply, result) = oneshot::channel();
        self.requests
            .send(WebRequest::Connect(id, reply))
            .await
            .map_err(|_| "Spotify service closed".to_string())?;
        result
            .await
            .map_err(|_| "Spotify service closed".to_string())?
    }
    pub async fn disconnect(&self) {
        let (reply, result) = oneshot::channel();
        if self
            .requests
            .send(WebRequest::Disconnect(reply))
            .await
            .is_ok()
        {
            let _ = result.await;
        }
    }
    pub async fn start_polling(&self) {
        let (reply, result) = oneshot::channel();
        if self.requests.send(WebRequest::Start(reply)).await.is_ok() {
            let _ = result.await;
        }
    }
    pub async fn stop_polling(&self) {
        let (reply, result) = oneshot::channel();
        if self.requests.send(WebRequest::Stop(reply)).await.is_ok() {
            let _ = result.await;
        }
    }
}
struct WebIo {
    client: reqwest::Client,
}
enum QueueResult {
    Snapshot(QueueSnapshot),
    Premium,
    Empty,
}
enum Finished {
    Auth {
        generation: u64,
        client_id: String,
        reply: Option<Reply<()>>,
        resume: Option<oneshot::Sender<()>>,
        result: Result<Option<TokenInfo>, RefreshError>,
    },
    Poll {
        generation: u64,
        epoch: u64,
        result: Result<(Option<TokenInfo>, QueueResult), RefreshError>,
    },
}
impl WebIo {
    async fn authorize(&self, client_id: &str) -> Result<TokenInfo, RefreshError> {
        let verifier = generate_verifier();
        let challenge = code_challenge(&verifier);
        let listener = TcpListener::bind(REDIRECT_BIND).await.map_err(|e| {
            RefreshError::Transient(format!("Could not bind loopback {REDIRECT_BIND}: {e}"))
        })?;
        let auth_url = format!(
            "{SPOTIFY_AUTH_URL}?response_type=code&client_id={}&redirect_uri={}&code_challenge_method=S256&code_challenge={}&scope={}",
            urlencode(client_id),
            urlencode(REDIRECT_URI),
            urlencode(&challenge),
            urlencode(SCOPES)
        );
        if let Err(err) = webbrowser::open(&auth_url) {
            warn!("Failed to open browser: {err}");
        }
        let code = time::timeout(AUTH_TIMEOUT, wait_for_callback(listener))
            .await
            .map_err(|_| RefreshError::Transient("OAuth timed out — try again".into()))?
            .map_err(|e| RefreshError::Transient(format!("OAuth callback error: {e}")))?;
        self.exchange_code(client_id, &code, &verifier)
            .await
            .map_err(|e| RefreshError::Transient(format!("Token exchange failed: {e}")))
    }
    async fn poll(
        &self,
        client_id: String,
        token: TokenInfo,
    ) -> Result<(Option<TokenInfo>, QueueResult), RefreshError> {
        if Instant::now() + TOKEN_SLACK < token.expires_at {
            let queue = self
                .fetch_queue(&token.access_token)
                .await
                .map_err(|e| RefreshError::Transient(e.to_string()))?;
            return Ok((None, queue));
        }
        let Some(refresh) = token.refresh_token else {
            return Ok((None, QueueResult::Empty));
        };
        let token = self.refresh_with(&client_id, &refresh).await?;
        let queue = self
            .fetch_queue(&token.access_token)
            .await
            .map_err(|e| RefreshError::Transient(e.to_string()))?;
        Ok((Some(token), queue))
    }
    async fn exchange_code(
        &self,
        client_id: &str,
        code: &str,
        verifier: &str,
    ) -> anyhow::Result<TokenInfo> {
        let resp = self
            .client
            .post(SPOTIFY_TOKEN_URL)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", REDIRECT_URI),
                ("client_id", client_id),
                ("code_verifier", verifier),
            ])
            .send()
            .await?;

        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("Spotify token endpoint {status}: {body}");
        }
        let parsed: SpotifyTokenResponse = serde_json::from_str(&body)?;
        Ok(parsed.into_token_info())
    }

    async fn refresh_with(
        &self,
        client_id: &str,
        refresh: &str,
    ) -> Result<TokenInfo, RefreshError> {
        let resp = self
            .client
            .post(SPOTIFY_TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh),
                ("client_id", client_id),
            ])
            .send()
            .await
            .map_err(|e| RefreshError::Transient(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| RefreshError::Transient(e.to_string()))?;
        if !status.is_success() {
            // Spotify returns 400 + `"error":"invalid_grant"` when the refresh
            // token has been revoked, expired, or was issued for a different
            // client_id. That state is permanent for this token — keeping it
            // in the keychain just makes every future startup log the same
            // warning. Distinguish from transient (5xx, network) errors so
            // only the dead-token case clears the keychain.
            let is_invalid_grant = status == reqwest::StatusCode::BAD_REQUEST
                && body.contains("\"error\":\"invalid_grant\"");
            return Err(if is_invalid_grant {
                RefreshError::InvalidGrant(body)
            } else {
                RefreshError::Transient(format!("Spotify refresh {status}: {body}"))
            });
        }
        let mut parsed: SpotifyTokenResponse =
            serde_json::from_str(&body).map_err(|e| RefreshError::Transient(e.to_string()))?;
        // Spotify sometimes omits refresh_token on refresh — keep the old one.
        if parsed.refresh_token.is_none() {
            parsed.refresh_token = Some(refresh.to_string());
        }
        Ok(parsed.into_token_info())
    }

    /// Returns a valid access token, refreshing if expired (with a small slack
    /// window to avoid races near the boundary). Returns None if not connected.
    async fn fetch_queue(&self, token: &str) -> anyhow::Result<QueueResult> {
        debug!("fetch_queue: calling /me/player");

        // /me/player tells us the playback context (playlist/album/...). We
        // only want to preload when context is playlist or album.
        let player: Option<PlayerResponse> = {
            let resp = self
                .client
                .get(format!("{SPOTIFY_API_BASE}/me/player"))
                .bearer_auth(&token)
                .send()
                .await?;
            match resp.status() {
                s if s.is_success() => resp.json().await.ok(),
                reqwest::StatusCode::NO_CONTENT => None,
                s => {
                    let body = resp.text().await.unwrap_or_default();
                    anyhow::bail!("/me/player {s}: {body}");
                }
            }
        };

        let context_type = player.as_ref().and_then(|p| {
            p.context
                .as_ref()
                .map(|c| c.context_type.clone().to_lowercase())
        });
        let currently_playing_id = player
            .as_ref()
            .and_then(|p| p.item.as_ref().and_then(|i| i.id.clone()));

        // /me/player/queue returns currently_playing + queue array.
        let resp = self
            .client
            .get(format!("{SPOTIFY_API_BASE}/me/player/queue"))
            .bearer_auth(&token)
            .send()
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::FORBIDDEN {
            return Ok(QueueResult::Premium);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("/me/player/queue {status}: {body}");
        }
        let body: QueueResponse = resp.json().await?;
        let upcoming: Vec<TrackMeta> = body
            .queue
            .into_iter()
            .filter_map(|t| t.into_meta())
            .collect();

        Ok(QueueResult::Snapshot(QueueSnapshot {
            context_type,
            currently_playing_id,
            upcoming,
        }))
    }
}

fn publish_status(inner: &Inner, status: &watch::Sender<SpotifyWebStatus>) {
    status.send_replace(SpotifyWebStatus {
        connected: inner.token.is_some(),
        premium_required: inner.premium_required,
        last_error: inner.last_error.clone(),
    });
}

fn release_failed_claim(claims: &mut HashMap<String, u64>, key: &str, id: u64) -> bool {
    if claims.get(key) != Some(&id) {
        return false;
    }
    claims.remove(key);
    true
}

type Resolution = (u64, String, String, anyhow::Result<()>);
fn publish_playback<'a>(
    list: &PlaybackList,
    tx: &watch::Sender<Option<QueueSnapshot>>,
    app: &'a AppHandle,
    lyrics: &'a LyricsState,
    _config: &Config,
    claims: &mut HashMap<String, u64>,
    next_id: &mut u64,
    resolutions: &mut FuturesUnordered<BoxFuture<'a, Resolution>>,
) {
    let snapshot = list.as_snapshot();
    tx.send_replace(Some(snapshot.clone()));
    let _ = app.emit("spotify_queue", snapshot);
    let Some((config, generation)) = lyrics.configuration() else {
        return;
    };
    if config.target_language_id.is_empty() {
        return;
    }
    let mut tracks = Vec::new();
    if let Some(track) = &list.current {
        tracks.push(track);
    }
    if matches!(
        list.context_type.as_deref(),
        Some("playlist") | Some("album")
    ) {
        tracks.extend(
            list.next
                .iter()
                .take(config.spotify_preload_count.min(3) as usize),
        );
    }
    for track in tracks {
        let key = format!(
            "{}|{}|{}|{:?}",
            track.id, config.target_language_id, config.model, config.translation_provider
        );
        if claims.contains_key(&key) {
            continue;
        }
        let id = *next_id;
        *next_id += 1;
        claims.insert(key.clone(), id);
        let cfg = config.clone();
        let track = track.clone();
        resolutions.push(
            async move {
                let result = AssertUnwindSafe(lyrics.resolve(
                    &cfg,
                    generation,
                    app,
                    &track,
                    &cfg.target_language_id,
                    cfg.model.clone(),
                ))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("lyrics resolver panicked")));
                (id, key, track.id, result)
            }
            .boxed(),
        );
    }
}
async fn serve_web(
    app: AppHandle,
    mut config: watch::Receiver<Config>,
    lyrics: LyricsState,
    mut requests: mpsc::Receiver<WebRequest>,
    status: watch::Sender<SpotifyWebStatus>,
    tx: watch::Sender<Option<QueueSnapshot>>,
    mut stop: watch::Receiver<bool>,
) {
    let io = WebIo {
        client: reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .expect("reqwest client"),
    };
    let mut inner = Inner::default();
    let mut network: FuturesUnordered<BoxFuture<'_, Finished>> = FuturesUnordered::new();
    let mut resolutions = FuturesUnordered::new();
    let mut auth = AuthQueue::default();
    let mut auth_abort: Option<AbortHandle> = None;
    let mut poll_busy = false;
    let mut poll_abort: Option<AbortHandle> = None;
    let mut polling = false;
    let mut epoch = 0u64;
    let mut next_poll = time::Instant::now();
    let mut list = PlaybackList::default();
    let mut claims = HashMap::new();
    let mut next_resolution = 0u64;
    #[cfg(target_os = "macos")]
    let mut native = lyrics.watcher.subscribe();
    #[cfg(not(target_os = "macos"))]
    let mut native = watch::channel::<Option<()>>(None).1;
    let mut last_track: Option<String> = None;
    let mut config_open = true;
    let mut native_open = true;
    loop {
        if let Some((client_id, reply)) = auth.start_next() {
            inner.generation = inner.generation.wrapping_add(1);
            inner.client_id = Some(client_id.clone());
            inner.last_error = None;
            let generation = inner.generation;
            let io = &io;
            let (abort, registration) = AbortHandle::new_pair();
            auth_abort = Some(abort);
            network.push(
                async move {
                    let result = Abortable::new(
                        AssertUnwindSafe(io.authorize(&client_id)).catch_unwind(),
                        registration,
                    )
                    .await
                    .map_err(|_| RefreshError::Transient("Spotify session changed".into()))
                    .and_then(|r| {
                        r.unwrap_or_else(|_| {
                            Err(RefreshError::Transient("OAuth worker panicked".into()))
                        })
                    })
                    .map(Some);
                    Finished::Auth {
                        generation,
                        client_id,
                        reply: Some(reply),
                        resume: None,
                        result,
                    }
                }
                .boxed(),
            );
        }
        if polling && !poll_busy && next_poll <= time::Instant::now() {
            #[cfg(target_os = "macos")]
            {
                list.current = lyrics
                    .watcher
                    .current()
                    .filter(|n| matches!(n.state, PlayerState::Playing | PlayerState::Paused))
                    .and_then(np_to_track_meta);
            }
            if list.current.is_none() {
                list.next.clear();
                list.context_type = None;
            }
            publish_playback(
                &list,
                &tx,
                &app,
                &lyrics,
                &config.borrow(),
                &mut claims,
                &mut next_resolution,
                &mut resolutions,
            );
            if list.current.is_some()
                && let (Some(client_id), Some(token)) =
                    (inner.client_id.clone(), inner.token.clone())
            {
                let generation = inner.generation;
                let epoch = epoch;
                let io = &io;
                poll_busy = true;
                let (abort, registration) = AbortHandle::new_pair();
                poll_abort = Some(abort);
                network.push(
                    async move {
                        let result = Abortable::new(
                            AssertUnwindSafe(io.poll(client_id, token)).catch_unwind(),
                            registration,
                        )
                        .await
                        .map_err(|_| RefreshError::Transient("poll stopped".into()))
                        .and_then(|r| {
                            r.unwrap_or_else(|_| {
                                Err(RefreshError::Transient("Spotify poll panicked".into()))
                            })
                        });
                        Finished::Poll {
                            generation,
                            epoch,
                            result,
                        }
                    }
                    .boxed(),
                );
            }
            next_poll = time::Instant::now() + QUEUE_POLL_INTERVAL;
        }
        publish_status(&inner, &status);
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            request = requests.recv() => match request {
                None => break,
                Some(WebRequest::Connect(id, reply)) => { auth.push(id, reply); }
                Some(WebRequest::Resume(id, reply)) => {
                    let Some(client_id) = id else { let _ = reply.send(()); continue; };
                    if !auth.start_resume() { let _ = reply.send(()); continue; }
                    inner.client_id = Some(client_id.clone()); let generation = inner.generation; let io = &io;
                    network.push(async move {
                        let result = match load_refresh_token().await { Some(refresh) => io.refresh_with(&client_id, &refresh).await.map(Some), None => Ok(None) };
                        Finished::Auth { generation, client_id, reply: None, resume: Some(reply), result }
                    }.boxed());
                }
                Some(WebRequest::Disconnect(reply)) => {
                    inner.generation = inner.generation.wrapping_add(1); inner.token = None; inner.last_error = None; inner.premium_required = false;
                    if let Some(abort) = auth_abort.take() { abort.abort(); }
                    if let Some(abort) = poll_abort.take() { abort.abort(); }
                    if let Err(err) = delete_refresh_token().await { debug!("Keyring delete: {err}"); }
                    list.next.clear(); list.context_type = None; tx.send_replace(None); let _ = reply.send(());
                }
                Some(WebRequest::Start(reply)) => { if !polling { polling = true; next_poll = time::Instant::now(); } let _ = reply.send(()); }
                Some(WebRequest::Stop(reply)) => {
                    polling = false; epoch = epoch.wrapping_add(1); poll_busy = false;
                    if let Some(abort) = poll_abort.take() { abort.abort(); }
                    resolutions.clear(); claims.clear(); list = PlaybackList::default(); tx.send_replace(None); let _ = reply.send(());
                }
            },
            changed = config.changed(), if config_open => { if changed.is_ok() { config.borrow_and_update(); resolutions.clear(); claims.clear(); next_poll = time::Instant::now(); } else { config_open = false; } }
            changed = native.changed(), if polling && native_open => {
                if changed.is_err() { native_open = false; continue; }
                if changed.is_ok() {
                    #[cfg(target_os = "macos")] let track = native.borrow_and_update().as_ref().and_then(|n| n.track_id.clone());
                    #[cfg(not(target_os = "macos"))] let track: Option<String> = None;
                    if track != last_track {
                        #[cfg(target_os = "macos")] let delay = if last_track.is_none() { Duration::ZERO } else { QUEUE_REFRESH_DELAY };
                        #[cfg(not(target_os = "macos"))] let delay = Duration::ZERO;
                        last_track = track; next_poll = time::Instant::now() + delay;
                    }
                }
            }
            _ = time::sleep_until(next_poll), if polling && !poll_busy => {}
            Some(finished) = network.next(), if !network.is_empty() => match finished {
                Finished::Auth { generation, client_id, reply, resume, result } => {
                    auth.finish(); auth_abort = None;
                    let accepted = inner.accepts_generation(generation);
                    let outcome = if !accepted { Err("Spotify session changed".into()) } else {
                        match result {
                            Ok(token) => {
                                if let Some(token) = token {
                                    if let Some(refresh) = &token.refresh_token { if let Err(err) = save_refresh_token(refresh.clone()).await { warn!("Could not persist refresh token: {err}"); } }
                                    inner.token = Some(token); inner.client_id = Some(client_id); inner.premium_required = false; inner.last_error = None; next_poll = time::Instant::now();
                                } Ok(())
                            }
                            Err(RefreshError::InvalidGrant(err)) => { let _ = delete_refresh_token().await; inner.token = None; inner.last_error = Some("Spotify access was revoked — please reconnect.".into()); Err(err) }
                            Err(RefreshError::Transient(err)) => { inner.last_error = Some(err.clone()); Err(err) }
                        }
                    };
                    if let Some(reply) = reply { let _ = reply.send(outcome); } if let Some(reply) = resume { let _ = reply.send(()); }
                }
                Finished::Poll { generation, epoch: issued, result } => {
                    if issued != epoch { continue; } poll_busy = false; poll_abort = None;
                    if !inner.accepts_generation(generation) { continue; }
                    match result {
                        Ok((token, result)) => {
                            if let Some(token) = token { inner.token = Some(token); }
                            match result {
                                QueueResult::Snapshot(snapshot) => { inner.premium_required = false; inner.last_error = None; let current_id = list.current.as_ref().map(|t| &t.id); list.next = snapshot.upcoming.into_iter().filter(|t| Some(&t.id) != current_id).collect(); list.context_type = snapshot.context_type; }
                                QueueResult::Premium => { inner.premium_required = true; list.next.clear(); list.context_type = None; }
                                QueueResult::Empty => { list.next.clear(); list.context_type = None; }
                            }
                        }
                        Err(RefreshError::InvalidGrant(_)) => { let _ = delete_refresh_token().await; inner.token = None; inner.last_error = Some("Spotify access was revoked — please reconnect.".into()); }
                        Err(RefreshError::Transient(err)) => { inner.last_error = Some(err); }
                    }
                    publish_playback(&list, &tx, &app, &lyrics, &config.borrow(), &mut claims, &mut next_resolution, &mut resolutions);
                }
            },
            Some((id, key, track_id, result)) = resolutions.next(), if !resolutions.is_empty() => {
                if claims.get(&key) != Some(&id) { continue; }
                if let Err(err) = result {
                    release_failed_claim(&mut claims, &key, id);
                    let _ = app.emit("lyrics_translation_error", crate::app::lyrics::LyricsTranslationError { track_id, error: err.to_string() });
                }
            }
        }
    }
    requests.close();
    network.clear();
    resolutions.clear();
    tx.send_replace(None);
}
// ----- Spotify API response types ---------------------------------------

#[derive(Deserialize)]
struct SpotifyTokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
}

impl SpotifyTokenResponse {
    fn into_token_info(self) -> TokenInfo {
        TokenInfo {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            expires_at: Instant::now() + Duration::from_secs(self.expires_in),
        }
    }
}

#[derive(Deserialize)]
struct PlayerResponse {
    context: Option<PlayerContext>,
    item: Option<PlayerItem>,
}

#[derive(Deserialize)]
struct PlayerContext {
    #[serde(rename = "type")]
    context_type: String,
}

#[derive(Deserialize)]
struct PlayerItem {
    id: Option<String>,
}

#[derive(Deserialize)]
struct QueueResponse {
    queue: Vec<ApiTrack>,
}

#[derive(Deserialize)]
struct ApiTrack {
    /// Tracks have an id; episodes/ads might not. Skip those when None.
    id: Option<String>,
    name: Option<String>,
    artists: Option<Vec<ApiArtist>>,
    album: Option<ApiAlbum>,
    duration_ms: Option<u32>,
}

#[derive(Deserialize)]
struct ApiArtist {
    name: String,
}

#[derive(Deserialize)]
struct ApiAlbum {
    name: String,
}

impl ApiTrack {
    fn into_meta(self) -> Option<TrackMeta> {
        Some(TrackMeta {
            id: format!("spotify:track:{}", self.id?),
            name: self.name?,
            artist: self
                .artists?
                .into_iter()
                .map(|a| a.name)
                .collect::<Vec<_>>()
                .join(", "),
            album: self.album.map(|a| a.name),
            duration_ms: self.duration_ms?,
        })
    }
}

#[cfg(target_os = "macos")]
fn np_to_track_meta(np: crate::app::spotify::applescript::NowPlaying) -> Option<TrackMeta> {
    Some(TrackMeta {
        id: np.track_id?,
        name: np.name?,
        artist: np.artist?,
        album: np.album,
        duration_ms: np.duration_ms?,
    })
}

// ----- PKCE helpers ------------------------------------------------------

fn generate_verifier() -> String {
    // RFC 7636: 43-128 chars from unreserved set. 64 random bytes -> base64url
    // is well within range and gives ~86 characters.
    let mut buf = [0u8; 64];
    rand::rng().fill(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

fn code_challenge(verifier: &str) -> String {
    let digest = sha2::Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn urlencode(s: &str) -> String {
    // Minimal RFC 3986 unreserved-only encoder. Avoids pulling in the `url`
    // crate just for this.
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ----- Loopback callback handler ----------------------------------------

const CALLBACK_PAGE: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>FLTS Spotify</title></head><body style=\"font-family:system-ui;text-align:center;padding:40px\"><h2>Spotify connected</h2><p>You can close this window and return to FLTS.</p></body></html>";

async fn wait_for_callback(listener: TcpListener) -> anyhow::Result<String> {
    // Accept connections until we get one that includes `?code=...` in the
    // GET line. Spotify might send a single request, but browsers occasionally
    // also fetch /favicon.ico etc., so we loop instead of taking the very
    // first accept.
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut reader = BufReader::new(&mut stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).await?;

        // Drain the rest of the headers (we don't need them).
        loop {
            let mut header = String::new();
            let n = reader.read_line(&mut header).await?;
            if n == 0 || header == "\r\n" || header == "\n" {
                break;
            }
        }

        // request_line: "GET /callback?code=XYZ&state=... HTTP/1.1"
        let code = parse_code(&request_line);
        let body = match &code {
            Some(_) => CALLBACK_PAGE.to_string(),
            None => {
                // Probably /favicon.ico or similar — respond and keep listening.
                "<html><body>FLTS callback ready.</body></html>".to_string()
            }
        };

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).await.ok();
        stream.shutdown().await.ok();

        if let Some(code) = code {
            return Ok(code);
        }
    }
}

fn parse_code(request_line: &str) -> Option<String> {
    // "GET /callback?code=XYZ&state=... HTTP/1.1"
    let path = request_line.split_whitespace().nth(1)?;
    let query = path.split_once('?')?.1;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=')
            && k == "code"
        {
            return Some(decode_url(v));
        }
    }
    None
}

fn decode_url(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                let v = u8::from_str_radix(hex, 16).unwrap_or(0);
                out.push(v);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ----- Keyring helpers --------------------------------------------------

/// Long enough for a user to answer a keychain password prompt.
const KEYRING_TIMEOUT: Duration = Duration::from_secs(30);

/// One at a time: a timed-out `spawn_blocking` thread stays parked on the OS
/// dialog forever (runtime shutdown waits on it), so never start a second.
static KEYRING_GATE: Mutex<()> = Mutex::const_new(());

/// Overridable so tests never touch the developer's real login keychain.
fn keyring_service() -> &'static str {
    static SERVICE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SERVICE.get_or_init(|| {
        std::env::var("FLTS_KEYRING_SERVICE")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| KEYRING_SERVICE.to_string())
    })
}

/// Test harnesses set this: any keychain touch can raise an OS access prompt,
/// and a per-run service name makes every run a freshly prompting one.
fn keyring_disabled() -> bool {
    keyring_disabled_from(std::env::var("FLTS_DISABLE_KEYRING").ok().as_deref())
}

fn keyring_disabled_from(v: Option<&str>) -> bool {
    v == Some("1")
}

/// The keychain blocks indefinitely on its access-confirmation dialog, so calls
/// run off-runtime under a hard bound; a lost call costs a manual reconnect.
async fn keyring_op<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> anyhow::Result<T> {
    let _gate = KEYRING_GATE.lock().await;
    match time::timeout(KEYRING_TIMEOUT, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(err)) => Err(anyhow::anyhow!("keychain task failed: {err}")),
        Err(_) => Err(anyhow::anyhow!("keychain call timed out")),
    }
}

async fn load_refresh_token() -> Option<String> {
    if keyring_disabled() {
        return None;
    }
    keyring_op(|| {
        let entry = keyring::Entry::new(keyring_service(), KEYRING_ACCOUNT).ok()?;
        entry.get_password().ok()
    })
    .await
    .unwrap_or_else(|err| {
        warn!("Keyring load: {err}");
        None
    })
}

async fn save_refresh_token(token: String) -> anyhow::Result<()> {
    if keyring_disabled() {
        return Ok(());
    }
    keyring_op(move || -> anyhow::Result<()> {
        let entry = keyring::Entry::new(keyring_service(), KEYRING_ACCOUNT)?;
        entry.set_password(&token)?;
        Ok(())
    })
    .await?
}

async fn delete_refresh_token() -> anyhow::Result<()> {
    if keyring_disabled() {
        return Ok(());
    }
    keyring_op(|| -> anyhow::Result<()> {
        let entry = keyring::Entry::new(keyring_service(), KEYRING_ACCOUNT)?;
        entry.delete_credential()?;
        Ok(())
    })
    .await?
}

// ----- Tauri commands ----------------------------------------------------

#[tauri::command]
pub async fn spotify_web_connect(
    state: tauri::State<'_, crate::app::AppState>,
    client_id: String,
) -> Result<(), String> {
    state.spotify_web.connect(client_id).await
}

#[tauri::command]
pub async fn spotify_web_disconnect(
    state: tauri::State<'_, crate::app::AppState>,
) -> Result<(), String> {
    state.spotify_web.disconnect().await;
    Ok(())
}

#[tauri::command]
pub async fn spotify_web_status(
    state: tauri::State<'_, crate::app::AppState>,
) -> Result<SpotifyWebStatus, String> {
    Ok(state.spotify_web.status().await)
}

#[tauri::command]
pub async fn spotify_web_get_queue(
    state: tauri::State<'_, crate::app::AppState>,
) -> Result<Option<QueueSnapshot>, String> {
    Ok(state.spotify_web.tx.borrow().clone())
}

/// Open a URL in the user's default external browser. Used by settings links
/// (e.g. the Spotify Developer Dashboard) — the webview would otherwise just
/// navigate away from the app.
#[tauri::command]
pub async fn open_external_url(url: String) -> Result<(), String> {
    // Only allow http(s) — anything else (file://, javascript:, ...) is either
    // unsafe or unintended for a settings page button.
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(format!("refusing to open non-http URL: {url}"));
    }
    webbrowser::open(&url).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn status_remains_readable_while_connect_is_waiting() {
        let (requests, mut receiver) = mpsc::channel(4);
        let status = watch::channel(SpotifyWebStatus {
            connected: false,
            premium_required: false,
            last_error: None,
        })
        .1;
        let tx = watch::channel(None).1;
        let client = SpotifyWebState {
            requests,
            status,
            tx,
        };
        let pending = tokio::spawn({
            let client = client.clone();
            async move { client.connect("test".into()).await }
        });
        let Some(WebRequest::Connect(_, reply)) = receiver.recv().await else {
            panic!("expected connect");
        };
        let status = time::timeout(Duration::from_millis(100), client.status())
            .await
            .unwrap();
        assert!(!status.connected);
        reply.send(Err("cancelled".into())).unwrap();
        assert_eq!(pending.await.unwrap(), Err("cancelled".into()));
    }
    #[test]
    fn oauth_requests_wait_in_order_and_share_one_active_flow() {
        let mut queue = AuthQueue::default();
        let (first, _first) = oneshot::channel();
        let (second, _second) = oneshot::channel();
        queue.push("first".into(), first);
        queue.push("second".into(), second);
        assert_eq!(queue.start_next().unwrap().0, "first");
        assert!(queue.start_next().is_none());
        assert!(!queue.start_resume());
        queue.finish();
        assert_eq!(queue.start_next().unwrap().0, "second");
        assert!(queue.start_next().is_none());
        queue.finish();
        assert!(queue.start_resume());
    }

    #[test]
    fn only_exactly_1_disables_the_keyring() {
        assert!(keyring_disabled_from(Some("1")));
        assert!(!keyring_disabled_from(None));
        assert!(!keyring_disabled_from(Some("0")));
        assert!(!keyring_disabled_from(Some("")));
    }

    #[test]
    fn pkce_challenge_matches_rfc7636_example() {
        // RFC 7636 Appendix B test vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = code_challenge(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn parses_code_from_request_line() {
        let line = "GET /callback?code=abc123&state=xyz HTTP/1.1\r\n";
        assert_eq!(parse_code(line), Some("abc123".to_string()));
    }

    #[test]
    fn parses_code_when_url_encoded() {
        let line = "GET /callback?code=a%20b HTTP/1.1\r\n";
        assert_eq!(parse_code(line), Some("a b".to_string()));
    }

    #[test]
    fn skips_when_no_code() {
        let line = "GET /favicon.ico HTTP/1.1\r\n";
        assert_eq!(parse_code(line), None);
    }

    #[test]
    fn url_encodes_unreserved_unchanged() {
        assert_eq!(urlencode("abcXYZ123-_.~"), "abcXYZ123-_.~");
    }

    #[test]
    fn url_encodes_reserved() {
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
    }

    // BUG C: a token refresh that completes after disconnect()/connect() bumped
    // the generation must be discarded, not written back.
    #[test]
    fn refreshed_token_rejected_after_generation_bump() {
        let mut inner = Inner::default();
        // A refresh started at generation 0 is accepted while still on gen 0.
        assert!(inner.accepts_generation(0));
        // disconnect()/connect() advance the controller generation.
        inner.generation = inner.generation.wrapping_add(1);
        // The in-flight refresh (captured gen 0) is now stale and rejected...
        assert!(!inner.accepts_generation(0));
        // ...while a refresh started after the bump (gen 1) is still valid.
        assert!(inner.accepts_generation(1));
    }

    #[test]
    fn failed_claim_can_retry_and_stale_completion_cannot_release_successor() {
        let mut claims = HashMap::new();
        let key = "track|eng|model".to_string();
        claims.insert(key.clone(), 1u64);
        assert!(claims.contains_key(&key));
        assert!(release_failed_claim(&mut claims, &key, 1));
        claims.insert(key.clone(), 2);
        assert!(!release_failed_claim(&mut claims, &key, 1));
        assert_eq!(claims.get(&key), Some(&2));
        assert!(release_failed_claim(&mut claims, &key, 2));
    }
}
