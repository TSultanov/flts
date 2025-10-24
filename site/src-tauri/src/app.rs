use std::{error::Error, fmt::Display, fs, path::PathBuf, sync::Mutex};

use directories::ProjectDirs;
use library::library::Library;
use tauri::Emitter;
use tracing::info;
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
}

impl App {
    pub fn init(app: tauri::AppHandle) -> anyhow::Result<Self> {
        let dirs = ProjectDirs::from("", "TS", "FLTS").ok_or(AppError::ProjectDirsError)?;

        let config_dir = dirs.config_dir();
        info!("config_dir = {:?}", config_dir);
        fs::create_dir_all(config_dir)?;
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
        if let Some(library_path) = &self.config.library_path {
            let fs = PhysicalFS::new(library_path);
            self.library = Some(LibraryView::create(Library::open(fs.into())?));
            if let Some(library) = &self.library {
                self.app.emit("library_updated", library.list_books()?)?;
            }
        }

        Ok(())
    }
}

#[tauri::command]
pub fn update_config(state: tauri::State<'_, Mutex<App>>, config: Config) -> Result<(), String> {
    let mut app = state
        .lock()
        .map_err(|_| AppError::StatePoisonError.to_string())?;
    app.update_config(config).map_err(|err| err.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_config(state: tauri::State<'_, Mutex<App>>) -> Result<Config, String> {
    let app = state
        .lock()
        .map_err(|_| AppError::StatePoisonError.to_string())?;
    Ok(app.config.clone())
}
