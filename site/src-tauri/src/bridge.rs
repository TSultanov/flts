//! Headless WS replacement for the webview IPC channel (E2E only).
//!
//! Frames are JSON text: client sends `{"id":n,"cmd":"...","args":{...}}`,
//! server replies `{"id":n,"ok":...}` or `{"id":n,"err":...}`. `args` keys are
//! camelCase because that is what Tauri's IPC produces from snake_case Rust
//! params; the per-command structs below replicate that mapping.

use std::io::Write;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use log::{info, warn};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;
use uuid::Uuid;

use library::epub_importer::EpubBook;
use library::translator::TranslationModel;

use crate::app::config::Config;

/// Every command in `lib.rs`'s `generate_handler!`. Kept honest by
/// `bridge_covers_all_registered_commands`.
pub const COMMANDS: &[&str] = &[
    "get_models",
    "get_languages",
    "parse_language_id",
    "get_config",
    "get_library_root",
    "reveal_library_root",
    "update_config",
    "purge_gemini_caches",
    "get_anki_sync_status",
    "sync_anki_now",
    "get_sync_status",
    "sync_get_this_device",
    "sync_web_ui_url",
    "sync_set_device_name",
    "sync_wake",
    "sync_set_enabled",
    "sync_list_devices",
    "sync_list_pending",
    "sync_add_device",
    "sync_remove_device",
    "translate_paragraph",
    "translate_chapter",
    "get_paragraph_translation_activity",
    "list_paragraph_translation_activity",
    "list_books",
    "list_book_chapters",
    "get_book_chapter_paragraph_ids",
    "get_paragraph_view",
    "get_paragraph_originals_batch",
    "get_paragraph_translations_batch",
    "get_translation_providers",
    "get_word_info",
    "import_plain_text",
    "import_epub",
    "get_book_reading_state",
    "get_book_summary_status",
    "save_book_reading_state",
    "move_book",
    "delete_book",
    "get_system_definition",
    "show_system_dictionary",
    "start_spotify_watcher",
    "stop_spotify_watcher",
    "get_now_playing",
    "get_track_lyrics_state",
    "spotify_web_connect",
    "spotify_web_disconnect",
    "spotify_web_status",
    "spotify_web_get_queue",
    "open_external_url",
];

/// Binds `127.0.0.1:port` (0 = ephemeral) and announces the real port on
/// stdout so a harness can read it back.
pub fn spawn(app: AppHandle, port: u16) {
    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => l,
            Err(err) => {
                warn!("e2e bridge: bind failed: {err}");
                return;
            }
        };
        let actual = match listener.local_addr() {
            Ok(addr) => addr.port(),
            Err(err) => {
                warn!("e2e bridge: local_addr failed: {err}");
                return;
            }
        };

        println!("FLTS_E2E_BRIDGE_LISTENING {}", json!({ "port": actual }));
        let _ = std::io::stdout().flush();
        info!("e2e bridge listening on 127.0.0.1:{actual}/bridge");

        let router = axum::Router::new()
            .route("/bridge", get(upgrade))
            .with_state(app);
        if let Err(err) = axum::serve(listener, router).await {
            warn!("e2e bridge: serve ended: {err}");
        }
    });
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

    drop(tx);
    let _ = writer.await;
}

fn wrap<T: serde::Serialize, E: serde::Serialize>(r: Result<T, E>) -> Result<Value, Value> {
    match r {
        Ok(v) => Ok(to_value(v)),
        Err(e) => Err(to_value(e)),
    }
}

/// Sync commands that don't return a `Result`.
fn plain<T: serde::Serialize>(v: T) -> Result<Value, Value> {
    Ok(to_value(v))
}

fn to_value<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or_else(|err| json!(format!("serialize failed: {err}")))
}

fn args_of<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, Value> {
    serde_json::from_value(args).map_err(|err| json!(format!("bad args: {err}")))
}

/// Declares a per-command camelCase args struct inline and destructures it.
macro_rules! args {
    ($args:expr, { $($f:ident : $t:ty),* $(,)? }) => {{
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct A { $($f: $t),* }
        let a: A = args_of($args)?;
        ($(a.$f),*)
    }};
}

async fn dispatch(app: &AppHandle, cmd: &str, args: Value) -> Result<Value, Value> {
    let state = app.state::<Arc<crate::app::AppState>>();

    match cmd {
        // --- config ---
        "get_models" => plain(crate::app::config::get_models()),
        "get_languages" => plain(crate::app::config::get_languages()),
        "get_translation_providers" => plain(crate::app::config::get_translation_providers()),
        "parse_language_id" => {
            // Only Option param in the surface: JS omits undefined, so default it.
            #[derive(serde::Deserialize)]
            struct A {
                #[serde(default)]
                code: Option<String>,
            }
            let a: A = args_of(args)?;
            plain(crate::app::config::parse_language_id(a.code))
        }
        "get_config" => wrap(crate::app::get_config(state).await),
        "update_config" => {
            let config = args!(args, { config: Config });
            wrap(crate::app::update_config(state, config).await)
        }
        "purge_gemini_caches" => wrap(crate::app::purge_gemini_caches(state).await),
        "get_library_root" => wrap(crate::app::get_library_root(app.clone()).await),
        "reveal_library_root" => wrap(crate::app::reveal_library_root(app.clone()).await),

        // --- anki sync ---
        "get_anki_sync_status" => wrap(crate::app::get_anki_sync_status(state).await),
        "sync_anki_now" => wrap(crate::app::sync_anki_now(state).await),

        // --- device sync ---
        "get_sync_status" => wrap(crate::app::sync::get_sync_status(state).await),
        "sync_get_this_device" => wrap(crate::app::sync::sync_get_this_device(state).await),
        "sync_web_ui_url" => wrap(crate::app::sync::sync_web_ui_url(state).await),
        "sync_wake" => wrap(crate::app::sync::sync_wake(state).await),
        "sync_list_devices" => wrap(crate::app::sync::sync_list_devices(state).await),
        "sync_list_pending" => wrap(crate::app::sync::sync_list_pending(state).await),
        "sync_set_device_name" => {
            let name = args!(args, { name: String });
            wrap(crate::app::sync::sync_set_device_name(state, name).await)
        }
        "sync_set_enabled" => {
            let enabled = args!(args, { enabled: bool });
            wrap(crate::app::sync::sync_set_enabled(state, enabled).await)
        }
        "sync_add_device" => {
            let (device_id, name) = args!(args, { device_id: String, name: String });
            wrap(crate::app::sync::sync_add_device(state, device_id, name).await)
        }
        "sync_remove_device" => {
            let device_id = args!(args, { device_id: String });
            wrap(crate::app::sync::sync_remove_device(state, device_id).await)
        }

        // --- translation ---
        "translate_paragraph" => {
            let (book_id, paragraph_id, model, use_cache) = args!(args, {
                book_id: Uuid, paragraph_id: usize, model: TranslationModel, use_cache: bool
            });
            wrap(
                crate::app::translate_paragraph(state, book_id, paragraph_id, model, use_cache)
                    .await,
            )
        }
        "translate_chapter" => {
            let (book_id, chapter_id, model, use_cache) = args!(args, {
                book_id: Uuid, chapter_id: usize, model: TranslationModel, use_cache: bool
            });
            wrap(crate::app::translate_chapter(state, book_id, chapter_id, model, use_cache).await)
        }
        "get_paragraph_translation_activity" => {
            let (book_id, paragraph_id) = args!(args, { book_id: Uuid, paragraph_id: usize });
            wrap(crate::app::get_paragraph_translation_activity(state, book_id, paragraph_id).await)
        }
        "list_paragraph_translation_activity" => {
            wrap(crate::app::list_paragraph_translation_activity(state).await)
        }

        // --- library reads ---
        "list_books" => wrap(crate::app::library_view::list_books(state).await),
        "list_book_chapters" => {
            let book_id = args!(args, { book_id: Uuid });
            wrap(crate::app::library_view::list_book_chapters(state, book_id).await)
        }
        "get_book_chapter_paragraph_ids" => {
            let (book_id, chapter_id) = args!(args, { book_id: Uuid, chapter_id: usize });
            wrap(
                crate::app::library_view::get_book_chapter_paragraph_ids(
                    state, book_id, chapter_id,
                )
                .await,
            )
        }
        "get_paragraph_view" => {
            let (book_id, paragraph_id) = args!(args, { book_id: Uuid, paragraph_id: usize });
            wrap(crate::app::library_view::get_paragraph_view(state, book_id, paragraph_id).await)
        }
        "get_paragraph_originals_batch" => {
            let (book_id, paragraph_ids) =
                args!(args, { book_id: Uuid, paragraph_ids: Vec<usize> });
            wrap(
                crate::app::library_view::get_paragraph_originals_batch(
                    state,
                    book_id,
                    paragraph_ids,
                )
                .await,
            )
        }
        "get_paragraph_translations_batch" => {
            let (book_id, paragraph_ids) =
                args!(args, { book_id: Uuid, paragraph_ids: Vec<usize> });
            wrap(
                crate::app::library_view::get_paragraph_translations_batch(
                    state,
                    book_id,
                    paragraph_ids,
                )
                .await,
            )
        }
        "get_word_info" => {
            let (book_id, paragraph_id, sentence_id, word_id) = args!(args, {
                book_id: Uuid, paragraph_id: usize, sentence_id: usize, word_id: usize
            });
            wrap(
                crate::app::library_view::get_word_info(
                    state,
                    book_id,
                    paragraph_id,
                    sentence_id,
                    word_id,
                )
                .await,
            )
        }
        "get_book_reading_state" => {
            let book_id = args!(args, { book_id: Uuid });
            wrap(crate::app::library_view::get_book_reading_state(state, book_id).await)
        }
        "get_book_summary_status" => {
            let book_id = args!(args, { book_id: Uuid });
            wrap(crate::app::library_view::get_book_summary_status(state, book_id).await)
        }

        // --- library writes ---
        "import_plain_text" => {
            let (title, text, source_language_id) = args!(args, {
                title: String, text: String, source_language_id: String
            });
            wrap(
                crate::app::library_view::import_plain_text(state, title, text, source_language_id)
                    .await,
            )
        }
        "import_epub" => {
            let (book, source_language_id) =
                args!(args, { book: EpubBook, source_language_id: String });
            wrap(crate::app::library_view::import_epub(state, book, source_language_id).await)
        }
        "save_book_reading_state" => {
            let (book_id, chapter_id, paragraph_id, page_offset) = args!(args, {
                book_id: Uuid, chapter_id: usize, paragraph_id: usize, page_offset: usize
            });
            wrap(
                crate::app::library_view::save_book_reading_state(
                    state,
                    book_id,
                    chapter_id,
                    paragraph_id,
                    page_offset,
                )
                .await,
            )
        }
        "move_book" => {
            let (book_id, path) = args!(args, { book_id: Uuid, path: Vec<String> });
            wrap(crate::app::library_view::move_book(state, book_id, path).await)
        }
        "delete_book" => {
            let book_id = args!(args, { book_id: Uuid });
            wrap(crate::app::library_view::delete_book(state, book_id).await)
        }

        // --- system dictionary ---
        "get_system_definition" => {
            let (word, source_lang, target_lang) = args!(args, {
                word: String, source_lang: String, target_lang: String
            });
            wrap(
                crate::app::get_system_definition(app.clone(), word, source_lang, target_lang)
                    .await,
            )
        }
        "show_system_dictionary" => {
            let word = args!(args, { word: String });
            wrap(crate::app::show_system_dictionary(app.clone(), word).await)
        }

        // --- lyrics / spotify ---
        "start_spotify_watcher" => {
            wrap(crate::app::lyrics::start_spotify_watcher(app.clone(), state).await)
        }
        "stop_spotify_watcher" => wrap(crate::app::lyrics::stop_spotify_watcher(state).await),
        "get_now_playing" => wrap(crate::app::lyrics::get_now_playing(state).await),
        "get_track_lyrics_state" => {
            let (track_id, target_lang, model) = args!(args, {
                track_id: String, target_lang: String, model: TranslationModel
            });
            wrap(
                crate::app::lyrics::get_track_lyrics_state(state, track_id, target_lang, model)
                    .await,
            )
        }
        "spotify_web_connect" => {
            let client_id = args!(args, { client_id: String });
            wrap(crate::app::spotify::web::spotify_web_connect(state, client_id).await)
        }
        "spotify_web_disconnect" => {
            wrap(crate::app::spotify::web::spotify_web_disconnect(state).await)
        }
        "spotify_web_status" => wrap(crate::app::spotify::web::spotify_web_status(state).await),
        "spotify_web_get_queue" => {
            wrap(crate::app::spotify::web::spotify_web_get_queue(state).await)
        }
        "open_external_url" => {
            let url = args!(args, { url: String });
            wrap(crate::app::spotify::web::open_external_url(url).await)
        }

        other => Err(json!(format!("unknown command: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registered() -> Vec<&'static str> {
        let src = include_str!("lib.rs");
        let block = src
            .split("generate_handler![")
            .nth(1)
            .expect("generate_handler! block")
            .split(']')
            .next()
            .expect("closing bracket");
        block
            .lines()
            .filter_map(|l| l.trim().trim_end_matches(',').rsplit("::").next())
            .filter(|s| !s.is_empty())
            .collect()
    }

    #[test]
    fn bridge_covers_all_registered_commands() {
        for cmd in registered() {
            assert!(COMMANDS.contains(&cmd), "bridge missing command: {cmd}");
        }
    }

    #[test]
    fn bridge_lists_no_unregistered_commands() {
        let registered = registered();
        for cmd in COMMANDS {
            assert!(
                registered.contains(cmd),
                "bridge lists stale command: {cmd}"
            );
        }
    }

    /// Arm literals of `dispatch`'s match, read out of this file's own source.
    /// Anchored on the first `match cmd {` (the dispatch one — this module's
    /// copy of the marker comes later) and stopped at the `other =>` fallback,
    /// so no other literal in the file can leak in.
    fn dispatch_arms() -> Vec<&'static str> {
        let src = include_str!("bridge.rs");
        let block = src
            .split("match cmd {")
            .nth(1)
            .expect("dispatch match block")
            .split("other =>")
            .next()
            .expect("fallback arm");
        block
            .lines()
            .filter_map(|l| {
                let l = l.trim();
                let rest = l.strip_prefix('"')?;
                let (name, tail) = rest.split_once('"')?;
                tail.trim_start().starts_with("=>").then_some(name)
            })
            .collect()
    }

    #[test]
    fn commands_match_dispatch_arms() {
        let arms = dispatch_arms();
        assert!(!arms.is_empty(), "arm parser found nothing");
        for cmd in COMMANDS {
            assert!(
                arms.contains(cmd),
                "COMMANDS entry has no dispatch arm: {cmd}"
            );
        }
        for arm in &arms {
            assert!(
                COMMANDS.contains(arm),
                "dispatch arm missing from COMMANDS: {arm}"
            );
        }
        assert_eq!(arms.len(), COMMANDS.len(), "duplicate dispatch arm");
    }

    #[test]
    fn args_of_maps_camel_case() {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct A {
            book_id: String,
            paragraph_ids: Vec<usize>,
        }
        let a: A = args_of(json!({"bookId": "x", "paragraphIds": [1, 2]})).unwrap();
        assert_eq!(a.book_id, "x");
        assert_eq!(a.paragraph_ids, vec![1, 2]);
    }

    #[test]
    fn args_of_reports_bad_args() {
        let err = args_of::<Vec<u8>>(json!({})).unwrap_err();
        assert!(err.as_str().unwrap().starts_with("bad args:"));
    }
}
