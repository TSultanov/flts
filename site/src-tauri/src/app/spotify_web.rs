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

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use log::{debug, info, warn};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, RwLock, watch};
use tokio::task::JoinHandle;
use tokio::time;

#[cfg(target_os = "macos")]
use crate::app::spotify::{PlayerState, SpotifyWatcher};

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
    /// Order-preserving sequence of track ids; used as the dedup key for
    /// "has the list changed?" comparisons.
    fn track_ids(&self) -> Vec<String> {
        let mut ids = Vec::with_capacity(self.next.len() + 1);
        if let Some(c) = &self.current {
            ids.push(c.id.clone());
        }
        ids.extend(self.next.iter().map(|t| t.id.clone()));
        ids
    }

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

#[derive(Debug)]
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
    poll_handle: Option<JoinHandle<()>>,
    premium_required: bool,
    last_error: Option<String>,
}

pub struct SpotifyWebState {
    inner: RwLock<Inner>,
    tx: Arc<watch::Sender<Option<QueueSnapshot>>>,
    client: reqwest::Client,
    /// Serializes the OAuth flow so two simultaneous Connect clicks don't
    /// fight over the loopback listener.
    auth_lock: Mutex<()>,
}

impl Default for SpotifyWebState {
    fn default() -> Self {
        Self::new()
    }
}

impl SpotifyWebState {
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(None);
        let client = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .expect("reqwest client builds with default config");
        Self {
            inner: RwLock::new(Inner::default()),
            tx: Arc::new(tx),
            client,
            auth_lock: Mutex::new(()),
        }
    }

    pub async fn status(&self) -> SpotifyWebStatus {
        let inner = self.inner.read().await;
        SpotifyWebStatus {
            connected: inner.token.is_some(),
            premium_required: inner.premium_required,
            last_error: inner.last_error.clone(),
        }
    }

    /// Restore session from keyring on app start. Best-effort: a failure here
    /// just means the user will see "Not connected" and can reconnect manually.
    /// Polling is owned by the watcher lifecycle (start_spotify_watcher), not
    /// by connection state — so this method only restores credentials.
    pub async fn try_resume(self: &Arc<Self>, client_id: Option<String>) {
        let Some(client_id) = client_id else {
            return;
        };
        let Some(refresh) = load_refresh_token() else {
            return;
        };
        {
            let mut inner = self.inner.write().await;
            inner.client_id = Some(client_id.clone());
        }
        match self.refresh_with(&client_id, &refresh).await {
            Ok(token) => {
                let mut inner = self.inner.write().await;
                inner.token = Some(token);
                info!("Spotify Web: resumed from stored refresh token");
            }
            Err(RefreshError::InvalidGrant(_)) => {
                warn!(
                    "Spotify Web: stored refresh token is no longer valid \
                     (revoked or expired); clearing keychain entry"
                );
                if let Err(err) = delete_refresh_token() {
                    debug!("Keyring delete after invalid_grant: {err}");
                }
                let mut inner = self.inner.write().await;
                inner.last_error =
                    Some("Spotify access was revoked — please reconnect.".to_string());
            }
            Err(RefreshError::Transient(err)) => {
                warn!("Spotify Web: refresh on resume failed (transient): {err}");
            }
        }
    }

    /// Run the PKCE auth flow. Blocks until the browser callback arrives (or
    /// timeout) and stores the refresh token in the keyring on success.
    /// Polling itself is tied to the watcher lifecycle and runs independently;
    /// connecting just gives that loop a valid token to use.
    pub async fn connect(self: &Arc<Self>, client_id: String) -> Result<(), String> {
        let _guard = self.auth_lock.lock().await;

        // Persist client_id immediately so polling can use it after a restart.
        {
            let mut inner = self.inner.write().await;
            inner.client_id = Some(client_id.clone());
            inner.last_error = None;
        }

        let verifier = generate_verifier();
        let challenge = code_challenge(&verifier);

        let listener = TcpListener::bind(REDIRECT_BIND)
            .await
            .map_err(|e| format!("Could not bind loopback {REDIRECT_BIND}: {e}"))?;

        let auth_url = format!(
            "{SPOTIFY_AUTH_URL}?response_type=code&client_id={cid}&redirect_uri={uri}&code_challenge_method=S256&code_challenge={challenge}&scope={scope}",
            cid = urlencode(&client_id),
            uri = urlencode(REDIRECT_URI),
            challenge = urlencode(&challenge),
            scope = urlencode(SCOPES),
        );

        if let Err(err) = webbrowser::open(&auth_url) {
            warn!("Failed to open browser: {err}. Auth URL: {auth_url}");
        }

        let code = time::timeout(AUTH_TIMEOUT, wait_for_callback(listener))
            .await
            .map_err(|_| "OAuth timed out — try again".to_string())?
            .map_err(|e| format!("OAuth callback error: {e}"))?;

        let token = self
            .exchange_code(&client_id, &code, &verifier)
            .await
            .map_err(|e| {
                let msg = format!("Token exchange failed: {e}");
                warn!("{msg}");
                msg
            })?;

        if let Some(refresh) = token.refresh_token.as_deref() {
            if let Err(err) = save_refresh_token(refresh) {
                warn!("Could not persist refresh token to keyring: {err}");
            }
        }

        {
            let mut inner = self.inner.write().await;
            inner.token = Some(token);
            inner.premium_required = false;
            inner.last_error = None;
        }
        info!("Spotify Web: connected");
        Ok(())
    }

    pub async fn disconnect(&self) {
        {
            let mut inner = self.inner.write().await;
            inner.token = None;
            inner.premium_required = false;
            inner.last_error = None;
            // Poll loop stays alive — its job is to keep resolving the
            // current track from AppleScript signals. Without a token its
            // queue fetches become no-ops, which is exactly what we want.
        }
        if let Err(err) = delete_refresh_token() {
            debug!("Keyring delete: {err}");
        }
        // Clear queue snapshot so the UI hides Up Next.
        let _ = self.tx.send(None);
        info!("Spotify Web: disconnected");
    }

    pub async fn stop_polling(&self) {
        let mut inner = self.inner.write().await;
        if let Some(h) = inner.poll_handle.take() {
            h.abort();
        }
        let _ = self.tx.send(None);
    }

    /// Spawns the queue poller. Idempotent; subsequent calls are no-ops while
    /// a poller is running. The poller exits silently when `disconnect()` is
    /// called (handle is aborted). The `state` reference lets the loop call
    /// into the lyrics module to preload upcoming tracks after each queue
    /// refresh.
    #[cfg(target_os = "macos")]
    pub async fn start_polling(
        self: &Arc<Self>,
        app: AppHandle,
        watcher: Arc<SpotifyWatcher>,
        state: Arc<crate::app::AppState>,
    ) {
        let mut inner = self.inner.write().await;
        if inner.poll_handle.is_some() {
            info!("Spotify Web: start_polling called but loop already running");
            return;
        }
        let connected = inner.token.is_some();
        info!(
            "Spotify Web: starting poll loop (connected={connected}, has_client_id={})",
            inner.client_id.is_some()
        );
        let me = self.clone();
        let tx = self.tx.clone();
        let handle = tokio::spawn(async move {
            poll_loop(me, app, watcher, tx, state).await;
        });
        inner.poll_handle = Some(handle);
    }

    // ----- HTTP helpers ---------------------------------------------------

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

    async fn refresh_with(&self, client_id: &str, refresh: &str) -> Result<TokenInfo, RefreshError> {
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
        let mut parsed: SpotifyTokenResponse = serde_json::from_str(&body)
            .map_err(|e| RefreshError::Transient(e.to_string()))?;
        // Spotify sometimes omits refresh_token on refresh — keep the old one.
        if parsed.refresh_token.is_none() {
            parsed.refresh_token = Some(refresh.to_string());
        }
        Ok(parsed.into_token_info())
    }

    /// Returns a valid access token, refreshing if expired (with a small slack
    /// window to avoid races near the boundary). Returns None if not connected.
    async fn access_token(&self) -> Option<String> {
        let now = Instant::now();
        {
            let inner = self.inner.read().await;
            if let Some(tok) = &inner.token
                && tok.expires_at > now + TOKEN_SLACK
            {
                return Some(tok.access_token.clone());
            }
        }
        // Need refresh.
        let (client_id, refresh) = {
            let inner = self.inner.read().await;
            let cid = inner.client_id.clone()?;
            let r = inner.token.as_ref().and_then(|t| t.refresh_token.clone())?;
            (cid, r)
        };
        match self.refresh_with(&client_id, &refresh).await {
            Ok(token) => {
                let access = token.access_token.clone();
                let mut inner = self.inner.write().await;
                inner.token = Some(token);
                Some(access)
            }
            Err(RefreshError::InvalidGrant(_)) => {
                // Token was revoked or expired mid-session. Drop it so future
                // fetch_queue calls silently no-op (the poll loop keeps
                // running for current-track resolution either way).
                warn!("Spotify Web: refresh token revoked mid-session; clearing");
                if let Err(err) = delete_refresh_token() {
                    debug!("Keyring delete after invalid_grant: {err}");
                }
                let mut inner = self.inner.write().await;
                inner.token = None;
                inner.last_error =
                    Some("Spotify access was revoked — please reconnect.".to_string());
                None
            }
            Err(RefreshError::Transient(err)) => {
                warn!("Spotify Web: refresh failed (transient): {err}");
                let mut inner = self.inner.write().await;
                inner.last_error = Some(err);
                None
            }
        }
    }

    async fn fetch_queue(&self) -> anyhow::Result<Option<QueueSnapshot>> {
        let Some(token) = self.access_token().await else {
            debug!("fetch_queue: no access token (not connected) — returning None");
            return Ok(None);
        };
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
            let mut inner = self.inner.write().await;
            inner.premium_required = true;
            return Ok(None);
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

        // Reset the "premium required" flag now that we got a real response.
        {
            let mut inner = self.inner.write().await;
            inner.premium_required = false;
        }

        Ok(Some(QueueSnapshot {
            context_type,
            currently_playing_id,
            upcoming,
        }))
    }
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

// ----- Polling loop ------------------------------------------------------

#[cfg(target_os = "macos")]
async fn poll_loop(
    web: Arc<SpotifyWebState>,
    app: AppHandle,
    watcher: Arc<SpotifyWatcher>,
    tx: Arc<watch::Sender<Option<QueueSnapshot>>>,
    app_state: Arc<crate::app::AppState>,
) {
    info!("poll_loop: entered");
    let mut ticker = time::interval(QUEUE_POLL_INTERVAL);
    ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    // Single source of truth for "tracks to keep resolved". Updated by both
    // the AppleScript watcher (current) and Spotify Web (next + context).
    let mut list = PlaybackList::default();
    let mut last_ids: Vec<String> = Vec::new();
    let mut resolved: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut state_rx = watcher.subscribe();
    // None means we haven't seen the watcher emit yet. The first emit is
    // treated as "initial" (not a track change) so we don't pay the Spotify-
    // API-settle delay before our first queue fetch.
    let mut last_track_id: Option<String> = None;

    loop {
        // Wake on either a scheduled tick or a track-change from the watcher.
        // play/pause and position jumps also wake us up but we filter them
        // here — only a real track_id transition is structurally interesting.
        let was_initial = last_track_id.is_none();
        let wake_reason: &'static str;
        tokio::select! {
            _ = ticker.tick() => { wake_reason = "tick"; }
            res = state_rx.changed() => {
                if res.is_err() {
                    info!("poll_loop: watcher channel closed, exiting");
                    return;
                }
                let new_id = state_rx
                    .borrow_and_update()
                    .as_ref()
                    .and_then(|np| np.track_id.clone());
                if new_id == last_track_id {
                    // Same track — must be play/pause or position seek. Not
                    // structurally interesting; wait for the next signal.
                    continue;
                }
                info!(
                    "poll_loop: track_id changed {:?} -> {:?} (initial={})",
                    last_track_id, new_id, was_initial
                );
                last_track_id = new_id;
                if !was_initial {
                    time::sleep(QUEUE_REFRESH_DELAY).await;
                }
                wake_reason = if was_initial { "initial" } else { "track_change" };
            }
        }
        info!("poll_loop: iteration begin (reason={wake_reason})");

        // Pull current track from the watcher. Both Playing and Paused are
        // valid "has-a-current-track" states — Spotify keeps the queue
        // context across pauses, and so should we. Only Stopped / NotRunning
        // mean there's nothing playing-or-pending to look at.
        let np = watcher.current();
        let has_current_track = matches!(
            np.as_ref().map(|n| &n.state),
            Some(PlayerState::Playing) | Some(PlayerState::Paused)
        );
        list.current = if has_current_track {
            np.and_then(np_to_track_meta)
        } else {
            None
        };

        // Refresh next + context from the Spotify Web API. On error we keep
        // the previous next so a transient hiccup doesn't blank "Up next".
        if has_current_track {
            let has_token = web.inner.read().await.token.is_some();
            info!(
                "poll_loop: current={:?}, fetching queue (web_connected={has_token})",
                list.current.as_ref().map(|c| &c.name)
            );
            match web.fetch_queue().await {
                Ok(Some(snap)) => {
                    let raw_upcoming = snap.upcoming.len();
                    info!(
                        "poll_loop: queue ok: context={:?}, currently_playing={:?}, raw_upcoming={raw_upcoming}",
                        snap.context_type, snap.currently_playing_id
                    );
                    // Defensive: drop the current track from `next` in case
                    // the API hasn't propagated the advance yet and still
                    // lists it as upcoming.
                    let current_id = list.current.as_ref().map(|c| c.id.clone());
                    list.next = snap
                        .upcoming
                        .into_iter()
                        .filter(|t| Some(&t.id) != current_id.as_ref())
                        .collect();
                    list.context_type = snap.context_type;
                }
                Ok(None) => {
                    info!(
                        "poll_loop: fetch_queue returned None (not connected, no playback context, or Premium required) — clearing next/context"
                    );
                    list.next.clear();
                    list.context_type = None;
                }
                Err(err) => {
                    warn!("poll_loop: queue fetch error: {err}");
                    let mut inner = web.inner.write().await;
                    inner.last_error = Some(err.to_string());
                    // Leave `next` and `context_type` as-is.
                }
            }
        } else {
            // Stopped / NotRunning — no track, no queue, nothing to show.
            info!("poll_loop: watcher reports no current track — clearing list");
            list.next.clear();
            list.context_type = None;
        }

        // Has the set of tracks-to-resolve actually changed? This gates the
        // resolver — we don't want to re-spawn work for an unchanged list.
        // We always emit the snapshot below, even when unchanged, so the
        // frontend's `receivedAt` keeps advancing; the timestamp is part of
        // the data ("yes, this is still current as of now"), and without
        // that heartbeat a paused user would see "Up next" disappear once
        // their snapshot crossed the frontend's staleness threshold.
        let new_ids = list.track_ids();
        let list_changed = new_ids != last_ids;
        if list_changed {
            info!(
                "poll_loop: playback list changed: {} -> {} tracks (current={:?}, next_count={})",
                last_ids.len(),
                new_ids.len(),
                list.current.as_ref().map(|c| &c.name),
                list.next.len()
            );
            last_ids = new_ids;
        }

        let snapshot = list.as_snapshot();
        let _ = tx.send(Some(snapshot.clone()));
        if let Err(err) = app.emit("spotify_queue", &snapshot) {
            warn!("Failed to emit spotify_queue: {err}");
        }

        if list_changed {
            // Resolve lyrics+translation for everything in the list. Dedup'd
            // across the whole session — once a track has been kicked off,
            // it's done whether it appears as current or as a future "next".
            resolve_playback_list(&app_state, &app, &list, &mut resolved).await;
        }
    }
}

#[cfg(target_os = "macos")]
fn np_to_track_meta(np: crate::app::spotify::NowPlaying) -> Option<TrackMeta> {
    Some(TrackMeta {
        id: np.track_id?,
        name: np.name?,
        artist: np.artist?,
        album: np.album,
        duration_ms: np.duration_ms?,
    })
}

/// Single resolver covering [current, ...next]. The dedup set spans the whole
/// session so a track that was preloaded as "next" doesn't get re-resolved
/// when it later becomes "current". Current is always considered (so an app
/// open that goes straight to a cached track still warms its caches and any
/// follow-up bookkeeping happens). Next is gated on a real playlist/album
/// context — for radio/autoplay the "next track" is a guess.
#[cfg(target_os = "macos")]
async fn resolve_playback_list(
    state: &Arc<crate::app::AppState>,
    app: &AppHandle,
    list: &PlaybackList,
    resolved: &mut std::collections::HashSet<String>,
) {
    let cfg = state.config.borrow().clone();
    let target_lang = cfg.target_language_id.clone();
    if target_lang.is_empty() {
        info!("resolve_playback_list: skipped — targetLanguageId is empty");
        return;
    }
    let model = cfg.model;
    let preload_count = cfg.spotify_preload_count.min(3) as usize;

    let mut to_resolve: Vec<&TrackMeta> = Vec::new();
    if let Some(c) = &list.current {
        to_resolve.push(c);
    }
    let next_eligible = matches!(
        list.context_type.as_deref(),
        Some("playlist") | Some("album")
    );
    if next_eligible {
        to_resolve.extend(list.next.iter().take(preload_count));
    }

    if to_resolve.is_empty() {
        info!(
            "resolve_playback_list: nothing to resolve (current={}, context={:?}, next_eligible={next_eligible}, next_count={}, preload_count={preload_count})",
            list.current.is_some(),
            list.context_type,
            list.next.len()
        );
        return;
    }

    info!(
        "resolve_playback_list: {} candidate(s) (next_eligible={next_eligible}, preload_count={preload_count})",
        to_resolve.len()
    );

    for (idx, track) in to_resolve.iter().enumerate() {
        let dedup_key = format!("{}|{}|{}", track.id, target_lang, model);
        if !resolved.insert(dedup_key) {
            debug!(
                "Resolve skip (already done): #{} {} — {}",
                idx,
                track.name,
                track.artist
            );
            continue;
        }
        let role = if idx == 0 { "current" } else { "next" };
        info!(
            "Resolve initiate ({role} #{}): {} — {} (track_id={}, lang={}, model={})",
            idx,
            track.name,
            track.artist,
            track.id,
            target_lang,
            model
        );
        let state = state.clone();
        let app = app.clone();
        let track = (*track).clone();
        let target_lang = target_lang.clone();
        tokio::spawn(async move {
            if let Err(err) = crate::app::lyrics::resolve_track(
                &state,
                &app,
                &track.id,
                &track.artist,
                &track.name,
                track.album.as_deref(),
                Some(track.duration_ms),
                &target_lang,
                model,
            )
            .await
            {
                warn!(
                    "Resolve failed for track={} ({}): {err}",
                    track.id, track.name
                );
            }
        });
    }
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

fn load_refresh_token() -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).ok()?;
    entry.get_password().ok()
}

fn save_refresh_token(token: &str) -> anyhow::Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)?;
    entry.set_password(token)?;
    Ok(())
}

fn delete_refresh_token() -> anyhow::Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)?;
    entry.delete_credential()?;
    Ok(())
}

// ----- Tauri commands ----------------------------------------------------

#[tauri::command]
pub async fn spotify_web_connect(
    state: tauri::State<'_, Arc<crate::app::AppState>>,
    client_id: String,
) -> Result<(), String> {
    state.spotify_web.connect(client_id).await
}

#[tauri::command]
pub async fn spotify_web_disconnect(
    state: tauri::State<'_, Arc<crate::app::AppState>>,
) -> Result<(), String> {
    state.spotify_web.disconnect().await;
    Ok(())
}

#[tauri::command]
pub async fn spotify_web_status(
    state: tauri::State<'_, Arc<crate::app::AppState>>,
) -> Result<SpotifyWebStatus, String> {
    Ok(state.spotify_web.status().await)
}

#[tauri::command]
pub async fn spotify_web_get_queue(
    state: tauri::State<'_, Arc<crate::app::AppState>>,
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
}
