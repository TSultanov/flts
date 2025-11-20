use std::sync::Arc;

use library::library::file_watcher::LibraryWatcher;
use log::{info, warn};
use tauri::{Builder, Manager, async_runtime::Mutex};

pub mod app;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(debug_assertions)]
    Builder::default()
        .plugin(tauri_plugin_dialog::init())
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
            let app_state = Arc::new(Mutex::new(crate::app::App::new(
                app.handle().clone(),
                None,
            )?));
            info!("App created");
            app.manage(app_state.clone());

            info!("Spawning async init");
            tauri::async_runtime::spawn(async move {
                if let Err(err) = app_state.lock().await.eval_config().await {
                    warn!("Failed to evaluate config at startup: {err}");
                }
                let recv = {
                    let mut watcher = watcher.lock().await;
                    watcher.get_recv()
                };

                loop {
                    let event = recv.recv_async().await;
                    match event {
                        Ok(event) => {
                            app_state
                                .lock()
                                .await
                                .handle_file_change_event(&event)
                                .await
                                .unwrap_or_else(|err| {
                                    warn!("Failed to process event {event:?}: {err}")
                                });
                        }
                        Err(err) => warn!("Failed to recieve event from watcher: {}", err),
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app::config::get_models,
            app::config::get_languages,
            app::get_config,
            app::update_config,
            app::translate_paragraph,
            app::get_paragraph_translation_request_id,
            app::library_view::list_books,
            app::library_view::list_book_chapters,
            app::library_view::get_book_chapter_paragraphs,
            app::library_view::get_word_info,
            app::library_view::import_plain_text,
            app::library_view::import_epub,
            app::library_view::get_book_reading_state,
            app::library_view::save_book_reading_state,
            app::library_view::move_book,
            app::library_view::delete_book,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
