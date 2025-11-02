use std::{
    error::Error,
    fmt::Display,
    path::{Path, PathBuf},
    sync::Arc,
};

use directories::ProjectDirs;
use isolang::Language;
use library::library::{
    Library,
    file_watcher::{LibraryFileChange, LibraryWatcher},
};
use log::{info, warn};
use tauri::{Emitter, async_runtime::Mutex};
use vfs::PhysicalFS;

use crate::app::{config::Config, library_view::LibraryView};

pub mod config;
pub mod library_view;

#[derive(Debug)]
pub enum AppError {
    StatePoisonError,
    ProjectDirsError,
}

impl Error for AppError {}

impl Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::ProjectDirsError => write!(f, "Failed to find app configuration directories"),
            AppError::StatePoisonError => write!(f, "Fatal: state poisoned"),
        }
    }
}

pub struct App {
    app: tauri::AppHandle,
    config_path: PathBuf,
    config: Config,
    library: Option<LibraryView>,
    watcher: Option<Arc<Mutex<LibraryWatcher>>>,
}

impl App {
    pub fn init(
        app: tauri::AppHandle,
        watcher: Option<Arc<Mutex<LibraryWatcher>>>,
    ) -> anyhow::Result<Self> {
        let dirs = ProjectDirs::from("", "TS", "FLTS").ok_or(AppError::ProjectDirsError)?;

        let config_dir = dirs.config_dir();
        info!("config_dir = {:?}", config_dir);
        let config_path = config_dir.join("config.json");

        let config = if config_path.exists() {
            Config::load(&config_path)?
        } else {
            Config::default()
        };

        let mut app = Self {
            app,
            config_path,
            config,
            library: None,
            watcher,
        };

        app.eval_config()?;

        Ok(app)
    }

    pub fn update_config(&mut self, config: Config) -> anyhow::Result<()> {
        self.config = config;
        self.config.save(&self.config_path)?;
        self.app.emit("config_updated", self.config.clone())?;
        self.eval_config()?;
        Ok(())
    }

    fn eval_config(&mut self) -> anyhow::Result<()> {
        let target_language = self
            .config
            .target_language_id
            .as_ref()
            .and_then(|l| Language::from_639_3(l));

        if let Some(library_path) = &self.config.library_path {
            if let Some(watcher) = &self.watcher {
                watcher
                    .blocking_lock()
                    .set_path(&Path::new(library_path).to_path_buf())
                    .unwrap_or_else(|err| {
                        warn!("Failed to set watcher path to {}: {}", library_path, err)
                    });
            }
            let fs = PhysicalFS::new(library_path);
            self.library = Some(LibraryView::create(
                self.app.clone(),
                Library::open(fs.into())?,
            ));
            if let Some(library) = &self.library {
                self.app.emit(
                    "library_updated",
                    library.list_books(target_language.as_ref())?,
                )?;
            }
        }

        Ok(())
    }

    pub async fn handle_file_change_event(
        &mut self,
        event: &LibraryFileChange,
    ) -> anyhow::Result<()> {
        if let Some(library) = &mut self.library {
            library.handle_file_change_event(event).await?;
        }
        match event {
            LibraryFileChange::BookChanged { modified: _, uuid } => {
                info!("Emitting book_updated event");
                self.app.emit("book_updated", uuid)?;
                if let Some(library) = &self.library {
                    let target_language = self
                        .config
                        .target_language_id
                        .as_ref()
                        .and_then(|l| Language::from_639_3(l));

                    info!("Emitting library_updated event");
                    self.app.emit(
                        "library_updated",
                        library.list_books(target_language.as_ref())?,
                    )?;
                }
            }
            LibraryFileChange::TranslationChanged {
                modified: _,
                from: _,
                to,
                uuid,
            } => {
                let target_language = self
                    .config
                    .target_language_id
                    .as_ref()
                    .and_then(|l| Language::from_639_3(l));

                if target_language.map_or(false, |tl| tl == *to) {
                    info!("Emitting book_updated event");
                    self.app.emit("book_updated", uuid)?;
                    if let Some(library) = &self.library {
                        info!("Emitting library_updated event");
                        self.app.emit(
                            "library_updated",
                            library.list_books(target_language.as_ref())?,
                        )?;
                    }
                }
            }
            LibraryFileChange::DictionaryChanged {
                modified: _,
                from,
                to,
            } => {
                info!("Emitting dictionary_updated event");
                self.app
                    .emit("dictionary_updated", (from.to_639_3(), to.to_639_3()))?;
            }
        }
        Ok(())
    }
}

#[tauri::command]
pub fn update_config(
    state: tauri::State<'_, Arc<Mutex<App>>>,
    config: Config,
) -> Result<(), String> {
    let mut app = state.blocking_lock();
    app.update_config(config).map_err(|err| err.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_config(state: tauri::State<'_, Arc<Mutex<App>>>) -> Result<Config, String> {
    let app = state.blocking_lock();
    Ok(app.config.clone())
}
