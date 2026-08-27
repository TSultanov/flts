//! First-party Spotify lyrics from the *running desktop client*, no remote
//! API credentials.
//!
//! Spotify's desktop app is a CEF (Chromium Embedded Framework) host whose
//! webview fetches lyrics itself from
//! `https://spclient.wg.spotify.com/color-lyrics/v2/track/{id}` — the native
//! side injects the session's internal access token into the webview's
//! request transport. We can't call that endpoint directly (it rejects public
//! OAuth tokens, and hammering it from third-party clients risks account
//! bans), but we don't need to: if Spotify is launched with
//! `--remote-debugging-port=<port>`, CEF exposes the Chrome DevTools Protocol
//! on loopback, and we can evaluate JS *inside Spotify's own webview*. The
//! request is then made by Spotify's signed client with Spotify's token —
//! indistinguishable from the lyrics pane opening on its own.
//!
//! Flow (see `docs/spotify-lyrics-HANDOFF.md` for the reverse engineering):
//! 1. `GET /json/list` on the CDP port, pick the `xpui.app.spotify.com` page.
//! 2. Open its `webSocketDebuggerUrl`, send `Runtime.evaluate`.
//! 3. The evaluated script captures the webpack `require` via the rspack
//!    chunk-push trap door, resolves the app's request-builder module, and
//!    replays the color-lyrics call. A 404 from Spotify maps to "no lyrics".
//!
//! Everything is loopback; nothing leaves the machine. If Spotify was not
//! launched with the flag, [`lyrics_for_track`] fails fast with `Ok(None)`
//! and [`spotify_restart_with_devtools`] can relaunch it (user-initiated).

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use log::{debug, info};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use library::lyrics::spotify::LyricsResponse;

/// CEF's conventional DevTools port. Override with `FLTS_SPOTIFY_CDP_PORT`.
const DEFAULT_CDP_PORT: u16 = 9222;

/// The desktop webview's main document origin (the xpui bundle).
const TARGET_URL_FRAGMENT: &str = "xpui.app.spotify.com";

const HTTP_TIMEOUT: Duration = Duration::from_secs(3);
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Generous: the page does its own token refresh + network round trip.
const EVAL_TIMEOUT: Duration = Duration::from_secs(20);
const RESTART_QUIT_TIMEOUT: Duration = Duration::from_secs(10);
const RESTART_READY_TIMEOUT: Duration = Duration::from_secs(25);

// ----- public API ---------------------------------------------------------

/// UI-facing availability snapshot for the DevTools bridge.
#[derive(Debug, Clone, Serialize)]
pub struct SpotifyCdpStatus {
    pub available: bool,
    pub port: u16,
    /// Human-readable explanation when `available` is false.
    pub hint: Option<String>,
}

/// Fetches Spotify's own (line-synced, Musixmatch-class) lyrics for a track
/// by asking the *running Spotify desktop client's webview* to make the
/// request through its authenticated transport.
///
/// `Ok(None)` covers every benign case: devtools not enabled, Spotify not
/// running, the webview still loading, or Spotify reporting "no lyrics for
/// this track" (404). Errors are reserved for protocol-level surprises.
pub async fn lyrics_for_track(track_id: &str) -> anyhow::Result<Option<library::lyrics::Lyrics>> {
    let Some(bare) = bare_track_id(track_id) else {
        debug!("cdp: not a spotify track id: {track_id:?}");
        return Ok(None);
    };

    let port = cdp_port();
    if !devtools_ready(port).await {
        log_unavailable_once(port);
        return Ok(None);
    }

    let Some(target) = find_target(port).await? else {
        // Spotify is up but the xpui document isn't there (still booting, or
        // sitting on the login screen). Not an error; caller retries later.
        debug!("cdp: no {TARGET_URL_FRAGMENT} target yet");
        return Ok(None);
    };

    match eval_lyrics(port, &target, &bare).await {
        Ok(Outcome::Lyrics(payload)) => Ok(Some(library::lyrics::spotify::parse_lyrics(
            track_id, payload,
        ))),
        Ok(Outcome::NoLyrics) => Ok(None),
        Ok(Outcome::PageError(err)) => Err(anyhow!("spotify webview: {err}")),
        Err(err) => Err(err),
    }
}

/// True when Spotify listens on the DevTools port.
async fn devtools_ready(port: u16) -> bool {
    client()
        .get(format!("http://127.0.0.1:{port}/json/version"))
        .timeout(Duration::from_millis(400))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success())
}

// ----- /json/list target discovery ----------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct DevToolsTarget {
    #[serde(rename = "type")]
    target_type: String,
    url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    web_socket_debugger_url: Option<String>,
}

fn select_target(targets: &[DevToolsTarget]) -> Option<&DevToolsTarget> {
    targets
        .iter()
        .find(|t| t.target_type == "page" && t.url.contains(TARGET_URL_FRAGMENT))
        .filter(|t| t.web_socket_debugger_url.is_some())
}

async fn find_target(port: u16) -> anyhow::Result<Option<DevToolsTarget>> {
    let resp: Value = client()
        .get(format!("http://127.0.0.1:{port}/json/list"))
        .timeout(HTTP_TIMEOUT)
        .send()
        .await?
        .json()
        .await
        .context("CDP /json/list")?;
    let targets: Vec<DevToolsTarget> =
        serde_json::from_value(resp).context("CDP /json/list shape")?;
    Ok(select_target(&targets).cloned())
}

// ----- CDP WebSocket evaluation --------------------------------------------

/// What the in-page script reported back.
#[derive(Debug)]
enum Outcome {
    Lyrics(LyricsResponse),
    NoLyrics,
    /// The script ran but could not complete (build drift, endpoint change).
    PageError(String),
}

async fn eval_lyrics(
    port: u16,
    target: &DevToolsTarget,
    bare_track_id: &str,
) -> anyhow::Result<Outcome> {
    let expression = build_lyrics_expression(bare_track_id)
        .ok_or_else(|| anyhow!("cdp: refusing unsafe track id {bare_track_id:?}"))?;
    let ws_url = target
        .web_socket_debugger_url
        .clone()
        .unwrap_or_else(|| format!("ws://127.0.0.1:{port}/devtools/page/missing"));
    let value = time_limited_eval(&ws_url, &expression).await?;
    parse_outcome(value)
}

/// Connects, evaluates one expression, returns its by-value result.
async fn time_limited_eval(ws_url: &str, expression: &str) -> anyhow::Result<Value> {
    let fut = eval_on_ws(ws_url, expression);
    match tokio::time::timeout(EVAL_TIMEOUT, fut).await {
        Ok(res) => res,
        Err(_) => bail!("CDP eval timed out after {EVAL_TIMEOUT:?}"),
    }
}

async fn eval_on_ws(ws_url: &str, expression: &str) -> anyhow::Result<Value> {
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message;

    let (mut ws, _resp) =
        tokio::time::timeout(WS_CONNECT_TIMEOUT, tokio_tungstenite::connect_async(ws_url))
            .await
            .map_err(|_| anyhow!("CDP connect timed out"))?
            .map_err(|e| anyhow!("CDP connect failed: {e}"))?;

    let request = serde_json::json!({
        "id": 1,
        "method": "Runtime.evaluate",
        "params": {
            "expression": expression,
            "awaitPromise": true,
            "returnByValue": true,
        },
    });
    ws.send(Message::Text(request.to_string().into()))
        .await
        .map_err(|e| anyhow!("CDP send failed: {e}"))?;

    // Events and unrelated responses arrive interleaved; wait for id == 1.
    while let Some(msg) = ws.next().await {
        let msg = msg.map_err(|e| anyhow!("CDP recv failed: {e}"))?;
        let Ok(text) = msg.into_text() else {
            continue; // pings, binary frames
        };
        let Ok(payload) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if payload.get("id").and_then(Value::as_i64) != Some(1) {
            continue;
        }
        return extract_eval_value(&payload);
    }
    bail!("CDP closed before a response arrived")
}

/// `{id, result: {result: {type, value}}}` (+ optional `exceptionDetails`).
fn extract_eval_value(response: &Value) -> anyhow::Result<Value> {
    if let Some(details) = response.get("exceptionDetails") {
        let desc = details
            .pointer("/exception/description")
            .and_then(Value::as_str)
            .unwrap_or("unknown exception");
        // First line is "TypeError: msg" — enough context without the stack.
        bail!("page eval threw: {}", desc.lines().next().unwrap_or(desc));
    }
    response
        .pointer("/result/result/value")
        .cloned()
        .ok_or_else(|| anyhow!("CDP response had no by-value result"))
}

fn parse_outcome(value: Value) -> anyhow::Result<Outcome> {
    // Shape produced by LYRICS_EXPRESSION: {"status":"ok"|"no_lyrics"|"error", ...}
    match value.get("status").and_then(Value::as_str) {
        Some("ok") => {
            let payload: LyricsResponse = serde_json::from_value(
                value
                    .get("lyrics")
                    .cloned()
                    .ok_or_else(|| anyhow!("ok outcome without lyrics payload"))?,
            )
            .context("color-lyrics payload shape")?;
            Ok(Outcome::Lyrics(payload))
        }
        Some("no_lyrics") => Ok(Outcome::NoLyrics),
        Some("error") => Ok(Outcome::PageError(
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown in-page error")
                .to_string(),
        )),
        other => Err(anyhow!(
            "unexpected in-page outcome status: {other:?} (Spotify build changed?)"
        )),
    }
}

// ----- the in-page script --------------------------------------------------

/// Evaluated inside Spotify's webview. Runs as a strict IIFE so nothing leaks
/// into the page beyond the captured `__fltsReq`. `__TRACK_ID__` is replaced
/// with a validated JSON string literal by [`build_lyrics_expression`].
const LYRICS_EXPRESSION: &str = r#"(async (trackId) => {
  const fail = (error) => ({ status: 'error', error: String(error) });
  try {
    if (!window.__fltsReq) {
      const chunkKey = Object.keys(window).find((k) => /^rspackChunk/.test(k));
      if (!chunkKey) return fail('no rspack runtime at ' + location.href);
      window[chunkKey].push([[900000000 + Math.floor(Math.random() * 99999999)], {}, (r) => { window.__fltsReq = r; }]);
    }
    const req = window.__fltsReq;
    if (!req || typeof req !== 'function') return fail('webpack require not captured');
    let mod = null;
    try {
      const m = req(22358);
      if (m && m.n && typeof m.n.getInstance === 'function') mod = m.n;
    } catch (e) {}
    if (!mod) {
      const factories = req.m || {};
      for (const id of Object.keys(factories)) {
        let src;
        try { src = String(factories[id]); } catch (e) { continue; }
        if (src.indexOf('withEndpointIdentifier') === -1) continue;
        if (src.indexOf('getInstance') === -1) continue;
        try {
          const m = req(id);
          if (m && m.n && typeof m.n.getInstance === 'function') { mod = m.n; break; }
        } catch (e) {}
      }
    }
    if (!mod) return fail('lyrics API module not found (Spotify build changed?)');
    const resp = await mod.getInstance().build()
      .withHost('https://spclient.wg.spotify.com/color-lyrics/v2')
      .withPath('/track/' + trackId)
      .withQueryParameters({ format: 'json', vocalRemoval: 'false', market: 'from_token' })
      .withEndpointIdentifier('/track/{trackId}')
      .send();
    const lyrics = resp && resp.body && resp.body.lyrics;
    if (!lyrics || !Array.isArray(lyrics.lines)) return fail('unexpected response shape');
    return { status: 'ok', lyrics: { syncType: lyrics.syncType, lines: lyrics.lines } };
  } catch (e) {
    if (e && (e.status === 404 || e.status === '404')) return { status: 'no_lyrics' };
    return fail((e && e.message) || e);
  }
})(__TRACK_ID__)"#;

/// Spotify track ids are base62. Anything else (injection attempt, episode
/// id, local file) is rejected before it can reach the page.
fn bare_track_id(id: &str) -> Option<String> {
    let bare = id.strip_prefix("spotify:track:").unwrap_or(id);
    if bare.is_empty() || bare.len() > 64 || !bare.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    Some(bare.to_string())
}

fn build_lyrics_expression(bare_id: &str) -> Option<String> {
    bare_track_id(bare_id).map(|id| LYRICS_EXPRESSION.replace("__TRACK_ID__", &format!("\"{id}\"")))
}

// ----- infrastructure -------------------------------------------------------

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .expect("reqwest client builds with default config")
    })
}

fn cdp_port() -> u16 {
    static PORT: OnceLock<u16> = OnceLock::new();
    *PORT.get_or_init(|| {
        std::env::var("FLTS_SPOTIFY_CDP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|p| *p > 0)
            .unwrap_or(DEFAULT_CDP_PORT)
    })
}

/// Warn about the missing bridge at most once per availability transition so
/// a long session without the flag doesn't spam the log on every track.
fn log_unavailable_once(port: u16) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        info!(
            "Spotify lyrics bridge unavailable (no DevTools endpoint on 127.0.0.1:{port}). \
             Relaunch Spotify with --remote-debugging-port={port} to enable first-party lyrics."
        );
    }
}

// ----- restart command ------------------------------------------------------

#[tauri::command]
pub async fn spotify_cdp_status() -> Result<SpotifyCdpStatus, String> {
    let port = cdp_port();
    if devtools_ready(port).await {
        return Ok(SpotifyCdpStatus {
            available: true,
            port,
            hint: None,
        });
    }
    Ok(SpotifyCdpStatus {
        available: false,
        port,
        hint: Some(if spotify_running().await {
            "Spotify is running without the lyrics bridge — restart it below.".to_string()
        } else {
            "Spotify is not running.".to_string()
        }),
    })
}

/// Relaunches Spotify with the DevTools flag so the lyrics bridge can attach.
/// No-op when the bridge is already up; quits a running Spotify first (its
/// session and playback position are restored on relaunch).
#[tauri::command]
pub async fn spotify_restart_with_devtools() -> Result<SpotifyCdpStatus, String> {
    let port = cdp_port();
    if devtools_ready(port).await {
        return Ok(SpotifyCdpStatus {
            available: true,
            port,
            hint: None,
        });
    }

    if spotify_running().await {
        let _ = tokio::process::Command::new("osascript")
            .args(["-e", r#"tell application "Spotify" to quit"#])
            .output()
            .await;
        if !wait_for(|| async { !spotify_running().await }, RESTART_QUIT_TIMEOUT).await {
            return Err("Spotify did not quit in time; close it and retry".to_string());
        }
    }

    tokio::process::Command::new("open")
        .args([
            "-a",
            "Spotify",
            "--args",
            &format!("--remote-debugging-port={port}"),
        ])
        .output()
        .await
        .map_err(|e| format!("failed to launch Spotify: {e}"))?;

    if !wait_for(
        || async { devtools_ready(port).await },
        RESTART_READY_TIMEOUT,
    )
    .await
    {
        return Err("Spotify started but the DevTools endpoint never came up".to_string());
    }
    info!("Spotify relaunched with DevTools bridge on port {port}");
    Ok(SpotifyCdpStatus {
        available: true,
        port,
        hint: None,
    })
}

// ----- login agent ----------------------------------------------------------

/// Reverse-DNS label of the LaunchAgent that starts Spotify with the flag.
const LOGIN_AGENT_LABEL: &str = "com.flts.spotify-devtools";

/// Whether a login agent that relaunches Spotify with the bridge is installed.
#[derive(Debug, Clone, Serialize)]
pub struct SpotifyLoginAgentStatus {
    pub installed: bool,
    pub path: String,
}

fn login_agent_path() -> Result<std::path::PathBuf, String> {
    let base = directories::BaseDirs::new().ok_or("no home directory")?;
    Ok(base
        .home_dir()
        .join("Library/LaunchAgents")
        .join(format!("{LOGIN_AGENT_LABEL}.plist")))
}

fn login_agent_status_at(path: &std::path::Path) -> SpotifyLoginAgentStatus {
    SpotifyLoginAgentStatus {
        installed: path.exists(),
        path: path.display().to_string(),
    }
}

#[tauri::command]
pub async fn spotify_login_agent_status() -> Result<SpotifyLoginAgentStatus, String> {
    let path = login_agent_path()?;
    Ok(login_agent_status_at(&path))
}

/// Installs a per-user LaunchAgent that opens Spotify with the DevTools flag at
/// login. Spotify's own "open at login" helper lives inside its signed bundle
/// and can't carry the flag, so the user must turn that one off to avoid two
/// launches racing.
#[tauri::command]
pub async fn spotify_install_login_agent() -> Result<SpotifyLoginAgentStatus, String> {
    let port = cdp_port();
    let path = login_agent_path()?;
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    }
    tokio::fs::write(&path, login_agent_plist(port))
        .await
        .map_err(|e| format!("failed to write {}: {e}", path.display()))?;

    // Replace any earlier revision; a first install has nothing to bootout.
    let _ = launchctl(&["bootout", &domain_target()]).await;
    let out = launchctl(&["bootstrap", &gui_domain(), &path.display().to_string()]).await?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("launchctl bootstrap failed: {err}"));
    }
    info!("installed Spotify login agent at {}", path.display());
    Ok(login_agent_status_at(&path))
}

#[tauri::command]
pub async fn spotify_remove_login_agent() -> Result<SpotifyLoginAgentStatus, String> {
    let path = login_agent_path()?;
    let _ = launchctl(&["bootout", &domain_target()]).await;
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("failed to remove {}: {e}", path.display())),
    }
    Ok(login_agent_status_at(&path))
}

/// launchctl addresses per-user domains by numeric uid; `id -u` avoids a
/// unix-only dependency in this otherwise platform-agnostic module.
fn gui_domain() -> String {
    static UID: OnceLock<String> = OnceLock::new();
    let uid = UID.get_or_init(|| {
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    });
    format!("gui/{uid}")
}

fn domain_target() -> String {
    format!("{}/{LOGIN_AGENT_LABEL}", gui_domain())
}

async fn launchctl(args: &[&str]) -> Result<std::process::Output, String> {
    tokio::process::Command::new("launchctl")
        .args(args)
        .output()
        .await
        .map_err(|e| format!("launchctl failed: {e}"))
}

fn login_agent_plist(port: u16) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LOGIN_AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/bin/open</string>
        <string>-a</string>
        <string>Spotify</string>
        <string>--args</string>
        <string>--remote-debugging-port={port}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
</dict>
</plist>
"#
    )
}

async fn spotify_running() -> bool {
    tokio::process::Command::new("pgrep")
        .args(["-x", "Spotify"])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Polls an async condition every 500ms until true or the deadline passes.
async fn wait_for<F, Fut>(mut cond: F, timeout: Duration) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if cond().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_track_id_accepts_uri_and_bare_forms() {
        assert_eq!(
            bare_track_id("spotify:track:1liIvvrfQgYNjIcg5Qh16V").as_deref(),
            Some("1liIvvrfQgYNjIcg5Qh16V")
        );
        assert_eq!(
            bare_track_id("1liIvvrfQgYNjIcg5Qh16V").as_deref(),
            Some("1liIvvrfQgYNjIcg5Qh16V")
        );
    }

    #[test]
    fn bare_track_id_rejects_garbage() {
        assert_eq!(bare_track_id(""), None);
        assert_eq!(bare_track_id("spotify:track:"), None);
        // Injection attempts must never reach the page as a literal.
        assert_eq!(bare_track_id(r#"abc") || fail('pwned"#), None);
        assert_eq!(bare_track_id("spotify:episode:abc"), None);
        assert_eq!(bare_track_id(&"a".repeat(65)), None);
    }

    #[test]
    fn expression_embeds_validated_id_only() {
        let expr = build_lyrics_expression("1liIvvrfQgYNjIcg5Qh16V").unwrap();
        assert!(expr.contains(r#"("1liIvvrfQgYNjIcg5Qh16V")"#));
        assert!(expr.contains("color-lyrics/v2"));
        assert!(expr.contains("rspackChunk"));
        assert!(expr.contains("withEndpointIdentifier"));
        // The fallback module scan is present for build drift.
        assert!(expr.contains("req.m"));
        assert!(build_lyrics_expression("no;thanks()").is_none());
    }

    #[test]
    fn selects_xpui_page_target() {
        let json = r#"[
          {"type":"page","url":"https://xpui.app.spotify.com/index.html","webSocketDebuggerUrl":"ws://x/1"},
          {"type":"page","url":"https://accounts.spotify.com/","webSocketDebuggerUrl":"ws://x/2"},
          {"type":"iframe","url":"https://xpui.app.spotify.com/embed","webSocketDebuggerUrl":"ws://x/3"}
        ]"#;
        let targets: Vec<DevToolsTarget> = serde_json::from_str(json).unwrap();
        let t = select_target(&targets).unwrap();
        assert_eq!(t.web_socket_debugger_url.as_deref(), Some("ws://x/1"));
    }

    #[test]
    fn skips_target_without_ws_url() {
        let json = r#"[{"type":"page","url":"https://xpui.app.spotify.com/index.html"}]"#;
        let targets: Vec<DevToolsTarget> = serde_json::from_str(json).unwrap();
        assert!(select_target(&targets).is_none());
    }

    #[test]
    fn extracts_by_value_result() {
        let resp: Value = serde_json::from_str(
            r#"{"id":1,"result":{"result":{"type":"object","value":{"status":"no_lyrics"}}}}"#,
        )
        .unwrap();
        let v = extract_eval_value(&resp).unwrap();
        assert_eq!(v["status"], "no_lyrics");
    }

    #[test]
    fn surfaces_eval_exceptions() {
        let resp: Value = serde_json::from_str(
            r#"{"id":1,"result":{},"exceptionDetails":{"exception":{"description":"TypeError: x is not a function\n at foo"}}}"#,
        )
        .unwrap();
        let err = extract_eval_value(&resp).unwrap_err().to_string();
        assert_eq!(err, "page eval threw: TypeError: x is not a function");
    }

    #[test]
    fn parses_outcomes() {
        let ok: Value = serde_json::from_str(
            r#"{"status":"ok","lyrics":{"syncType":"LINE_SYNCED","lines":[{"startTimeMs":"960","words":"One, two"}]}}"#,
        )
        .unwrap();
        match parse_outcome(ok).unwrap() {
            Outcome::Lyrics(p) => {
                assert_eq!(p.sync_type, "LINE_SYNCED");
                assert_eq!(p.lines.len(), 1);
            }
            _ => panic!("expected lyrics"),
        }
        assert!(matches!(
            parse_outcome(serde_json::json!({"status":"no_lyrics"})).unwrap(),
            Outcome::NoLyrics
        ));
        assert!(matches!(
            parse_outcome(serde_json::json!({"status":"error","error":"boom"})).unwrap(),
            Outcome::PageError(msg) if msg == "boom"
        ));
        assert!(parse_outcome(serde_json::json!({"status":"???"})).is_err());
    }

    /// Live round trip against a Spotify already running with
    /// `--remote-debugging-port`. Run manually:
    /// `FLTS_CDP_LIVE=1 cargo test -p app spotify_cdp_live -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn spotify_cdp_live() {
        if std::env::var("FLTS_CDP_LIVE").as_deref() != Ok("1") {
            return;
        }
        let track = std::env::var("FLTS_CDP_LIVE_TRACK")
            .unwrap_or_else(|_| "spotify:track:1liIvvrfQgYNjIcg5Qh16V".to_string());
        let lyrics = lyrics_for_track(&track)
            .await
            .expect("live lyrics fetch")
            .expect("track is known to have synced lyrics");
        assert!(lyrics.synced, "expected LINE_SYNCED lyrics");
        assert!(!lyrics.lines.is_empty());
        info!(
            "live lyrics: {} lines, first: {:?}",
            lyrics.lines.len(),
            lyrics.lines[0].text
        );
    }
}
