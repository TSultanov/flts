//! macOS-only Spotify desktop client integration via AppleScript.
//!
//! Polls Spotify.app every ~500ms while a watcher is active and emits a
//! `spotify_state` Tauri event whenever the player state changes (track id,
//! play/pause, or non-trivial position jump).

use std::time::Duration;

use crate::app::task::ManagedTask;
use crate::host::AppHandle;
use futures_util::{StreamExt, stream::FuturesUnordered};
use log::{debug, warn};
use serde::Serialize;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, watch};
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
/// Position and duration must be coerced to integer ms inside AppleScript:
/// `(player position as text)` uses the macOS decimal separator (`125,357` under
/// a Spanish locale), which `f64::parse` rejects.
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

/// Queries Spotify.app once; errors only when osascript itself fails, leaving
/// the caller to surface or skip the tick.
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

/// Track change, play/pause, or a position jump beyond what playback between
/// polls could produce.
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

enum WatcherCommand {
    Start(AppHandle, oneshot::Sender<()>),
    Stop(oneshot::Sender<()>),
}
#[derive(Clone)]
pub struct SpotifyWatcher {
    commands: mpsc::Sender<WatcherCommand>,
    current: watch::Receiver<Option<NowPlaying>>,
}
impl SpotifyWatcher {
    pub fn new() -> (Self, ManagedTask) {
        let (commands, mut requests) = mpsc::channel(32);
        let (tx, current) = watch::channel(None);
        let keepalive = commands.clone();
        let task = ManagedTask::spawn(move |mut stop| async move {
            let _keepalive = keepalive;
            let mut app: Option<AppHandle> = None;
            let mut ticker = time::interval(POLL_INTERVAL);
            ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
            let mut queries = FuturesUnordered::new();
            loop {
                tokio::select! {
                    biased;
                    _ = stop.changed() => break,
                    request = requests.recv() => match request {
                        Some(WatcherCommand::Start(handle, reply)) => { if app.is_none() { app = Some(handle); ticker.reset_immediately(); } let _ = reply.send(()); }
                        Some(WatcherCommand::Stop(reply)) => { app = None; queries.clear(); let _ = reply.send(()); }
                        None => break,
                    },
                    _ = ticker.tick(), if app.is_some() && queries.is_empty() => { queries.push(query_once()); }
                    Some(result) = queries.next(), if !queries.is_empty() => {
                        match result {
                            Ok(next) => {
                                let significant = tx.send_if_modified(|cur: &mut Option<NowPlaying>| {
                                    let changed = cur.as_ref().is_none_or(|prev| is_significant_change(prev, &next));
                                    *cur = Some(next); changed
                                });
                                if significant && let (Some(app), Some(payload)) = (&app, tx.borrow().as_ref()) {
                                    if let Err(err) = app.emit("spotify_state", payload) { warn!("Failed to emit spotify_state: {err}"); }
                                }
                            }
                            Err(err) => debug!("Spotify poll failed: {err}"),
                        }
                    }
                }
            }
            queries.clear();
        });
        (Self { commands, current }, task)
    }
    pub fn current(&self) -> Option<NowPlaying> {
        self.current.borrow().clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<Option<NowPlaying>> {
        self.current.clone()
    }
    pub async fn start(&self, app: AppHandle) {
        let (reply, result) = oneshot::channel();
        if self
            .commands
            .send(WatcherCommand::Start(app, reply))
            .await
            .is_ok()
        {
            let _ = result.await;
        }
    }
    pub async fn stop(&self) {
        let (reply, result) = oneshot::channel();
        if self
            .commands
            .send(WatcherCommand::Stop(reply))
            .await
            .is_ok()
        {
            let _ = result.await;
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
        // A locale-formatted field must yield None, never a silent 0 in JS.
        let np = parse_line("playing|spotify:track:abc|N|A|Al|125,357|279760");
        assert_eq!(np.position_ms, None);
        assert_eq!(np.duration_ms, Some(279_760));
    }

    #[test]
    fn parse_handles_pipe_in_title() {
        // splitn(_, 7) collapses a '|' in the title into the duration field,
        // which then fails to parse.
        let np = parse_line("playing|id|some|weird|title|with|pipes");
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
