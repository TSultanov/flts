use std::{
    error::Error,
    fmt::Display,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use directories::ProjectDirs;
use isolang::Language;
use library::{
    cache::TranslationsCache,
    library::{
        Library,
        file_watcher::{LibraryFileChange, LibraryWatcher},
    },
    translation_stats::TranslationSizeCache,
    translator::TranslationModel,
};
use log::{info, warn};
use tauri::{Emitter, async_runtime::Mutex};
use uuid::Uuid;

use crate::app::{config::Config, library_view::LibraryView, translation_queue::TranslationQueue};

#[cfg(mobile)]
use dirs_next::{config_dir, document_dir};

pub mod config;
pub mod library_view;
pub mod translation_queue;

#[derive(Debug)]
pub enum AppError {
    StatePoisonError,
    ProjectDirsError,
    NoTranslationQueueError,
    TestError,
}

impl Error for AppError {}

impl Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::ProjectDirsError => write!(f, "Failed to find app configuration directories"),
            AppError::StatePoisonError => write!(f, "Fatal: state poisoned"),
            AppError::NoTranslationQueueError => write!(
                f,
                "Failed to translate paragraph: no translation queue initialized"
            ),
            AppError::TestError => write!(f, "Test error"),
        }
    }
}

pub struct App {
    app: tauri::AppHandle,
    config_path: PathBuf,
    config: Config,
    library: Option<Arc<Mutex<Library>>>,
    library_view: Option<LibraryView>,
    translation_queue: Option<TranslationQueue>,
    watcher: Option<Arc<Mutex<LibraryWatcher>>>,
}

impl App {
    pub fn new(
        app: tauri::AppHandle,
        watcher: Option<Arc<Mutex<LibraryWatcher>>>,
    ) -> anyhow::Result<Self> {
        info!("Startup!");

        let dirs = ProjectDirs::from("com", "TS", "FLTS").ok_or(AppError::ProjectDirsError)?;
        let config_dir = dirs.config_dir();

        if !fs::exists(config_dir)? {
            fs::create_dir(config_dir)?;
        }

        // #[cfg(mobile)]
        // let config_dir = config_dir().unwrap();

        info!("config_dir = {:?}", config_dir);
        let config_path = config_dir.join("config.json");

        let config = if config_path.exists() {
            Config::load(&config_path)?
        } else {
            Config::default()
        };

        let app = Self {
            app,
            config_path,
            config,
            library: None,
            library_view: None,
            translation_queue: None,
            watcher,
        };

        Ok(app)
    }

    pub async fn update_config(&mut self, config: Config) -> anyhow::Result<()> {
        self.config = config;

        // Translator settings (provider/key/model) are captured when the translation queue is created.
        // Reset it so the next translation uses the latest config.
        self.translation_queue = None;

        #[cfg(mobile)]
        {
            let library_path = {
                let documents = document_dir();
                if let Some(documents) = &documents
                    && !fs::exists(documents)?
                {
                    fs::create_dir(documents)?;
                };
                let library_directory = documents.map(|p| p.join("FLTSLibrary"));
                if let Some(library_directory) = &library_directory
                    && !fs::exists(library_directory)?
                {
                    fs::create_dir(library_directory)?;
                };
                library_directory.map(|d| d.to_string_lossy().to_string())
            };

            self.config.library_path = library_path;
        }
        info!("config = {:?}", self.config);

        self.config.save(&self.config_path)?;
        info!("Emitting \"config_updated\"");
        self.app.emit("config_updated", self.config.clone())?;
        self.eval_config().await?;
        Ok(())
    }

    pub async fn eval_config(&mut self) -> anyhow::Result<()> {
        let target_language = Language::from_639_3(&self.config.target_language_id);

        let library_path = &self.config.library_path;

        info!("library_path = {library_path:?}");

        if let Some(library_path) = library_path {
            let library = Arc::new(Mutex::new(
                Library::open(PathBuf::from(library_path)).await?,
            ));
            self.library = Some(library.clone());
            if let Some(watcher) = &self.watcher {
                watcher
                    .lock()
                    .await
                    .set_path(&Path::new(library_path).to_path_buf())
                    .unwrap_or_else(|err| {
                        warn!("Failed to set watcher path to {}: {}", library_path, err)
                    });
            }

            self.library_view = Some(LibraryView::create(self.app.clone(), library.clone()));
            if let Some(library) = &self.library_view {
                let books = library.list_books(target_language.as_ref()).await?;
                info!("Emitting \"library_updated\"");
                self.app.emit("library_updated", books)?;
            }
        }

        Ok(())
    }

    async fn get_cache() -> anyhow::Result<TranslationsCache> {
        let dirs = ProjectDirs::from("", "TS", "FLTS").unwrap();
        let cache_dir = dirs.cache_dir();
        TranslationsCache::create(cache_dir).await
    }

    async fn get_stats_cache() -> anyhow::Result<TranslationSizeCache> {
        let dirs = ProjectDirs::from("", "TS", "FLTS").unwrap();
        let cache_dir = dirs.cache_dir();
        TranslationSizeCache::create(cache_dir).await
    }

    pub async fn handle_file_change_event(
        &mut self,
        event: &LibraryFileChange,
    ) -> anyhow::Result<()> {
        if let Some(library) = &mut self.library_view {
            let had_effect = library.handle_file_change_event(event).await?;
            if had_effect {
                match event {
                    LibraryFileChange::BookChanged { modified: _, uuid } => {
                        info!("Emitting \"book_updated\" for {uuid}");
                        self.app.emit("book_updated", uuid)?;
                        if let Some(library) = &self.library_view {
                            let target_language =
                                Language::from_639_3(&self.config.target_language_id);

                            info!("Emitting \"library_updated\"");
                            self.app.emit(
                                "library_updated",
                                library.list_books(target_language.as_ref()).await?,
                            )?;
                        }
                    }
                    LibraryFileChange::TranslationChanged {
                        modified: _,
                        from: _,
                        to,
                        uuid,
                    } => {
                        let target_language = Language::from_639_3(&self.config.target_language_id);

                        if target_language.map_or(false, |tl| tl == *to) {
                            info!("Emitting \"book_updated\" for {uuid}");
                            self.app.emit("book_updated", uuid)?;
                            if let Some(library) = &self.library_view {
                                info!("Emitting \"library_updated\"");
                                self.app.emit(
                                    "library_updated",
                                    library.list_books(target_language.as_ref()).await?,
                                )?;
                            }
                        }
                    }
                    LibraryFileChange::DictionaryChanged {
                        modified: _,
                        from,
                        to,
                    } => {
                        let payload = (from.to_639_3(), to.to_639_3());
                        info!("Emitting \"dictionary_updated\" for {payload:?}",);
                        self.app.emit("dictionary_updated", payload)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn translate_paragraph(
        &mut self,
        book_id: Uuid,
        paragraph_id: usize,
        model: TranslationModel,
        use_cache: bool,
    ) -> anyhow::Result<usize> {
        if let Some(library) = &self.library
            && self.translation_queue.is_none()
        {
            let cache = Arc::new(Mutex::new(Self::get_cache().await?));
            let stats_cache = Arc::new(Mutex::new(Self::get_stats_cache().await?));
            self.translation_queue = TranslationQueue::init(
                library.clone(),
                cache,
                stats_cache,
                &self.config,
                self.app.clone(),
            );
        }

        if let Some(q) = &self.translation_queue {
            Ok(q.translate(book_id, paragraph_id, model, use_cache).await?)
        } else {
            Err(AppError::NoTranslationQueueError.into())
        }
    }

    pub async fn get_paragraph_translation_request_id(
        &mut self,
        book_id: Uuid,
        paragraph_id: usize,
    ) -> anyhow::Result<Option<usize>> {
        if let Some(library) = &self.library
            && self.translation_queue.is_none()
        {
            let cache = Arc::new(Mutex::new(Self::get_cache().await?));
            let stats_cache = Arc::new(Mutex::new(Self::get_stats_cache().await?));
            self.translation_queue = TranslationQueue::init(
                library.clone(),
                cache,
                stats_cache,
                &self.config,
                self.app.clone(),
            );
        }

        if let Some(q) = &self.translation_queue {
            Ok(q.get_request_id(book_id, paragraph_id).await)
        } else {
            Err(AppError::NoTranslationQueueError.into())
        }
    }
}

#[tauri::command]
pub async fn update_config(
    state: tauri::State<'_, Arc<Mutex<App>>>,
    config: Config,
) -> Result<(), String> {
    info!("Update config request");
    let mut app = state.lock().await;
    info!("App lock acquired");
    app.update_config(config)
        .await
        .map_err(|err| err.to_string())?;
    info!("Config processed");
    Ok(())
}

#[tauri::command]
pub async fn get_config(state: tauri::State<'_, Arc<Mutex<App>>>) -> Result<Config, String> {
    let app = state.lock().await;
    Ok(app.config.clone())
}

#[tauri::command]
pub async fn translate_paragraph(
    state: tauri::State<'_, Arc<Mutex<App>>>,
    book_id: Uuid,
    paragraph_id: usize,
    model: usize,
    use_cache: bool,
) -> Result<usize, String> {
    let mut app = state.lock().await;
    app.translate_paragraph(
        book_id,
        paragraph_id,
        TranslationModel::from(model),
        use_cache,
    )
    .await
    .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_paragraph_translation_request_id(
    state: tauri::State<'_, Arc<Mutex<App>>>,
    book_id: Uuid,
    paragraph_id: usize,
) -> Result<Option<usize>, String> {
    let mut app = state.lock().await;
    app.get_paragraph_translation_request_id(book_id, paragraph_id)
        .await
        .map_err(|err| err.to_string())
}
