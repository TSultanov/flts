use std::sync::Arc;

use library::library::file_watcher::LibraryWatcher;
use log::{info, warn};
use tauri::{Builder, Emitter, Manager, RunEvent};
use tokio::sync::Mutex;

pub mod app;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[allow(unused_mut)]
    let mut builder = Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init());

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        builder = builder.plugin(tauri_plugin_window_state::Builder::new().build());
    }

    builder
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .targets([
                            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                                file_name: Some("logs".to_string()),
                            }),
                        ])
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            info!("Creating watcher");
            let watcher = Arc::new(Mutex::new(LibraryWatcher::new()?));
            info!("Watcher created");
            app.manage(watcher.clone());
            info!("Creating app");
            let app_state = Arc::new(crate::app::AppState::new(
                app.handle().clone(),
                watcher.clone(),
            )?);
            info!("App created");
            app.manage(app_state.clone());

            info!("Spawning watch->emit bridges");
            {
                let mut rx = app_state.subscribe_config();
                let app = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    while rx.changed().await.is_ok() {
                        info!("Emitting \"config_updated\"");
                        if let Err(err) = app.emit("config_updated", ()) {
                            warn!("Failed to emit config_updated: {err}");
                        }
                    }
                });
            }
            {
                let mut rx = app_state.subscribe_library();
                let app = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    while rx.changed().await.is_ok() {
                        info!("Emitting \"library_updated\"");
                        if let Err(err) = app.emit("library_updated", ()) {
                            warn!("Failed to emit library_updated: {err}");
                        }
                    }
                });
            }
            {
                let mut rx = app_state.subscribe_anki_sync_status();
                let app = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    while rx.changed().await.is_ok() {
                        if let Err(err) = app.emit("anki_sync_status_changed", ()) {
                            warn!("Failed to emit anki_sync_status_changed: {err}");
                        }
                    }
                });
            }

            info!("Spawning async init");
            let resume_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = resume_state.eval_config().await {
                    warn!("Failed to evaluate config at startup: {err}");
                }

                // Try to silently restore Spotify Web credentials from the
                // OS keychain. Polling itself starts when the user opens the
                // lyrics view (start_spotify_watcher); this just ensures we
                // have a valid token in hand by then.
                let client_id = resume_state.config_borrow_client_id();
                resume_state.spotify_web.try_resume(client_id).await;

                let mut recv = {
                    let mut watcher = watcher.lock().await;
                    watcher
                        .take_recv()
                        .expect("LibraryWatcher receiver already taken")
                };

                while let Some(event) = recv.recv().await {
                    app_state
                        .handle_file_change_event(&event)
                        .await
                        .unwrap_or_else(|err| warn!("Failed to process event {event:?}: {err}"));
                }
                warn!("LibraryWatcher sender disconnected; file change loop exiting");
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app::config::get_models,
            app::config::get_languages,
            app::get_config,
            app::update_config,
            app::get_anki_sync_status,
            app::sync_anki_now,
            app::translate_paragraph,
            app::get_paragraph_translation_activity,
            app::library_view::list_books,
            app::library_view::list_book_chapters,
            app::library_view::get_book_chapter_paragraph_ids,
            app::library_view::get_paragraph_view,
            app::library_view::get_paragraph_originals_batch,
            app::library_view::get_paragraph_translations_batch,
            app::config::get_translation_providers,
            app::library_view::get_word_info,
            app::library_view::import_plain_text,
            app::library_view::import_epub,
            app::library_view::get_book_reading_state,
            app::library_view::get_book_summary_status,
            app::library_view::save_book_reading_state,
            app::library_view::move_book,
            app::library_view::delete_book,
            app::library_view::mark_word_visible,
            app::get_system_definition,
            app::show_system_dictionary,
            app::lyrics::start_spotify_watcher,
            app::lyrics::stop_spotify_watcher,
            app::lyrics::get_now_playing,
            app::lyrics::get_track_lyrics_state,
            app::spotify::web::spotify_web_connect,
            app::spotify::web::spotify_web_disconnect,
            app::spotify::web::spotify_web_status,
            app::spotify::web::spotify_web_get_queue,
            app::spotify::web::open_external_url,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let RunEvent::Exit = event {
                let app_state = app_handle.state::<Arc<crate::app::AppState>>();
                let app_state = app_state.inner().clone();
                tauri::async_runtime::block_on(async move {
                    app_state.shutdown().await;
                });
                info!("Forcing process exit");
                app_handle.cleanup_before_exit();
                std::process::exit(0);
            }
        });
}
