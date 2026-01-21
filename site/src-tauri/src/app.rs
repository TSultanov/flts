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
use tauri::Emitter;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::app::{config::Config, library_view::LibraryView, translation_queue::TranslationQueue};

#[cfg(mobile)]
use dirs_next::document_dir;

pub mod config;
pub mod library_view;
pub mod translation_queue;

#[derive(Debug)]
pub enum AppError {
    StatePoisonError,
    ProjectDirsError,
    NoTranslationQueueError,
    NoLibraryError,
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
            AppError::NoLibraryError => {
                write!(f, "Failed to translate paragraph: no library configured")
            }
            AppError::TestError => write!(f, "Test error"),
        }
    }
}

pub struct AppState {
    app: tauri::AppHandle,
    config_path: PathBuf,
    config: RwLock<Config>,
    library: RwLock<Option<Arc<Library>>>,
    translation_queue: RwLock<Option<Arc<TranslationQueue>>>,
    watcher: Arc<Mutex<LibraryWatcher>>,
}

impl AppState {
    pub fn new(app: tauri::AppHandle, watcher: Arc<Mutex<LibraryWatcher>>) -> anyhow::Result<Self> {
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

        Ok(Self {
            app,
            config_path,
            config: RwLock::new(config),
            library: RwLock::new(None),
            translation_queue: RwLock::new(None),
            watcher,
        })
    }

    pub async fn update_config(&self, config: Config) -> anyhow::Result<()> {
        #[cfg(mobile)]
        let mut config = config;
        #[cfg(not(mobile))]
        let config = config;

        // Translator settings (provider/key/model) are captured when the translation queue is created.
        // Reset it so the next translation uses the latest config.
        *self.translation_queue.write().await = None;

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

            config.library_path = library_path;
        }
        info!("config = {:?}", config);

        config.save(&self.config_path)?;
        *self.config.write().await = config.clone();
        info!("Emitting \"config_updated\"");
        self.app.emit("config_updated", config)?;
        self.eval_config().await?;
        Ok(())
    }

    pub async fn eval_config(&self) -> anyhow::Result<()> {
        let config = self.config.read().await.clone();
        let target_language = Language::from_639_3(&config.target_language_id);
        let library_path = config.library_path.clone();

        info!("library_path = {library_path:?}");

        if let Some(library_path) = library_path {
            let library = Arc::new(Library::open(PathBuf::from(&library_path)).await?);
            *self.library.write().await = Some(library.clone());

            self.watcher
                .lock()
                .await
                .set_path(&Path::new(&library_path).to_path_buf())
                .unwrap_or_else(|err| {
                    warn!("Failed to set watcher path to {}: {}", library_path, err)
                });

            let library_view = LibraryView::create(self.app.clone(), library.clone());
            let books = library_view.list_books(target_language.as_ref()).await?;
            info!("Emitting \"library_updated\"");
            self.app.emit("library_updated", books)?;
        } else {
            *self.library.write().await = None;
            *self.translation_queue.write().await = None;
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

    pub async fn handle_file_change_event(&self, event: &LibraryFileChange) -> anyhow::Result<()> {
        let library = { self.library.read().await.clone() };
        let Some(library) = library else {
            return Ok(());
        };

        let had_effect = library.handle_file_change_event(event).await?;
        if !had_effect {
            return Ok(());
        }

        match event {
            LibraryFileChange::BookChanged { modified: _, uuid } => {
                info!("Emitting \"book_updated\" for {uuid}");
                self.app.emit("book_updated", uuid)?;

                let target_language_id = { self.config.read().await.target_language_id.clone() };
                let target_language = Language::from_639_3(&target_language_id);
                let library_view = LibraryView::create(self.app.clone(), library.clone());

                info!("Emitting \"library_updated\"");
                self.app.emit(
                    "library_updated",
                    library_view.list_books(target_language.as_ref()).await?,
                )?;
            }
            LibraryFileChange::TranslationChanged {
                modified: _,
                from: _,
                to,
                uuid,
            } => {
                let target_language_id = { self.config.read().await.target_language_id.clone() };
                let target_language = Language::from_639_3(&target_language_id);

                if target_language.map_or(false, |tl| tl == *to) {
                    info!("Emitting \"book_updated\" for {uuid}");
                    self.app.emit("book_updated", uuid)?;

                    let library_view = LibraryView::create(self.app.clone(), library.clone());
                    info!("Emitting \"library_updated\"");
                    self.app.emit(
                        "library_updated",
                        library_view.list_books(target_language.as_ref()).await?,
                    )?;
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

        Ok(())
    }

    async fn get_or_init_translation_queue(
        &self,
        library: Arc<Library>,
    ) -> anyhow::Result<Arc<TranslationQueue>> {
        if let Some(queue) = self.translation_queue.read().await.clone() {
            return Ok(queue);
        }

        let config = self.config.read().await.clone();
        let cache = Arc::new(Self::get_cache().await?);
        let stats_cache = Arc::new(Self::get_stats_cache().await?);
        let queue = TranslationQueue::init(library, cache, stats_cache, &config, self.app.clone())
            .ok_or(AppError::NoTranslationQueueError)?;

        let mut guard = self.translation_queue.write().await;
        if let Some(existing) = guard.as_ref() {
            return Ok(existing.clone());
        }
        *guard = Some(queue.clone());
        Ok(queue)
    }

    pub async fn translate_paragraph(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
        model: TranslationModel,
        use_cache: bool,
    ) -> anyhow::Result<usize> {
        let library = { self.library.read().await.clone() }.ok_or(AppError::NoLibraryError)?;
        let queue = self.get_or_init_translation_queue(library).await?;
        Ok(queue
            .translate(book_id, paragraph_id, model, use_cache)
            .await?)
    }

    pub async fn get_paragraph_translation_request_id(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
    ) -> anyhow::Result<Option<usize>> {
        let library = { self.library.read().await.clone() }.ok_or(AppError::NoLibraryError)?;
        let queue = self.get_or_init_translation_queue(library).await?;
        Ok(queue.get_request_id(book_id, paragraph_id).await)
    }
}

#[tauri::command]
pub async fn update_config(
    state: tauri::State<'_, Arc<AppState>>,
    config: Config,
) -> Result<(), String> {
    info!("Update config request");
    state
        .update_config(config)
        .await
        .map_err(|err| err.to_string())?;
    info!("Config processed");
    Ok(())
}

#[tauri::command]
pub async fn get_config(state: tauri::State<'_, Arc<AppState>>) -> Result<Config, String> {
    Ok(state.config.read().await.clone())
}

#[tauri::command]
pub async fn translate_paragraph(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    paragraph_id: usize,
    model: usize,
    use_cache: bool,
) -> Result<usize, String> {
    state
        .translate_paragraph(
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
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    paragraph_id: usize,
) -> Result<Option<usize>, String> {
    state
        .get_paragraph_translation_request_id(book_id, paragraph_id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_translation_status(
    state: tauri::State<'_, Arc<AppState>>,
    request_id: usize,
) -> Result<Option<translation_queue::TranslationStatus>, String> {
    let queue = { state.translation_queue.read().await.clone() };
    Ok(match queue {
        Some(q) => q.get_translation_status(request_id).await,
        None => None,
    })
}

#[tauri::command]
pub async fn get_system_definition(
    #[allow(unused_variables)] app: tauri::AppHandle,
    #[allow(unused_variables)] word: String,
    #[allow(unused_variables)] source_lang: String,
    #[allow(unused_variables)] target_lang: String,
) -> Result<Option<library::dictionary::SystemDefinition>, String> {
    #[cfg(target_os = "macos")]
    {
        use std::sync::mpsc::channel;
        let (tx, rx) = channel();

        let word = word.clone();
        let source_lang = source_lang.clone();
        let target_lang = target_lang.clone();

        app.run_on_main_thread(move || {
            let result = library::dictionary::system_macos::get_definition(
                &word,
                &source_lang,
                &target_lang,
            );
            let _ = tx.send(result);
        })
        .map_err(|e| e.to_string())?;

        rx.recv().map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(None)
    }
}

#[tauri::command]
pub async fn show_system_dictionary(
    #[allow(unused_variables)] app: tauri::AppHandle,
    #[allow(unused_variables)] word: String,
) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        app.run_on_main_thread(move || {
            library::dictionary::system_ios::show_dictionary(&word);
        })
        .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "ios"))]
    {
        Ok(())
    }
}
