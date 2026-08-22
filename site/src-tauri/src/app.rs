use std::{
    error::Error,
    fmt::Display,
    fs,
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[cfg(not(target_os = "android"))]
use directories::ProjectDirs;
use isolang::Language;
use library::{
    cache::{GEMINI_PROMPT_CACHE_CAPACITY, TranslationsCache},
    library::{
        Library,
        file_watcher::{LibraryFileChange, LibraryWatcher},
    },
    translation_stats::TranslationSizeCache,
    translator::{
        TranslationProvider,
        catalog::{ModelCatalog, ReqwestListTransport, list_base_url_from_env},
        gemini_cache::GeminiPromptCache,
    },
};
use log::{info, warn};
use tokio::sync::{Mutex, watch};
use uuid::Uuid;

use tauri::Emitter;

use crate::app::{
    anki_sync::AnkiSyncTask, chapter_context::SummaryBackedChapterContext, config::Config,
    summary_generation_queue::SummaryGenerationQueue, translation_queue::TranslationQueue,
};

const EXIT_STOP_QUEUE_TIMEOUT: Duration = Duration::from_secs(2);
const EXIT_SAVE_ALL_TIMEOUT: Duration = Duration::from_secs(10);
const EXIT_CACHE_CLOSE_TIMEOUT: Duration = Duration::from_millis(250);
const DEFAULT_ANKI_SYNC_INTERVAL_SECS: u64 = 300;

pub mod anki_sync;
pub mod chapter_context;
pub mod config;
pub mod gated_state;
pub mod library_view;
pub mod lyrics;
pub mod spotify;
pub mod summary_generation_queue;
pub mod sync;
pub mod sync_daemon;
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

/// Sync-roster device name: OS hostname sans `.local`, or a platform label when
/// the hostname is useless (iOS reports `localhost`).
fn default_device_name() -> String {
    let raw = tauri_plugin_os::hostname();
    let host = raw.trim().trim_end_matches(".local").trim();
    if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
        return host.to_string();
    }
    #[cfg(target_os = "ios")]
    {
        "iPad".to_string()
    }
    #[cfg(not(target_os = "ios"))]
    {
        "FLTS device".to_string()
    }
}

/// Config dir (`config.json`, Syncthing home). `FLTS_CONFIG_DIR` overrides so
/// E2E runs are isolated. Android has no XDG/HOME, so it goes through Tauri's
/// path API — `app` is required there, ignored elsewhere.
fn resolve_config_dir(app: Option<&tauri::AppHandle>) -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("FLTS_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let app = app.ok_or_else(|| {
            anyhow::anyhow!("AppHandle required to resolve config dir on Android")
        })?;
        return Ok(app.path().app_config_dir()?);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let dirs = ProjectDirs::from("com", "TS", "FLTS").ok_or(AppError::ProjectDirsError)?;
        Ok(dirs.config_dir().to_path_buf())
    }
}

/// `<FLTS_CONFIG_DIR>/cache`, or `None` when unset/empty. Pure so tests need no
/// process env.
fn cache_dir_override(env: Option<String>) -> Option<PathBuf> {
    env.filter(|v| !v.is_empty())
        .map(|dir| PathBuf::from(dir).join("cache"))
}

/// Per-platform cache dir (transient, OS-evictable). `FLTS_CONFIG_DIR` overrides
/// as `<dir>/cache`. The empty `ProjectDirs` qualifier is load-bearing: changing
/// it would relocate existing installs' caches.
fn resolve_cache_dir(app: Option<&tauri::AppHandle>) -> anyhow::Result<PathBuf> {
    if let Some(dir) = cache_dir_override(std::env::var("FLTS_CONFIG_DIR").ok()) {
        return Ok(dir);
    }
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let app = app
            .ok_or_else(|| anyhow::anyhow!("AppHandle required to resolve cache dir on Android"))?;
        return Ok(app.path().app_cache_dir()?);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let dirs = ProjectDirs::from("", "TS", "FLTS").ok_or(AppError::ProjectDirsError)?;
        Ok(dirs.cache_dir().to_path_buf())
    }
}

/// App-managed library root; the user never picks it.
/// `FLTS_LIBRARY_DIR` > `<FLTS_CONFIG_DIR>/library` > app-private data dir.
/// The iOS data dir is `Library/Application Support`: invisible to the Files app
/// and backed up by default, so no `isExcludedFromBackup` handling is needed.
/// `app` is required on Android, ignored elsewhere.
fn resolve_library_root(app: Option<&tauri::AppHandle>) -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("FLTS_LIBRARY_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    if let Some(cfg) = std::env::var_os("FLTS_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(cfg).join("library"));
    }
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let app = app.ok_or_else(|| {
            anyhow::anyhow!("AppHandle required to resolve library root on Android")
        })?;
        return Ok(app.path().app_data_dir()?.join("library"));
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let dirs = ProjectDirs::from("com", "TS", "FLTS").ok_or(AppError::ProjectDirsError)?;
        Ok(dirs.data_dir().join("library"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MigrationOutcome {
    /// Source absent or already at the destination.
    NothingToDo,
    Moved,
    /// Destination had content; source left untouched.
    KeptExisting,
}

/// Moves a legacy library into `new_root` only when the destination is absent or
/// empty. The source is removed only after a fully successful copy.
fn migrate_library_files(old: &Path, new_root: &Path) -> anyhow::Result<MigrationOutcome> {
    if old == new_root || !old.exists() {
        return Ok(MigrationOutcome::NothingToDo);
    }

    let new_is_empty = !new_root.exists()
        || fs::read_dir(new_root)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false);
    if !new_is_empty {
        return Ok(MigrationOutcome::KeptExisting);
    }

    if let Some(parent) = new_root.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::rename(old, new_root).is_err() {
        copy_dir_recursive(old, new_root)?;
        fs::remove_dir_all(old)?;
    }
    Ok(MigrationOutcome::Moved)
}

/// Cross-filesystem fallback for when `rename` cannot move the legacy library.
fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn spawn_catalog_prefetch(catalog: Arc<ModelCatalog>, config: &Config) {
    let keys = config.api_keys();
    tauri::async_runtime::spawn(async move {
        for provider in [
            TranslationProvider::Google,
            TranslationProvider::Openai,
            TranslationProvider::Deepseek,
            TranslationProvider::Zai,
            TranslationProvider::Openrouter,
        ] {
            let Some(key) = keys.for_provider(provider).filter(|k| !k.is_empty()) else {
                continue;
            };
            let key = key.to_string();
            let base = list_base_url_from_env(provider);
            catalog.models_for(provider, Some(&key), &base).await;
        }
    });
}

fn api_key_changed(old: Option<&str>, new: Option<&str>) -> bool {
    old.unwrap_or("") != new.unwrap_or("")
}

fn invalidate_changed_api_keys(catalog: &ModelCatalog, old: &Config, new: &Config) {
    let pairs = [
        (
            TranslationProvider::Google,
            old.gemini_api_key.as_deref(),
            new.gemini_api_key.as_deref(),
        ),
        (
            TranslationProvider::Openai,
            old.openai_api_key.as_deref(),
            new.openai_api_key.as_deref(),
        ),
        (
            TranslationProvider::Deepseek,
            old.deepseek_api_key.as_deref(),
            new.deepseek_api_key.as_deref(),
        ),
        (
            TranslationProvider::Zai,
            old.zai_api_key.as_deref(),
            new.zai_api_key.as_deref(),
        ),
        (
            TranslationProvider::Openrouter,
            old.openrouter_api_key.as_deref(),
            new.openrouter_api_key.as_deref(),
        ),
    ];
    for (provider, old_key, new_key) in pairs {
        if api_key_changed(old_key, new_key) {
            catalog.invalidate(provider);
        }
    }
}

pub struct AppState {
    app: tauri::AppHandle,
    config_path: PathBuf,
    config: watch::Sender<Config>,
    /// Everything `eval_config` installs; see `gated_state`.
    gated: crate::app::gated_state::GatedState,
    translation_queue: watch::Sender<Option<Arc<TranslationQueue>>>,
    translation_queue_init_lock: Mutex<()>,
    summary_generation_queue: watch::Sender<Option<Arc<SummaryGenerationQueue>>>,
    summary_generation_queue_init_lock: Mutex<()>,
    watcher: Arc<Mutex<LibraryWatcher>>,
    backfill_lock: Arc<Mutex<()>>,
    /// Serializes config/sync evaluation: the Go engine is one-per-process.
    /// Lock order: eval_lock → translation_queue_init_lock →
    /// summary_generation_queue_init_lock. Task-slot mutexes are leaves.
    eval_lock: Mutex<()>,
    /// Stable across `eval_config` re-spawns. The transient `AnkiSyncTask`
    /// holds a clone and pushes status into it on every tick.
    anki_sync_status: Arc<watch::Sender<crate::app::anki_sync::AnkiSyncStatus>>,
    /// Stable across re-spawns, like `anki_sync_status`.
    sync_status: Arc<watch::Sender<crate::app::sync_daemon::SyncStatus>>,
    translations_cache: tokio::sync::OnceCell<Arc<TranslationsCache>>,
    stats_cache: tokio::sync::OnceCell<Arc<TranslationSizeCache>>,
    gemini_prompt_cache: tokio::sync::OnceCell<Arc<GeminiPromptCache>>,
    model_catalog: Arc<ModelCatalog>,
    pub lyrics_state: crate::app::lyrics::LyricsState,
    pub spotify_web: Arc<crate::app::spotify::web::SpotifyWebState>,
}

impl AppState {
    pub fn new(app: tauri::AppHandle, watcher: Arc<Mutex<LibraryWatcher>>) -> anyhow::Result<Self> {
        info!("Startup!");

        let config_dir = resolve_config_dir(Some(&app))?;

        if !fs::exists(&config_dir)? {
            fs::create_dir_all(&config_dir)?;
        }

        info!("config_dir = {:?}", config_dir);
        let config_path = config_dir.join("config.json");

        let config = if config_path.exists() {
            Config::load(&config_path)?
        } else {
            Config::default()
        };

        let model_catalog = Arc::new(ModelCatalog::new(
            resolve_cache_dir(Some(&app))?,
            Arc::new(ReqwestListTransport::new()),
        ));
        spawn_catalog_prefetch(model_catalog.clone(), &config);

        // Unreachable until a tick proves otherwise: the UI hides the sync
        // button in that state, so it stays hidden until we know.
        let initial_anki_status = crate::app::anki_sync::AnkiSyncStatus {
            state: crate::app::anki_sync::AnkiSyncStatusState::Unreachable,
            ..Default::default()
        };

        Ok(Self {
            app,
            config_path,
            config: watch::channel(config).0,
            gated: crate::app::gated_state::GatedState::new(),
            translation_queue: watch::channel(None).0,
            translation_queue_init_lock: Mutex::new(()),
            summary_generation_queue: watch::channel(None).0,
            summary_generation_queue_init_lock: Mutex::new(()),
            watcher,
            backfill_lock: Arc::new(Mutex::new(())),
            eval_lock: Mutex::new(()),
            anki_sync_status: Arc::new(watch::channel(initial_anki_status).0),
            sync_status: Arc::new(watch::channel(crate::app::sync_daemon::SyncStatus::default()).0),
            translations_cache: tokio::sync::OnceCell::new(),
            stats_cache: tokio::sync::OnceCell::new(),
            gemini_prompt_cache: tokio::sync::OnceCell::new(),
            model_catalog,
            lyrics_state: crate::app::lyrics::LyricsState::new(),
            spotify_web: Arc::new(crate::app::spotify::web::SpotifyWebState::new()),
        })
    }

    pub fn publish_ready(&self, outcome: Result<(), String>) {
        self.gated.publish_ready(outcome);
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

    pub fn config_borrow_sync_device_name(&self) -> Option<String> {
        self.config
            .borrow()
            .sync_device_name
            .clone()
            .filter(|s| !s.trim().is_empty())
    }

    /// Persists `syncEnabled` and re-evaluates config, starting/stopping the
    /// embedded engine.
    pub async fn set_sync_enabled(&self, enabled: bool) -> anyhow::Result<()> {
        let mut config = self.config.borrow().clone();
        config.sync_enabled = enabled;
        self.update_config(config).await
    }

    /// Call on foreground. iOS tears down the engine's sockets (incl. its
    /// loopback REST listener) during suspension and doesn't reliably rebind, so
    /// an unreachable engine is restarted; a healthy one is left alone.
    pub async fn wake_sync(&self) {
        if !self.config.borrow().sync_enabled {
            return;
        }
        // Gate explicitly: a wake must not race startup into a second engine start.
        if let Err(err) = self.gated.await_ready().await {
            warn!("wake_sync: startup did not complete: {err}");
            return;
        }
        let healthy = match self.sync_engine().await.ok().flatten() {
            Some(engine) => {
                crate::app::sync_daemon::probe_healthy(
                    engine.client().as_ref(),
                    crate::app::sync_daemon::WAKE_PROBE_TIMEOUT,
                )
                .await
            }
            None => false,
        };
        if healthy {
            return;
        }
        info!("Sync engine unreachable after wake; restarting");
        let _eval = self.eval_lock.lock().await;
        // Don't bounce an engine another evaluation just restarted.
        if let Some(engine) = self.sync_engine().await.ok().flatten()
            && crate::app::sync_daemon::probe_healthy(
                engine.client().as_ref(),
                crate::app::sync_daemon::WAKE_PROBE_TIMEOUT,
            )
            .await
        {
            return;
        }
        let config = self.config.borrow().clone();
        match resolve_library_root(Some(&self.app)) {
            Ok(root) => self.eval_sync(&config, &root).await,
            Err(err) => warn!("wake_sync: cannot resolve library root: {err}"),
        }
    }

    /// Persists the display name and applies it live, without an engine restart.
    pub async fn set_sync_device_name(&self, name: String) -> anyhow::Result<()> {
        let trimmed = name.trim().to_string();
        let mut config = self.config.borrow().clone();
        config.sync_device_name = Some(trimmed.clone()).filter(|s| !s.is_empty());
        config.save(&self.config_path)?;
        self.config.send_replace(config);

        if !trimmed.is_empty() {
            if let Some(engine) = self.sync_engine().await.map_err(|e| anyhow::anyhow!(e))? {
                engine.set_device_name(&trimmed).await?;
            }
        }
        Ok(())
    }

    pub fn subscribe_library(&self) -> watch::Receiver<Option<Arc<Library>>> {
        self.gated.subscribe_library()
    }

    pub fn notify_library_changed(&self) {
        self.gated.notify_library_changed();
    }

    /// Every command reaches the library through here.
    pub async fn library(&self) -> Result<Arc<Library>, String> {
        self.gated.library().await
    }

    pub fn library_sender(&self) -> Arc<watch::Sender<Option<Arc<Library>>>> {
        self.gated.library_sender()
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
        self.gated.sync_anki_now().await
    }

    pub fn subscribe_sync_status(&self) -> watch::Receiver<crate::app::sync_daemon::SyncStatus> {
        self.sync_status.subscribe()
    }

    pub fn sync_status(&self) -> crate::app::sync_daemon::SyncStatus {
        self.sync_status.borrow().clone()
    }

    /// The running sync engine, if a task is installed.
    pub async fn sync_engine(
        &self,
    ) -> Result<Option<Arc<library::sync::engine::SyncEngine>>, String> {
        self.gated.sync_engine().await
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

    /// Persists and applies a new config. Rebuilds gated state rather than
    /// reading it, so it waits out a startup in flight yet still runs on a
    /// *failed* one — the repair path — and republishes its own outcome.
    pub async fn update_config(&self, config: Config) -> anyhow::Result<()> {
        self.gated
            .await_settled()
            .await
            .map_err(|err| anyhow::anyhow!(err))?;
        let _eval = self.eval_lock.lock().await;
        let outcome = self.apply_config(config).await;
        self.gated
            .publish_ready(outcome.as_ref().map(|_| ()).map_err(|err| err.to_string()));
        outcome
    }

    /// Caller must hold `eval_lock`.
    async fn apply_config(&self, config: Config) -> anyhow::Result<()> {
        // Hold the init locks across stop → library swap so no queue is built
        // against the outgoing Library; drop them before the slow eval_sync
        // tail so translates don't wait on an engine restart.
        let (config, library_root) = {
            let _tq_init = self.translation_queue_init_lock.lock().await;
            let _sq_init = self.summary_generation_queue_init_lock.lock().await;

            // Queues capture translator/summarizer settings at creation.
            self.stop_translation_queue().await;
            self.stop_summary_generation_queue().await;

            // Flush unsaved books before the Library swap.
            self.save_all().await;

            {
                let old = self.config.borrow();
                invalidate_changed_api_keys(&self.model_catalog, &old, &config);
            }

            info!("config = {:?}", config);
            config.save(&self.config_path)?;
            self.config.send_replace(config);
            self.eval_library_config().await?
        };
        self.eval_sync(&config, &library_root).await;
        Ok(())
    }

    pub async fn eval_config(&self) -> anyhow::Result<()> {
        let _eval = self.eval_lock.lock().await;
        let (config, library_root) = self.eval_library_config().await?;
        self.eval_sync(&config, &library_root).await;
        Ok(())
    }

    /// Migrates + opens the library, (re)spawns the Anki task, points the
    /// watcher. Caller must hold `eval_lock`.
    async fn eval_library_config(&self) -> anyhow::Result<(Config, PathBuf)> {
        let config = self.config.borrow().clone();

        let library_root = resolve_library_root(Some(&self.app))?;
        info!("library_root = {library_root:?}");
        self.migrate_legacy_library(&config, &library_root).await?;

        let library = Arc::new(Library::open(library_root.clone()).await?);
        self.gated.install_library(library.clone());

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

        let prior = self.gated.take_anki_task().await;
        if let Some(task) = prior {
            info!("Stopping prior Anki sync task before re-spawn");
            task.shutdown().await;
        }

        // FLTS_DISABLE_ANKI_SYNC=1 suppresses the spawn (CI has no AnkiConnect).
        let disable_env = std::env::var_os("FLTS_DISABLE_ANKI_SYNC");
        if crate::app::anki_sync::anki_sync_disabled(disable_env.as_deref()) {
            info!("Anki sync disabled by FLTS_DISABLE_ANKI_SYNC env var");
            self.set_anki_sync_unreachable("Anki sync disabled by FLTS_DISABLE_ANKI_SYNC env var");
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
            self.gated.install_anki_task(task).await;
            info!("Anki sync task spawned (interval = {interval_secs}s)");
        }

        self.watcher
            .lock()
            .await
            .set_path(&library_root)
            .unwrap_or_else(|err| {
                warn!(
                    "Failed to set watcher path to {}: {}",
                    library_root.display(),
                    err
                )
            });

        Ok((config, library_root))
    }

    /// (Re)starts or tears down the native sync task to match config + env.
    /// Caller must hold `eval_lock`.
    /// Opt-in via `syncEnabled`; `FLTS_DISABLE_SYNC` / `FLTS_MOCK_SYNC` force it
    /// off (CI / E2E); `FLTS_SYNC_HERMETIC` keeps it local (tests / Docker).
    /// Never fails `eval_config` — a sync start error is surfaced via status.
    async fn eval_sync(&self, config: &Config, library_root: &Path) {
        use crate::app::sync_daemon::{SyncStatus, SyncTask, sync_disabled};

        let prior = self.gated.take_sync_task().await;
        if let Some(task) = prior {
            info!("Stopping prior sync task before re-spawn");
            task.shutdown().await;
        }

        let mock = std::env::var_os("FLTS_MOCK_SYNC").is_some_and(|v| !v.is_empty());
        let disabled = sync_disabled(std::env::var_os("FLTS_DISABLE_SYNC").as_deref());

        if !config.sync_enabled {
            info!("Sync disabled (syncEnabled = false)");
            self.sync_status.send_replace(SyncStatus::disabled());
            return;
        }
        if disabled || mock {
            info!("Sync suppressed by env (FLTS_DISABLE_SYNC / FLTS_MOCK_SYNC)");
            self.sync_status.send_replace(SyncStatus::disabled());
            return;
        }

        let home = match resolve_config_dir(Some(&self.app)) {
            Ok(dir) => dir.join("syncthing"),
            Err(err) => {
                warn!("Cannot resolve syncthing home: {err}");
                self.sync_status
                    .send_replace(SyncStatus::error(err.to_string()));
                return;
            }
        };
        let hermetic = std::env::var_os("FLTS_SYNC_HERMETIC").is_some_and(|v| !v.is_empty());
        let device_name = config
            .sync_device_name
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(default_device_name);

        match SyncTask::init(
            home,
            library_root.to_path_buf(),
            device_name,
            hermetic,
            self.sync_status.clone(),
        )
        .await
        {
            Ok(task) => {
                self.gated.install_sync_task(task).await;
                info!("Sync task spawned");
            }
            Err(err) => {
                warn!("Sync engine failed to start: {err}");
                self.sync_status
                    .send_replace(SyncStatus::error(err.to_string()));
            }
        }
    }

    /// Idempotent migration of a `config.library_path` library into the
    /// app-managed root. Never clobbers a populated destination; clears the
    /// pointer so subsequent runs are no-ops.
    async fn migrate_legacy_library(&self, config: &Config, new_root: &Path) -> anyhow::Result<()> {
        let Some(old) = config
            .library_path
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
        else {
            return Ok(());
        };

        match migrate_library_files(&old, new_root)? {
            MigrationOutcome::Moved => info!("Migrated library {old:?} -> {new_root:?}"),
            MigrationOutcome::KeptExisting => warn!(
                "Library destination {new_root:?} already has content; keeping it and leaving \
                 the legacy library at {old:?} untouched"
            ),
            MigrationOutcome::NothingToDo => {}
        }

        self.clear_library_path().await
    }

    /// Drops `library_path` from the persisted config; no-op if already cleared.
    async fn clear_library_path(&self) -> anyhow::Result<()> {
        let mut config = self.config.borrow().clone();
        if config.library_path.is_some() {
            config.library_path = None;
            config.save(&self.config_path)?;
            self.config.send_replace(config);
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

    pub async fn stop_summary_generation_queue(&self) {
        if let Some(queue) = self.summary_generation_queue.send_replace(None) {
            info!("Stopping summary generation queue");
            queue.shutdown().await;
            info!("Summary generation queue stopped");
        }
    }

    pub async fn save_all(&self) {
        let library = self.gated.library_unchecked();
        if let Some(library) = library {
            info!("Saving all dirty books before shutdown");
            library.save_all().await;
        }
    }

    async fn get_translations_cache(&self) -> anyhow::Result<Arc<TranslationsCache>> {
        self.translations_cache
            .get_or_try_init(|| async {
                let cache_dir = resolve_cache_dir(Some(&self.app))?;
                Ok(Arc::new(TranslationsCache::create(&cache_dir).await?))
            })
            .await
            .cloned()
    }

    async fn get_stats_cache(&self) -> anyhow::Result<Arc<TranslationSizeCache>> {
        self.stats_cache
            .get_or_try_init(|| async {
                let cache_dir = resolve_cache_dir(Some(&self.app))?;
                Ok(Arc::new(TranslationSizeCache::create(&cache_dir).await?))
            })
            .await
            .cloned()
    }

    async fn get_gemini_prompt_cache(&self) -> anyhow::Result<Arc<GeminiPromptCache>> {
        self.gemini_prompt_cache
            .get_or_try_init(|| async {
                let cache_dir = resolve_cache_dir(Some(&self.app))?.join("gemini_caches");
                GeminiPromptCache::open(&cache_dir, GEMINI_PROMPT_CACHE_CAPACITY).await
            })
            .await
            .cloned()
    }

    pub async fn shutdown(&self) {
        // Best effort: no shutdown step may hang app exit.
        run_exit_step(
            "translation queue shutdown",
            EXIT_STOP_QUEUE_TIMEOUT,
            self.stop_translation_queue(),
        )
        .await;
        // Take the task out of its slot before awaiting, so a long tick can't
        // block inside the mutex. No final sync_pass: a flush against a slow
        // AnkiConnect would stall exit, and the next launch syncs immediately.
        let anki_task = self.gated.take_anki_task().await;
        if let Some(task) = anki_task {
            run_exit_step(
                "anki sync shutdown",
                EXIT_STOP_QUEUE_TIMEOUT,
                task.shutdown(),
            )
            .await;
        }
        let sync_task = self.gated.take_sync_task().await;
        if let Some(task) = sync_task {
            run_exit_step(
                "sync engine shutdown",
                EXIT_STOP_QUEUE_TIMEOUT,
                task.shutdown(),
            )
            .await;
        }
        run_exit_step("save all", EXIT_SAVE_ALL_TIMEOUT, self.save_all()).await;
        self.close_caches_for_exit().await;
    }

    pub async fn handle_file_change_event(&self, event: &LibraryFileChange) -> anyhow::Result<()> {
        let library = self.gated.library_unchecked();
        let Some(library) = library else {
            return Ok(());
        };

        let had_effect = library.handle_file_change_event(event).await?;

        // `had_effect` gates per arm, not the whole match: the library has no
        // card cache, so CardChanged is always false yet must still notify.
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
                // Unconditional: the frontend invalidates its translation cache here.
                info!("Emitting \"cards_updated\"");
                self.app.emit("cards_updated", ())?;
            }
            _ => {}
        }

        Ok(())
    }

    async fn get_or_init_translation_queue(&self) -> anyhow::Result<Arc<TranslationQueue>> {
        if let Some(queue) = self.translation_queue.borrow().clone() {
            return Ok(queue);
        }

        // Before the lock: waiting for startup while holding an init lock would
        // invert the lock order against anything startup may come to need.
        self.gated
            .await_ready()
            .await
            .map_err(|err| anyhow::anyhow!(err))?;

        let _guard = self.translation_queue_init_lock.lock().await;

        // Another caller may have populated the queue while we waited.
        if let Some(queue) = self.translation_queue.borrow().clone() {
            return Ok(queue);
        }

        // Read under the init lock: update_config holds it across the Library
        // swap, so this can never be the outgoing instance.
        let library = self
            .gated
            .library_unchecked()
            .ok_or_else(|| anyhow::anyhow!("library is not configured"))?;

        let config = self.config.borrow().clone();
        let cache = self.get_translations_cache().await?;
        let stats_cache = self.get_stats_cache().await?;
        let gemini_prompt_cache = self.get_gemini_prompt_cache().await?;
        let summary_queue = self.get_or_init_summary_generation_queue().await?;
        let context_provider: Arc<dyn library::translator::ChapterContextProvider> =
            Arc::new(SummaryBackedChapterContext {
                queue: summary_queue,
                library_rx: self.subscribe_library(),
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
    ) -> anyhow::Result<Arc<SummaryGenerationQueue>> {
        if let Some(queue) = self.summary_generation_queue.borrow().clone() {
            return Ok(queue);
        }

        let _guard = self.summary_generation_queue_init_lock.lock().await;

        if let Some(queue) = self.summary_generation_queue.borrow().clone() {
            return Ok(queue);
        }

        let config = self.config.borrow().clone();
        let queue =
            SummaryGenerationQueue::init(self.subscribe_library(), &config, self.app.clone());

        self.summary_generation_queue
            .send_replace(Some(queue.clone()));
        Ok(queue)
    }

    pub async fn translate_paragraph(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
        model: String,
        use_cache: bool,
    ) -> anyhow::Result<usize> {
        let model = self.config.borrow().resolved_model_id(&model);
        let queue = self.get_or_init_translation_queue().await?;
        queue
            .translate(book_id, paragraph_id, model, use_cache)
            .await
    }

    pub async fn translate_chapter(
        &self,
        book_id: Uuid,
        chapter_id: usize,
        model: String,
        use_cache: bool,
    ) -> anyhow::Result<usize> {
        let library = self.library().await.map_err(|err| anyhow::anyhow!(err))?;

        let target_language_id = { self.config.borrow().target_language_id.clone() };
        let target_language = Language::from_639_3(&target_language_id)
            .ok_or_else(|| anyhow::anyhow!("invalid target language: {target_language_id}"))?;

        // Drop the book lock before enqueueing: queue.translate re-acquires it.
        let untranslated: Vec<usize> = {
            let book = library.get_book(&book_id).await?;
            let book = book.lock().await;
            let translation_arc = book.get_translation(&target_language).await;
            let translation_guard = match &translation_arc {
                Some(arc) => Some(arc.lock().await),
                None => None,
            };
            let chapter = book.book.chapter_view(chapter_id);
            chapter
                .paragraphs()
                .filter(|p| {
                    translation_guard
                        .as_ref()
                        .map(|t| t.paragraph_view(p.id).is_none())
                        .unwrap_or(true)
                })
                .map(|p| p.id)
                .collect()
        };

        let model = self.config.borrow().resolved_model_id(&model);
        let queue = self.get_or_init_translation_queue().await?;
        for paragraph_id in &untranslated {
            // Swallow per-item errors so one bad paragraph doesn't abandon the rest.
            if let Err(err) = queue
                .translate(book_id, *paragraph_id, model.clone(), use_cache)
                .await
            {
                warn!("translate_chapter: failed to enqueue paragraph {paragraph_id}: {err}");
            }
        }
        Ok(untranslated.len())
    }

    pub async fn get_paragraph_translation_activity(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
    ) -> anyhow::Result<Option<translation_queue::ParagraphTranslationActivity>> {
        // Pure read: initializing the queue here could pin a stale Library inside
        // update_config's stop→eval window. Bind first, so no read-guard crosses
        // the await.
        let queue = self.translation_queue.borrow().clone();
        match queue {
            Some(queue) => Ok(queue.get_active_translation(book_id, paragraph_id).await),
            None => Ok(None),
        }
    }

    pub async fn list_paragraph_translation_activity(
        &self,
    ) -> anyhow::Result<Vec<translation_queue::ActiveParagraphTranslation>> {
        // Pure read; see get_paragraph_translation_activity.
        let queue = self.translation_queue.borrow().clone();
        match queue {
            Some(queue) => Ok(queue.list_active_translations().await),
            None => Ok(Vec::new()),
        }
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

    /// Work on spawn_blocking must stay preemptable by run_exit_step's timeout;
    /// a raw blocking call has no await point.
    #[tokio::test]
    async fn exit_step_times_out_when_step_blocks_a_thread_via_spawn_blocking() {
        let started = Instant::now();
        let success = run_exit_step("blocked step", Duration::from_millis(50), async {
            let _ = tokio::task::spawn_blocking(|| {
                std::thread::sleep(Duration::from_millis(500));
            })
            .await;
        })
        .await;
        assert!(!success, "step must time out, not complete");
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "timeout must preempt the blocked thread, elapsed {:?}",
            started.elapsed()
        );
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("flts-mig-{tag}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn migration_moves_into_empty_destination() {
        let base = scratch_dir("move");
        let old = base.join("old");
        let new = base.join("new");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("book.dat"), b"hello").unwrap();

        let outcome = migrate_library_files(&old, &new).unwrap();

        assert_eq!(outcome, MigrationOutcome::Moved);
        assert!(!old.exists(), "source removed after move");
        assert_eq!(fs::read(new.join("book.dat")).unwrap(), b"hello");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn migration_is_non_destructive_when_destination_populated() {
        let base = scratch_dir("keep");
        let old = base.join("old");
        let new = base.join("new");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("book.dat"), b"legacy").unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("book.dat"), b"current").unwrap();

        let outcome = migrate_library_files(&old, &new).unwrap();

        assert_eq!(outcome, MigrationOutcome::KeptExisting);
        assert!(old.exists(), "legacy library left untouched");
        assert_eq!(fs::read(new.join("book.dat")).unwrap(), b"current");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn migration_noop_when_source_missing_or_same() {
        let base = scratch_dir("noop");
        let old = base.join("old");
        let new = base.join("new");

        // Source missing.
        assert_eq!(
            migrate_library_files(&old, &new).unwrap(),
            MigrationOutcome::NothingToDo
        );

        // Source == destination.
        fs::create_dir_all(&old).unwrap();
        assert_eq!(
            migrate_library_files(&old, &old).unwrap(),
            MigrationOutcome::NothingToDo
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn resolve_library_root_honors_overrides() {
        // FLTS_LIBRARY_DIR wins outright.
        unsafe { std::env::set_var("FLTS_LIBRARY_DIR", "/tmp/flts-explicit") };
        assert_eq!(
            resolve_library_root(None).unwrap(),
            PathBuf::from("/tmp/flts-explicit")
        );
        unsafe { std::env::remove_var("FLTS_LIBRARY_DIR") };

        // Else <FLTS_CONFIG_DIR>/library for E2E isolation.
        unsafe { std::env::set_var("FLTS_CONFIG_DIR", "/tmp/flts-cfg") };
        assert_eq!(
            resolve_library_root(None).unwrap(),
            PathBuf::from("/tmp/flts-cfg/library")
        );
        unsafe { std::env::remove_var("FLTS_CONFIG_DIR") };
    }

    #[test]
    fn cache_dir_override_follows_config_dir() {
        assert_eq!(
            cache_dir_override(Some("/tmp/flts-cfg".into())),
            Some(PathBuf::from("/tmp/flts-cfg/cache"))
        );
        assert_eq!(cache_dir_override(Some(String::new())), None);
        assert_eq!(cache_dir_override(None), None);
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

/// Deletes every FLTS-created Gemini server-side context cache (display name
/// prefix "flts-") and clears the local pointers; returns the count. Safe during
/// active translation: chapters self-heal via the 403/404 evict-and-retry path.
#[tauri::command]
pub async fn purge_gemini_caches(state: tauri::State<'_, Arc<AppState>>) -> Result<usize, String> {
    let api_key = state
        .config
        .borrow()
        .gemini_api_key
        .clone()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "No Gemini API key configured".to_string())?;
    let cache = state
        .get_gemini_prompt_cache()
        .await
        .map_err(|err| err.to_string())?;
    info!("Purging Gemini server caches");
    let report = cache
        .purge_all(&api_key)
        .await
        .map_err(|err| err.to_string())?;
    info!(
        "Gemini cache purge: {} deleted, {} failed",
        report.deleted,
        report.failures.len()
    );
    if report.failures.is_empty() {
        Ok(report.deleted)
    } else {
        Err(format!(
            "Removed {} cache(s), but {} deletion(s) failed (first: {})",
            report.deleted,
            report.failures.len(),
            report.failures[0]
        ))
    }
}

/// The app-managed library location, for read-only display in settings.
#[tauri::command]
pub async fn get_library_root(app: tauri::AppHandle) -> Result<String, String> {
    resolve_library_root(Some(&app))
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|err| err.to_string())
}

/// Opens the library location in the desktop OS file manager.
#[tauri::command]
pub async fn reveal_library_root(app: tauri::AppHandle) -> Result<(), String> {
    let path = resolve_library_root(Some(&app)).map_err(|err| err.to_string())?;
    let _ = fs::create_dir_all(&path);
    reveal_in_file_manager(&path).map_err(|err| err.to_string())
}

fn reveal_in_file_manager(path: &Path) -> anyhow::Result<()> {
    // Harness runs must not open a file manager window.
    if std::env::var_os("FLTS_E2E_BRIDGE_PORT").is_some() {
        info!("reveal suppressed under e2e bridge: {}", path.display());
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer").arg(path).spawn()?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        anyhow::bail!("revealing {path:?} is not supported on this platform")
    }
}

#[tauri::command]
pub async fn translate_paragraph(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    paragraph_id: usize,
    model: String,
    use_cache: bool,
) -> Result<usize, String> {
    state
        .translate_paragraph(book_id, paragraph_id, model, use_cache)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn translate_chapter(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    chapter_id: usize,
    model: String,
    use_cache: bool,
) -> Result<usize, String> {
    state
        .translate_chapter(book_id, chapter_id, model, use_cache)
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
pub async fn list_paragraph_translation_activity(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Vec<translation_queue::ActiveParagraphTranslation>, String> {
    state
        .list_paragraph_translation_activity()
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
        let (tx, rx) = tokio::sync::oneshot::channel();

        app.run_on_main_thread(move || {
            let _ = tx.send(library::system_dictionary::system_macos::get_definition(
                &word,
                &source_lang,
                &target_lang,
            ));
        })
        .map_err(|e| e.to_string())?;

        // Bound the wait so a stalled main loop can't leave the invoke pending.
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => Err("system dictionary lookup was dropped".to_string()),
            Err(_) => Err("system dictionary lookup timed out".to_string()),
        }
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
