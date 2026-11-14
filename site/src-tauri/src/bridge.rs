//! Headless WS replacement for the webview IPC channel (E2E only).
//!
//! Frames are JSON text: `{"id":n,"cmd":"...","args":{...}}` in,
//! `{"id":n,"ok":...}` / `{"id":n,"err":...}` out. `args` keys are camelCase
//! because Tauri's IPC produces that from snake_case Rust params.

use std::io::Write;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use log::{info, warn};
use serde_json::{Value, json};
use tauri::{AppHandle, Listener, Manager};
use tokio::sync::mpsc;

/// Bridge-only commands with no `generate_handler!` counterpart: entry points
/// production reaches only through a driver the harness cannot run headlessly
/// (the Spotify poller). They call the same backend functions production does.
pub const E2E_ONLY_COMMANDS: &[&str] = &["e2e_resolve_track", "e2e_eval"];

/// Scripts run in the app's own webview (`e2e_eval`), waiting for the value
/// each posts back to `/eval/{id}`.
static EVALS: std::sync::Mutex<Vec<(u64, tokio::sync::oneshot::Sender<Value>)>> =
    std::sync::Mutex::new(Vec::new());
static NEXT_EVAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

pub use app_core::dispatch::{COMMANDS, FORWARDED_EVENTS};

/// Binds `127.0.0.1:port` (0 = ephemeral) and announces the real port on stdout.
pub fn spawn(app: AppHandle, port: u16) {
    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => l,
            Err(err) => {
                // stderr too: otherwise the harness sees only a missing stdout line.
                eprintln!("FLTS_E2E_BRIDGE_ERROR bind failed: {err}");
                warn!("e2e bridge: bind failed: {err}");
                return;
            }
        };
        let actual = match listener.local_addr() {
            Ok(addr) => addr.port(),
            Err(err) => {
                eprintln!("FLTS_E2E_BRIDGE_ERROR local_addr failed: {err}");
                warn!("e2e bridge: local_addr failed: {err}");
                return;
            }
        };

        PORT.store(actual, std::sync::atomic::Ordering::Relaxed);
        println!("FLTS_E2E_BRIDGE_LISTENING {}", json!({ "port": actual }));
        let _ = std::io::stdout().flush();
        info!("e2e bridge listening on 127.0.0.1:{actual}/bridge");

        let router = axum::Router::new()
            .route("/bridge", get(upgrade))
            .route("/eval/{id}", axum::routing::post(eval_result))
            .with_state(app);
        if let Err(err) = axum::serve(listener, router).await {
            warn!("e2e bridge: serve ended: {err}");
        }
    });
}

async fn eval_result(axum::extract::Path(id): axum::extract::Path<u64>, body: String) {
    let waiting = {
        let mut evals = EVALS.lock().unwrap();
        evals
            .iter()
            .position(|(waiting, _)| *waiting == id)
            .map(|at| evals.swap_remove(at).1)
    };
    if let Some(reply) = waiting {
        let value = serde_json::from_str(&body).unwrap_or(Value::String(body));
        let _ = reply.send(value);
    }
}

/// Runs `js` (a function body; its return value may be a promise) in the
/// app's webview and returns `{ok}` or `{err}` as the page reported it.
async fn eval_in_webview(app: &AppHandle, js: &str) -> Result<Value, Value> {
    let window = app
        .webview_windows()
        .into_values()
        .next()
        .ok_or_else(|| json!("no webview: start the app with FLTS_E2E_WINDOW_URL"))?;
    let id = NEXT_EVAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let port = PORT.load(std::sync::atomic::Ordering::Relaxed);
    let (reply, result) = tokio::sync::oneshot::channel();
    EVALS.lock().unwrap().push((id, reply));
    let script = format!(
        "(async () => {{ let r; try {{ r = {{ ok: (await (async () => {{ {js}\n }})()) ?? null }}; }} \
         catch (e) {{ r = {{ err: String(e && e.stack || e) }}; }} \
         fetch('http://127.0.0.1:{port}/eval/{id}', {{ method: 'POST', mode: 'no-cors', \
         body: JSON.stringify(r === undefined ? null : r) }}); }})()"
    );
    window
        .eval(script)
        .map_err(|err| json!(format!("eval failed: {err}")))?;
    match tokio::time::timeout(std::time::Duration::from_secs(60), result).await {
        Ok(Ok(Value::Object(mut r))) if r.contains_key("ok") => Ok(r.remove("ok").unwrap()),
        Ok(Ok(other)) => Err(other),
        _ => {
            EVALS.lock().unwrap().retain(|(waiting, _)| *waiting != id);
            Err(json!("the webview did not answer"))
        }
    }
}

async fn upgrade(
    ws: WebSocketUpgrade,
    axum::extract::State(app): axum::extract::State<AppHandle>,
) -> Response {
    ws.on_upgrade(move |socket| serve_conn(socket, app))
}

async fn serve_conn(socket: WebSocket, app: AppHandle) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let writer = tauri::async_runtime::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    // `listen_any` (EventTarget::Any) receives emits regardless of their target.
    let listeners: Vec<_> = FORWARDED_EVENTS
        .iter()
        .map(|name| {
            let tx = tx.clone();
            app.listen_any(*name, move |ev| {
                let _ = tx.send(event_frame(name, ev.payload()));
            })
        })
        .collect();

    while let Some(Ok(msg)) = stream.next().await {
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => break,
            _ => continue,
        };
        let frame: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(err) => {
                warn!("e2e bridge: unparseable frame: {err}");
                continue;
            }
        };
        let id = frame.get("id").cloned().unwrap_or(Value::Null);
        let cmd = frame
            .get("cmd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let args = frame.get("args").cloned().unwrap_or_else(|| json!({}));

        // Own task per command: a slow command must not stall the read loop.
        let app = app.clone();
        let tx = tx.clone();
        tauri::async_runtime::spawn(async move {
            let reply = match dispatch(&app, &cmd, args).await {
                Ok(ok) => json!({ "id": id, "ok": ok }),
                Err(err) => json!({ "id": id, "err": err }),
            };
            let _ = tx.send(reply.to_string());
        });
    }

    for id in listeners {
        app.unlisten(id);
    }
    drop(tx);
    let _ = writer.await;
}

/// Raw Tauri payloads are JSON; anything else is passed through as a string.
fn event_frame(name: &str, payload: &str) -> String {
    let payload = serde_json::from_str::<Value>(payload)
        .unwrap_or_else(|_| Value::String(payload.to_string()));
    json!({ "event": name, "payload": payload }).to_string()
}

use app_core::args;
use app_core::dispatch::wrap;

async fn dispatch(app: &AppHandle, cmd: &str, args: Value) -> Result<Value, Value> {
    let core = app.state::<app_core::AppState>().inner().clone();

    match cmd {
        // --- bridge-only (see E2E_ONLY_COMMANDS) ---
        "e2e_eval" => {
            let js = args!(args, { js: String });
            eval_in_webview(app, &js).await
        }
        "e2e_resolve_track" => {
            let (track_id, name, artist, album, duration_ms, target_lang, model) = args!(args, {
                track_id: String, name: String, artist: String, album: Option<String>,
                duration_ms: u32, target_lang: String, model: String
            });
            let track = app_core::app::spotify::web::TrackMeta {
                id: track_id,
                name,
                artist,
                album,
                duration_ms,
            };
            wrap(
                app_core::app::lyrics::resolve_track(
                    &core,
                    &core.host(),
                    &track,
                    &target_lang,
                    model,
                )
                .await
                .map_err(|e| e.to_string()),
            )
        }

        // The harness must never restart Spotify or write LaunchAgents.
        cmd if app_core::dispatch::HOST_ONLY_COMMANDS.contains(&cmd) => {
            Err(json!(format!("not available over the bridge: {cmd}")))
        }
        _ => app_core::dispatch::dispatch(&core, cmd, args).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e2e_only_commands_are_not_production_commands() {
        for cmd in E2E_ONLY_COMMANDS {
            assert!(
                !COMMANDS.contains(cmd) && !app_core::dispatch::HOST_ONLY_COMMANDS.contains(cmd),
                "e2e-only command shadows a real one: {cmd}"
            );
        }
    }

    #[test]
    fn event_frames_unwrap_json_payloads() {
        let frame: Value = serde_json::from_str(&event_frame("book_updated", "{\"a\":1}")).unwrap();
        assert_eq!(frame["event"], "book_updated");
        assert_eq!(frame["payload"], json!({"a": 1}));
    }

    #[test]
    fn event_frames_fall_back_to_string_payloads() {
        let frame: Value = serde_json::from_str(&event_frame("book_updated", "not json")).unwrap();
        assert_eq!(frame["payload"], json!("not json"));
    }
}
