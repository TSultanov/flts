use std::sync::Arc;

use library::library::file_watcher::LibraryWatcher;
use tauri::{Builder, Manager, async_runtime::Mutex};
use log::{info, warn};

pub mod app;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            let watcher = Arc::new(Mutex::new(LibraryWatcher::new()?));
            app.manage(watcher.clone());
            let app_state = Arc::new(Mutex::new(crate::app::App::init(
                app.handle().clone(),
                Some(watcher.clone()),
            )?));
            app.manage(app_state.clone());

            tauri::async_runtime::spawn(async move {
                loop {
                    let event = {
                        let mut watcher = watcher.lock().await;
                        watcher.recv().await
                    };
                    if let Some(event) = event {
                        app_state
                            .lock()
                            .await
                            .handle_file_change_event(&event)
                            .await
                            .unwrap_or_else(|err| {
                                warn!("Failed to process event {event:?}: {err}")
                            });
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
            app::library_view::list_books,
            app::library_view::list_book_chapters,
            app::library_view::get_book_chapter_paragraphs,
            app::library_view::get_word_info,
            app::library_view::import_plain_text,
            app::library_view::import_epub,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
