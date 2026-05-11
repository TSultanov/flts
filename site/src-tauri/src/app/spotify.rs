//! macOS-only Spotify desktop client integration via AppleScript.
//!
//! Polls Spotify.app every ~500ms while a watcher is active and emits a
//! `spotify_state` Tauri event whenever the player state changes (track id,
//! play/pause, or non-trivial position jump).

use std::sync::Arc;
use std::time::Duration;

use log::{debug, info, warn};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::process::Command;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tokio::time;

const POLL_INTERVAL: Duration = Duration::from_millis(500);
const OSASCRIPT_TIMEOUT: Duration = Duration::from_secs(2);
const POSITION_JUMP_THRESHOLD_MS: i64 = 1500;

/// AppleScript that queries Spotify.app and prints a single pipe-delimited line.
/// Output forms:
///   "notrunning"
///   "stopped"
///   "<state>|<id>|<name>|<artist>|<album>|<position_ms>|<duration_ms>"
/// where `<state>` is one of `playing`/`paused`.
///
/// Both position and duration are emitted as integer milliseconds. We do the
/// `* 1000` and `as integer` coercion in AppleScript itself rather than parsing
/// a real number, because `(player position as text)` formats with the user's
/// macOS decimal separator (e.g. `125,357` in a Spanish locale), which Rust's
/// `f64::parse` rejects.
const SPOTIFY_QUERY: &str = r#"
if application "Spotify" is running then
  tell application "Spotify"
    set s to player state as string
    if s is "stopped" then return "stopped"
    set t to current track
    return s & "|" & (id of t) & "|" & (name of t) & "|" & (artist of t) & "|" & (album of t) & "|" & ((player position * 1000) as integer) & "|" & (duration of t as integer)
  end tell
else
  return "notrunning"
end if
"#;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PlayerState {
    Playing,
    Paused,
    Stopped,
    NotRunning,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NowPlaying {
    pub state: PlayerState,
    #[serde(rename = "trackId")]
    pub track_id: Option<String>,
    pub name: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    #[serde(rename = "positionMs")]
    pub position_ms: Option<u32>,
    #[serde(rename = "durationMs")]
    pub duration_ms: Option<u32>,
}

impl NowPlaying {
    pub fn empty(state: PlayerState) -> Self {
        Self {
            state,
            track_id: None,
            name: None,
            artist: None,
            album: None,
            position_ms: None,
            duration_ms: None,
        }
    }
}

/// Query Spotify.app once. Returns `Ok(None)` only on osascript failures so the
/// caller can decide whether to surface the error or treat it as "no data this tick".
pub async fn query_once() -> anyhow::Result<NowPlaying> {
    let output = time::timeout(
        OSASCRIPT_TIMEOUT,
        Command::new("osascript")
            .arg("-e")
            .arg(SPOTIFY_QUERY)
            .output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("osascript timed out"))??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("osascript failed: {stderr}");
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(parse_line(&raw))
}

fn parse_line(raw: &str) -> NowPlaying {
    if raw == "notrunning" || raw.is_empty() {
        return NowPlaying::empty(PlayerState::NotRunning);
    }
    if raw == "stopped" {
        return NowPlaying::empty(PlayerState::Stopped);
    }

    let parts: Vec<&str> = raw.splitn(7, '|').collect();
    if parts.len() != 7 {
        warn!("Unexpected Spotify osascript output: {raw:?}");
        return NowPlaying::empty(PlayerState::NotRunning);
    }

    let state = match parts[0] {
        "playing" => PlayerState::Playing,
        "paused" => PlayerState::Paused,
        other => {
            warn!("Unknown Spotify player state: {other}");
            PlayerState::NotRunning
        }
    };

    let track_id = parts[1].to_string();
    let name = parts[2].to_string();
    let artist = parts[3].to_string();
    let album = parts[4].to_string();
    let position_ms = parts[5].parse::<u32>().ok();
    let duration_ms = parts[6].parse::<u32>().ok();

    NowPlaying {
        state,
        track_id: Some(track_id),
        name: Some(name),
        artist: Some(artist),
        album: Some(album),
        position_ms,
        duration_ms,
    }
}

/// Returns true if the two states differ in a way the UI cares about:
/// track change, play/pause, or position jump (>1.5s — beyond what natural
/// playback between polls would produce).
fn is_significant_change(prev: &NowPlaying, next: &NowPlaying) -> bool {
    if prev.state != next.state {
        return true;
    }
    if prev.track_id != next.track_id {
        return true;
    }
    match (prev.position_ms, next.position_ms) {
        (Some(a), Some(b)) => (a as i64 - b as i64).abs() > POSITION_JUMP_THRESHOLD_MS,
        _ => false,
    }
}

pub struct SpotifyWatcher {
    handle: Mutex<Option<JoinHandle<()>>>,
    tx: Arc<watch::Sender<Option<NowPlaying>>>,
}

impl Default for SpotifyWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl SpotifyWatcher {
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(None);
        Self {
            handle: Mutex::new(None),
            tx: Arc::new(tx),
        }
    }

    pub fn current(&self) -> Option<NowPlaying> {
        self.tx.borrow().clone()
    }

    pub async fn start(&self, app: AppHandle) {
        let mut handle_guard = self.handle.lock().await;
        if handle_guard.is_some() {
            debug!("SpotifyWatcher already running");
            return;
        }

        let tx = self.tx.clone();
        let task = tokio::spawn(async move {
            let mut ticker = time::interval(POLL_INTERVAL);
            ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let next = match query_once().await {
                    Ok(np) => np,
                    Err(err) => {
                        debug!("Spotify poll failed: {err}");
                        continue;
                    }
                };

                let significant = tx.send_if_modified(|cur| {
                    let sig = cur
                        .as_ref()
                        .map(|prev| is_significant_change(prev, &next))
                        .unwrap_or(true);
                    *cur = Some(next);
                    sig
                });

                if significant
                    && let Some(payload) = tx.borrow().as_ref()
                    && let Err(err) = app.emit("spotify_state", payload)
                {
                    warn!("Failed to emit spotify_state: {err}");
                }
            }
        });
        *handle_guard = Some(task);
        info!("SpotifyWatcher started");
    }

    pub async fn stop(&self) {
        let mut handle_guard = self.handle.lock().await;
        if let Some(task) = handle_guard.take() {
            task.abort();
            info!("SpotifyWatcher stopped");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_not_running() {
        let np = parse_line("notrunning");
        assert_eq!(np.state, PlayerState::NotRunning);
        assert!(np.track_id.is_none());
    }

    #[test]
    fn parse_stopped() {
        let np = parse_line("stopped");
        assert_eq!(np.state, PlayerState::Stopped);
    }

    #[test]
    fn parse_playing() {
        let np =
            parse_line("playing|spotify:track:abc|Song Name|Artist Name|Album Name|12345|234567");
        assert_eq!(np.state, PlayerState::Playing);
        assert_eq!(np.track_id.as_deref(), Some("spotify:track:abc"));
        assert_eq!(np.name.as_deref(), Some("Song Name"));
        assert_eq!(np.artist.as_deref(), Some("Artist Name"));
        assert_eq!(np.album.as_deref(), Some("Album Name"));
        assert_eq!(np.position_ms, Some(12_345));
        assert_eq!(np.duration_ms, Some(234_567));
    }

    #[test]
    fn parse_paused() {
        let np = parse_line("paused|spotify:track:xyz|N|A|Al|0|123456");
        assert_eq!(np.state, PlayerState::Paused);
        assert_eq!(np.position_ms, Some(0));
    }

    #[test]
    fn parse_rejects_locale_specific_decimal_fragments() {
        // Regression: earlier the script returned `player position as text`,
        // which formats with the macOS decimal separator (e.g. "125,357" in a
        // Spanish locale). We now expect a pure integer millisecond field; if
        // anything decimal-looking sneaks through, it must parse to None
        // rather than silently coerce to 0 on the JS side.
        let np = parse_line("playing|spotify:track:abc|N|A|Al|125,357|279760");
        assert_eq!(np.position_ms, None);
        assert_eq!(np.duration_ms, Some(279_760));
    }

    #[test]
    fn parse_handles_pipe_in_title() {
        // splitn(_, 7) collapses the rest into duration. Verifies we don't accidentally
        // mis-parse when a Spotify track has '|' in its name (rare, but possible).
        let np = parse_line("playing|id|some|weird|title|with|pipes");
        // 7 fields produced, last is "pipes" which won't parse as u32 → duration None.
        assert_eq!(np.state, PlayerState::Playing);
        assert_eq!(np.duration_ms, None);
    }

    #[test]
    fn significant_change_detects_track_id() {
        let mut a = parse_line("playing|t1|n|a|al|1.0|180000");
        let b = parse_line("playing|t2|n|a|al|1.0|180000");
        assert!(is_significant_change(&a, &b));
        a.position_ms = Some(1000);
        let mut c = a.clone();
        c.position_ms = Some(1400);
        assert!(
            !is_significant_change(&a, &c),
            "natural advance not significant"
        );
        c.position_ms = Some(20_000);
        assert!(is_significant_change(&a, &c), "big jump is significant");
    }
}
