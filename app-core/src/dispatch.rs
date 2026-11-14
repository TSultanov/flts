//! `{cmd, args}` JSON in, JSON out: the one command table every shell and the
//! E2E bridge go through. `args` keys are camelCase, as Tauri's IPC sends them.

use serde_json::{Value, json};
use uuid::Uuid;

use library::epub_importer::EpubBook;

use crate::app::{AppState, config::Config};
use crate::host::State;

/// Every command a frontend may call; the Playwright conformance spec
/// mirrors this list.
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
    "sync_reset_identity",
    "sync_keep_identity",
    "translate_paragraph",
    "translate_chapter",
    "get_paragraph_translation_activity",
    "list_paragraph_translation_activity",
    "list_books",
    "list_book_chapters",
    "get_book_chapter_paragraph_ids",
    "get_paragraph_view",
    "get_paragraph_originals_batch",
    "get_paragraph_html_batch",
    "get_paragraph_texts_batch",
    "get_translation_providers",
    "get_word_info",
    "create_card_for_word",
    "import_plain_text",
    "parse_epub",
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

/// Spotify DevTools commands: dispatched like the rest, but they restart
/// Spotify and touch `~/Library/LaunchAgents`, so harnesses never call them.
pub const HOST_ONLY_COMMANDS: &[&str] = &[
    "spotify_cdp_status",
    "spotify_restart_with_devtools",
    "spotify_login_agent_status",
    "spotify_install_login_agent",
    "spotify_remove_login_agent",
];

/// Every event the core emits.
pub const FORWARDED_EVENTS: &[&str] = &[
    "anki_sync_status_changed",
    "book_deleted",
    "book_updated",
    "cards_updated",
    "config_updated",
    "library_import_progress",
    "library_updated",
    "lyrics_resolved",
    "lyrics_translation_done",
    "lyrics_translation_error",
    "lyrics_translation_progress",
    "paragraph_translation_finished",
    "paragraph_translation_progress",
    "paragraph_translation_started",
    "paragraph_updated",
    "spotify_queue",
    "spotify_state",
    "summary_generation_progress",
    "sync_status_changed",
];

/// Error prefix for a name the table does not know, so a shell can try its own.
pub const UNKNOWN_COMMAND: &str = "unknown command: ";

pub fn wrap<T: serde::Serialize, E: serde::Serialize>(r: Result<T, E>) -> Result<Value, Value> {
    match r {
        Ok(v) => Ok(to_value(v)),
        Err(e) => Err(to_value(e)),
    }
}

/// Sync commands that don't return a `Result`.
pub fn plain<T: serde::Serialize>(v: T) -> Result<Value, Value> {
    Ok(to_value(v))
}

pub fn to_value<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or_else(|err| json!(format!("serialize failed: {err}")))
}

pub fn args_of<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, Value> {
    serde_json::from_value(args).map_err(|err| json!(format!("bad args: {err}")))
}

/// Declares a per-command camelCase args struct inline and destructures it.
#[macro_export]
macro_rules! args {
    ($args:expr, { $($f:ident : $t:ty),* $(,)? }) => {{
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct A { $($f: $t),* }
        let a: A = $crate::dispatch::args_of($args)?;
        ($(a.$f),*)
    }};
}

pub async fn dispatch(core: &AppState, cmd: &str, args: Value) -> Result<Value, Value> {
    let app = &core.host();
    let state = State(core);

    match cmd {
        // --- config ---
        "get_models" => wrap(crate::app::config::get_models(state).await),
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
        "sync_reset_identity" => wrap(crate::app::sync::sync_reset_identity(state).await),
        "sync_keep_identity" => wrap(crate::app::sync::sync_keep_identity(state).await),

        // --- translation ---
        "translate_paragraph" => {
            let (book_id, paragraph_id, model, use_cache) = args!(args, {
                book_id: Uuid, paragraph_id: usize, model: String, use_cache: bool
            });
            wrap(
                crate::app::translate_paragraph(state, book_id, paragraph_id, model, use_cache)
                    .await,
            )
        }
        "translate_chapter" => {
            let (book_id, chapter_id, model, use_cache) = args!(args, {
                book_id: Uuid, chapter_id: usize, model: String, use_cache: bool
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
        "get_paragraph_html_batch" => {
            let (book_id, paragraph_ids) =
                args!(args, { book_id: Uuid, paragraph_ids: Vec<usize> });
            wrap(
                crate::app::library_view::get_paragraph_html_batch(state, book_id, paragraph_ids)
                    .await,
            )
        }
        "get_paragraph_texts_batch" => {
            let (book_id, paragraph_ids) =
                args!(args, { book_id: Uuid, paragraph_ids: Vec<usize> });
            wrap(
                crate::app::library_view::get_paragraph_texts_batch(state, book_id, paragraph_ids)
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
        "create_card_for_word" => {
            let (book_id, paragraph_id, sentence_id, word_id) = args!(args, {
                book_id: Uuid, paragraph_id: usize, sentence_id: usize, word_id: usize
            });
            wrap(
                crate::app::library_view::create_card_for_word(
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
        "parse_epub" => {
            let epub_base64 = args!(args, { epub_base64: String });
            wrap(crate::app::library_view::parse_epub(epub_base64).await)
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
                track_id: String, target_lang: String, model: String
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

        "spotify_cdp_status" => wrap(crate::app::spotify::cdp::spotify_cdp_status().await),
        "spotify_restart_with_devtools" => {
            wrap(crate::app::spotify::cdp::spotify_restart_with_devtools().await)
        }
        "spotify_login_agent_status" => {
            wrap(crate::app::spotify::cdp::spotify_login_agent_status().await)
        }
        "spotify_install_login_agent" => {
            wrap(crate::app::spotify::cdp::spotify_install_login_agent().await)
        }
        "spotify_remove_login_agent" => {
            wrap(crate::app::spotify::cdp::spotify_remove_login_agent().await)
        }

        other => Err(json!(format!("{UNKNOWN_COMMAND}{other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Arm literals of `dispatch`'s match, read out of this file's own source.
    fn dispatch_arms() -> Vec<&'static str> {
        let src = include_str!("dispatch.rs");
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
        let declared: Vec<&str> = COMMANDS.iter().chain(HOST_ONLY_COMMANDS).copied().collect();
        for cmd in &declared {
            assert!(
                arms.contains(cmd),
                "declared command has no dispatch arm: {cmd}"
            );
        }
        for arm in &arms {
            assert!(declared.contains(arm), "undeclared dispatch arm: {arm}");
        }
        assert_eq!(arms.len(), declared.len(), "duplicate dispatch arm");
    }

    /// Every emitted name in the crate. Comments are stripped, and the name
    /// literal may sit lines below its emit call.
    fn emitted_event_names() -> Vec<String> {
        fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("read_dir") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let src = std::fs::read_to_string(&path).expect("read");
                    let code: String = src
                        .lines()
                        .filter(|l| !l.trim_start().starts_with("//"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    scan(&code, &path, out);
                }
            }
        }
        // Built by concat so this scanner's own source holds no emit token.
        fn scan(src: &str, path: &std::path::Path, out: &mut Vec<String>) {
            let pat = concat!(".emit", "(");
            let mut rest = src;
            while let Some(at) = rest.find(pat) {
                rest = &rest[at + pat.len()..];
                let trimmed = rest.trim_start();
                // `Host::emit(&self, event: ...)` and its forwarders take a name.
                if trimmed.starts_with("event") || trimmed.starts_with("&self") {
                    continue;
                }
                let name = trimmed
                    .strip_prefix('"')
                    .and_then(|s| s.split_once('"'))
                    .map(|(name, _)| name)
                    .unwrap_or_else(|| panic!("non-literal emit name in {}", path.display()));
                out.push(name.to_string());
            }
        }

        let mut out = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut out,
        );
        out
    }

    #[test]
    fn forwarded_events_track_emit_sites() {
        let names = emitted_event_names();
        assert!(!names.is_empty(), "emit scanner found nothing");
        for name in &names {
            assert!(
                FORWARDED_EVENTS.contains(&name.as_str()),
                "core emits an unlisted event: {name}"
            );
        }
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
