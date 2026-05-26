use std::{
    error::Error,
    fmt::Display,
    fs,
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use directories::ProjectDirs;
use isolang::Language;
use library::{
    cache::{GEMINI_PROMPT_CACHE_CAPACITY, TranslationsCache},
    library::{
        Library,
        file_watcher::{LibraryFileChange, LibraryWatcher},
    },
    translation_stats::TranslationSizeCache,
    translator::{TranslationModel, gemini_cache::GeminiPromptCache},
};
use log::{info, warn};
use tokio::sync::{Mutex, watch};
use uuid::Uuid;

use tauri::Emitter;

use crate::app::{
    anki_sync::AnkiSyncTask, chapter_context::SummaryBackedChapterContext, config::Config,
    summary_generation_queue::SummaryGenerationQueue, translation_queue::TranslationQueue,
};

#[cfg(mobile)]
fn document_dir() -> Option<std::path::PathBuf> {
    directories::UserDirs::new().and_then(|u| u.document_dir().map(std::path::Path::to_owned))
}

const EXIT_STOP_QUEUE_TIMEOUT: Duration = Duration::from_secs(2);
const EXIT_SAVE_ALL_TIMEOUT: Duration = Duration::from_secs(10);
const EXIT_CACHE_CLOSE_TIMEOUT: Duration = Duration::from_millis(250);
const EXIT_FINAL_SYNC_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_ANKI_SYNC_INTERVAL_SECS: u64 = 300;

pub mod anki_sync;
pub mod chapter_context;
pub mod config;
pub mod library_view;
pub mod lyrics;
pub mod spotify;
pub mod summary_generation_queue;
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
    config: watch::Sender<Config>,
    library: Arc<watch::Sender<Option<Arc<Library>>>>,
    translation_queue: watch::Sender<Option<Arc<TranslationQueue>>>,
    translation_queue_init_lock: Mutex<()>,
    summary_generation_queue: watch::Sender<Option<Arc<SummaryGenerationQueue>>>,
    summary_generation_queue_init_lock: Mutex<()>,
    watcher: Arc<Mutex<LibraryWatcher>>,
    backfill_lock: Arc<Mutex<()>>,
    anki_sync_task: Mutex<Option<Arc<AnkiSyncTask>>>,
    /// Stable across `eval_config` re-spawns. The transient `AnkiSyncTask`
    /// holds a clone and pushes status into it on every tick.
    anki_sync_status: Arc<watch::Sender<crate::app::anki_sync::AnkiSyncStatus>>,
    translations_cache: tokio::sync::OnceCell<Arc<TranslationsCache>>,
    stats_cache: tokio::sync::OnceCell<Arc<TranslationSizeCache>>,
    gemini_prompt_cache: tokio::sync::OnceCell<Arc<GeminiPromptCache>>,
    pub lyrics_state: crate::app::lyrics::LyricsState,
    pub spotify_web: Arc<crate::app::spotify::web::SpotifyWebState>,
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

        // Initial status: Unreachable until the first periodic / on-demand
        // tick proves otherwise. UI hides the sync button on Unreachable, so
        // the safe default is "hidden until we know."
        let initial_anki_status = crate::app::anki_sync::AnkiSyncStatus {
            state: crate::app::anki_sync::AnkiSyncStatusState::Unreachable,
            ..Default::default()
        };

        Ok(Self {
            app,
            config_path,
            config: watch::channel(config).0,
            library: Arc::new(watch::channel::<Option<Arc<Library>>>(None).0),
            translation_queue: watch::channel(None).0,
            translation_queue_init_lock: Mutex::new(()),
            summary_generation_queue: watch::channel(None).0,
            summary_generation_queue_init_lock: Mutex::new(()),
            watcher,
            backfill_lock: Arc::new(Mutex::new(())),
            anki_sync_task: Mutex::new(None),
            anki_sync_status: Arc::new(watch::channel(initial_anki_status).0),
            translations_cache: tokio::sync::OnceCell::new(),
            stats_cache: tokio::sync::OnceCell::new(),
            gemini_prompt_cache: tokio::sync::OnceCell::new(),
            lyrics_state: crate::app::lyrics::LyricsState::new(),
            spotify_web: Arc::new(crate::app::spotify::web::SpotifyWebState::new()),
        })
    }

    pub fn subscribe_config(&self) -> watch::Receiver<Config> {
        self.config.subscribe()
    }

    pub fn config_borrow_client_id(&self) -> Option<String> {
        self.config
            .borrow()
            .spotify_client_id
            .clone()
            .filter(|s| !s.trim().is_empty())
    }

    pub fn subscribe_library(&self) -> watch::Receiver<Option<Arc<Library>>> {
        self.library.subscribe()
    }

    pub fn notify_library_changed(&self) {
        self.library.send_modify(|_| {});
    }

    pub fn library_sender(&self) -> Arc<watch::Sender<Option<Arc<Library>>>> {
        Arc::clone(&self.library)
    }

    pub fn subscribe_anki_sync_status(
        &self,
    ) -> watch::Receiver<crate::app::anki_sync::AnkiSyncStatus> {
        self.anki_sync_status.subscribe()
    }

    pub fn anki_sync_status(&self) -> crate::app::anki_sync::AnkiSyncStatus {
        self.anki_sync_status.borrow().clone()
    }

    pub async fn sync_anki_now(&self) -> anyhow::Result<crate::app::anki_sync::SyncReportDto> {
        crate::app::anki_sync::sync_now_or_err(&self.anki_sync_task).await
    }

    fn set_anki_sync_unreachable(&self, reason: &str) {
        self.anki_sync_status
            .send_replace(crate::app::anki_sync::AnkiSyncStatus {
                state: crate::app::anki_sync::AnkiSyncStatusState::Unreachable,
                last_error: Some(reason.to_owned()),
                last_finished_at_ms: None,
                last_report: None,
            });
    }

    pub async fn update_config(&self, config: Config) -> anyhow::Result<()> {
        #[cfg(mobile)]
        let mut config = config;

        // Translator settings (provider/key/model) are captured when the translation queue is created.
        // Reset it so the next translation uses the latest config.
        self.stop_translation_queue().await;

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
        self.config.send_replace(config);
        self.eval_config().await?;
        Ok(())
    }

    pub async fn eval_config(&self) -> anyhow::Result<()> {
        let config = self.config.borrow().clone();
        let library_path = config.library_path.clone();

        info!("library_path = {library_path:?}");

        if let Some(library_path) = library_path {
            let library = Arc::new(Library::open(PathBuf::from(&library_path)).await?);
            self.library.send_replace(Some(library.clone()));

            if std::env::var_os("FLTS_ENABLE_CARD_BACKFILL").is_some_and(|v| !v.is_empty()) {
                let backfill_lock = self.backfill_lock.clone();
                let backfill_library = library.clone();
                tauri::async_runtime::spawn(async move {
                    let Ok(_guard) = backfill_lock.try_lock() else {
                        info!("Card backfill skipped: already in progress");
                        return;
                    };
                    if let Err(err) = backfill_library.backfill_cards_from_translations().await {
                        warn!("Card backfill failed: {err}");
                    }
                });
            } else {
                info!("Card backfill disabled: set FLTS_ENABLE_CARD_BACKFILL=1 to enable");
            }

            // Stop any prior Anki sync task (config may have changed).
            if let Some(task) = self.anki_sync_task.lock().await.take() {
                info!("Stopping prior Anki sync task before re-spawn");
                task.shutdown().await;
            }

            // Stage 8: sync is ON by default. Set FLTS_DISABLE_ANKI_SYNC=1
            // (e.g. on CI machines without AnkiConnect) to suppress the
            // task spawn.
            let disable_env = std::env::var_os("FLTS_DISABLE_ANKI_SYNC");
            if crate::app::anki_sync::anki_sync_disabled(disable_env.as_deref()) {
                info!("Anki sync disabled by FLTS_DISABLE_ANKI_SYNC env var");
                self.set_anki_sync_unreachable(
                    "Anki sync disabled by FLTS_DISABLE_ANKI_SYNC env var",
                );
            } else {
                let endpoint = config
                    .anki_endpoint
                    .clone()
                    .unwrap_or_else(|| "http://127.0.0.1:8765".to_owned());
                let api_key = config.anki_api_key.clone();
                let client: Arc<dyn library::anki::connect::AnkiConnect> =
                    library::anki::connect::get_anki_connect(endpoint, api_key).into();
                let interval_secs = std::env::var("FLTS_ANKI_SYNC_INTERVAL_SECS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(DEFAULT_ANKI_SYNC_INTERVAL_SECS);
                let task = AnkiSyncTask::init(
                    library.clone(),
                    client,
                    Duration::from_secs(interval_secs),
                    self.anki_sync_status.clone(),
                );
                *self.anki_sync_task.lock().await = Some(task);
                info!("Anki sync task spawned (interval = {interval_secs}s)");
            }

            self.watcher
                .lock()
                .await
                .set_path(&Path::new(&library_path).to_path_buf())
                .unwrap_or_else(|err| {
                    warn!("Failed to set watcher path to {}: {}", library_path, err)
                });
        } else {
            self.library.send_replace(None);
            self.stop_translation_queue().await;
            // No library = no sync. UI hides the button.
            if let Some(task) = self.anki_sync_task.lock().await.take() {
                task.shutdown().await;
            }
            self.set_anki_sync_unreachable("Library not configured");
        }

        Ok(())
    }

    pub async fn stop_translation_queue(&self) {
        if let Some(queue) = self.translation_queue.send_replace(None) {
            info!("Stopping translation queue");
            queue.shutdown().await;
            info!("Translation queue stopped");
        }
    }

    pub async fn save_all(&self) {
        if let Some(library) = self.library.borrow().clone() {
            info!("Saving all dirty books before shutdown");
            library.save_all().await;
        }
    }

    async fn get_translations_cache(&self) -> anyhow::Result<Arc<TranslationsCache>> {
        self.translations_cache
            .get_or_try_init(|| async {
                let dirs = ProjectDirs::from("", "TS", "FLTS").ok_or(AppError::ProjectDirsError)?;
                let cache_dir = dirs.cache_dir();
                Ok(Arc::new(TranslationsCache::create(cache_dir).await?))
            })
            .await
            .cloned()
    }

    async fn get_stats_cache(&self) -> anyhow::Result<Arc<TranslationSizeCache>> {
        self.stats_cache
            .get_or_try_init(|| async {
                let dirs = ProjectDirs::from("", "TS", "FLTS").ok_or(AppError::ProjectDirsError)?;
                let cache_dir = dirs.cache_dir();
                Ok(Arc::new(TranslationSizeCache::create(cache_dir).await?))
            })
            .await
            .cloned()
    }

    async fn get_gemini_prompt_cache(&self) -> anyhow::Result<Arc<GeminiPromptCache>> {
        self.gemini_prompt_cache
            .get_or_try_init(|| async {
                let dirs = ProjectDirs::from("", "TS", "FLTS").ok_or(AppError::ProjectDirsError)?;
                let cache_dir = dirs.cache_dir().join("gemini_caches");
                GeminiPromptCache::open(&cache_dir, GEMINI_PROMPT_CACHE_CAPACITY).await
            })
            .await
            .cloned()
    }

    pub async fn shutdown(&self) {
        // Best effort only: do not let app exit hang forever on any shutdown step.
        run_exit_step(
            "translation queue shutdown",
            EXIT_STOP_QUEUE_TIMEOUT,
            self.stop_translation_queue(),
        )
        .await;
        // Pull task out of the slot under lock; release the lock before awaiting
        // so we never block on a long-running tick from inside the mutex.
        let anki_task = self.anki_sync_task.lock().await.take();
        if let Some(task) = anki_task {
            run_exit_step("anki final sync", EXIT_FINAL_SYNC_TIMEOUT, async {
                if let Err(err) = task.run_one_pass().await {
                    warn!("Anki final sync_pass failed: {err}");
                }
                task.shutdown().await;
            })
            .await;
        }
        run_exit_step("save all", EXIT_SAVE_ALL_TIMEOUT, self.save_all()).await;
        self.close_caches_for_exit().await;
    }

    pub async fn handle_file_change_event(&self, event: &LibraryFileChange) -> anyhow::Result<()> {
        let library = self.library.borrow().clone();
        let Some(library) = library else {
            return Ok(());
        };

        let had_effect = library.handle_file_change_event(event).await?;

        // Note: do not gate the entire match on `had_effect`. CardChanged
        // always returns Ok(false) from the library handler (no in-memory
        // card cache), and its emit must run regardless. Per-arm `if
        // had_effect` guards on BookChanged / TranslationChanged below
        // preserve the prior gating for those variants only.
        match event {
            LibraryFileChange::BookChanged { modified: _, uuid } if had_effect => {
                info!("Emitting \"book_updated\" for {uuid}");
                self.app.emit("book_updated", uuid)?;
                self.notify_library_changed();
            }
            LibraryFileChange::TranslationChanged {
                modified: _,
                from: _,
                to,
                uuid,
            } if had_effect => {
                let target_language_id = { self.config.borrow().target_language_id.clone() };
                let target_language = Language::from_639_3(&target_language_id);

                if target_language == Some(*to) {
                    info!("Emitting \"book_updated\" for {uuid}");
                    self.app.emit("book_updated", uuid)?;
                    self.notify_library_changed();
                }
            }
            LibraryFileChange::CardChanged { .. } => {
                // Always emit — the library doesn't cache cards, so `had_effect`
                // is unconditionally false here. The frontend invalidates its
                // translation cache on this signal.
                info!("Emitting \"cards_updated\"");
                self.app.emit("cards_updated", ())?;
            }
            _ => {}
        }

        Ok(())
    }

    async fn get_or_init_translation_queue(
        &self,
        library: Arc<Library>,
    ) -> anyhow::Result<Arc<TranslationQueue>> {
        if let Some(queue) = self.translation_queue.borrow().clone() {
            return Ok(queue);
        }

        let _guard = self.translation_queue_init_lock.lock().await;

        // Another caller may have populated the queue while we were waiting.
        if let Some(queue) = self.translation_queue.borrow().clone() {
            return Ok(queue);
        }

        let config = self.config.borrow().clone();
        let cache = self.get_translations_cache().await?;
        let stats_cache = self.get_stats_cache().await?;
        let gemini_prompt_cache = self.get_gemini_prompt_cache().await?;
        let summary_queue = self.get_or_init_summary_generation_queue(library.clone()).await?;
        let context_provider: Arc<dyn library::translator::ChapterContextProvider> =
            Arc::new(SummaryBackedChapterContext {
                queue: summary_queue,
                library: library.clone(),
            });
        let queue = TranslationQueue::init(
            library,
            cache,
            stats_cache,
            gemini_prompt_cache,
            context_provider,
            &config,
            self.app.clone(),
            self.library_sender(),
        )
        .ok_or(AppError::NoTranslationQueueError)?;

        self.translation_queue.send_replace(Some(queue.clone()));
        Ok(queue)
    }

    pub async fn get_or_init_summary_generation_queue(
        &self,
        library: Arc<Library>,
    ) -> anyhow::Result<Arc<SummaryGenerationQueue>> {
        if let Some(queue) = self.summary_generation_queue.borrow().clone() {
            return Ok(queue);
        }

        let _guard = self.summary_generation_queue_init_lock.lock().await;

        if let Some(queue) = self.summary_generation_queue.borrow().clone() {
            return Ok(queue);
        }

        let config = self.config.borrow().clone();
        let queue = SummaryGenerationQueue::init(library, &config, self.app.clone());

        self.summary_generation_queue
            .send_replace(Some(queue.clone()));
        Ok(queue)
    }

    pub async fn translate_paragraph(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
        model: TranslationModel,
        use_cache: bool,
    ) -> anyhow::Result<usize> {
        let library = self
            .library
            .borrow()
            .clone()
            .ok_or(AppError::NoLibraryError)?;
        let queue = self.get_or_init_translation_queue(library).await?;
        queue
            .translate(book_id, paragraph_id, model, use_cache)
            .await
    }

    pub async fn get_paragraph_translation_activity(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
    ) -> anyhow::Result<Option<translation_queue::ParagraphTranslationActivity>> {
        let library = self
            .library
            .borrow()
            .clone()
            .ok_or(AppError::NoLibraryError)?;
        let queue = self.get_or_init_translation_queue(library).await?;
        Ok(queue.get_active_translation(book_id, paragraph_id).await)
    }
}

async fn run_exit_step<F>(step_name: &str, timeout: Duration, future: F) -> bool
where
    F: Future<Output = ()>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(()) => true,
        Err(_) => {
            warn!("Timed out during {step_name} after {:?}", timeout);
            false
        }
    }
}

impl AppState {
    async fn close_caches_for_exit(&self) {
        if let Some(cache) = self.translations_cache.get() {
            info!("Closing translations cache");
            if run_exit_step(
                "translations cache close",
                EXIT_CACHE_CLOSE_TIMEOUT,
                cache.close(),
            )
            .await
            {
                info!("Translations cache closed");
            }
        }
        if let Some(cache) = self.stats_cache.get() {
            info!("Closing translation stats cache");
            if run_exit_step(
                "translation stats cache close",
                EXIT_CACHE_CLOSE_TIMEOUT,
                cache.close(),
            )
            .await
            {
                info!("Translation stats cache closed");
            }
        }
        if let Some(cache) = self.gemini_prompt_cache.get() {
            info!("Closing Gemini prompt cache");
            if run_exit_step(
                "gemini prompt cache close",
                EXIT_CACHE_CLOSE_TIMEOUT,
                cache.close(),
            )
            .await
            {
                info!("Gemini prompt cache closed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{future::pending, sync::atomic::AtomicBool, time::Instant};

    use super::*;

    #[tokio::test]
    async fn exit_step_completes_when_future_finishes() {
        let completed = Arc::new(AtomicBool::new(false));
        let completed_flag = completed.clone();

        let success = run_exit_step("quick step", Duration::from_secs(1), async move {
            completed_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .await;

        assert!(success);
        assert!(completed.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn exit_step_times_out_instead_of_hanging() {
        let start = Instant::now();

        let success = run_exit_step("hung step", Duration::from_millis(50), pending::<()>()).await;

        assert!(!success);
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}

#[tauri::command]
pub async fn get_anki_sync_status(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<crate::app::anki_sync::AnkiSyncStatus, String> {
    Ok(state.anki_sync_status())
}

#[tauri::command]
pub async fn sync_anki_now(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<crate::app::anki_sync::SyncReportDto, String> {
    state.sync_anki_now().await.map_err(|err| err.to_string())
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
    Ok(state.config.borrow().clone())
}

#[tauri::command]
pub async fn translate_paragraph(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    paragraph_id: usize,
    model: TranslationModel,
    use_cache: bool,
) -> Result<usize, String> {
    state
        .translate_paragraph(book_id, paragraph_id, model, use_cache)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_paragraph_translation_activity(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    paragraph_id: usize,
) -> Result<Option<translation_queue::ParagraphTranslationActivity>, String> {
    state
        .get_paragraph_translation_activity(book_id, paragraph_id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_system_definition(
    #[allow(unused_variables)] app: tauri::AppHandle,
    #[allow(unused_variables)] word: String,
    #[allow(unused_variables)] source_lang: String,
    #[allow(unused_variables)] target_lang: String,
) -> Result<Option<library::system_dictionary::SystemDefinition>, String> {
    #[cfg(target_os = "macos")]
    {
        use std::sync::mpsc::channel;
        let (tx, rx) = channel();

        let word = word.clone();
        let source_lang = source_lang.clone();
        let target_lang = target_lang.clone();

        app.run_on_main_thread(move || {
            let result = library::system_dictionary::system_macos::get_definition(
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
            library::system_dictionary::system_ios::show_dictionary(&word);
        })
        .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "ios"))]
    {
        Ok(())
    }
}
