use tauri::{async_runtime::Mutex, Builder, Manager};

pub mod app;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_state = crate::app::App::init(app.handle().clone())?;
            app.manage(Mutex::new(app_state));

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
