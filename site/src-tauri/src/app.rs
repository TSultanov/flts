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

const EXIT_STOP_QUEUE_TIMEOUT: Duration = Duration::from_secs(2);
const EXIT_SAVE_ALL_TIMEOUT: Duration = Duration::from_secs(10);
const EXIT_CACHE_CLOSE_TIMEOUT: Duration = Duration::from_millis(250);
const DEFAULT_ANKI_SYNC_INTERVAL_SECS: u64 = 300;

pub mod anki_sync;
pub mod chapter_context;
pub mod config;
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

/// A sensible default device name for the sync roster. Uses the OS hostname
/// (stripping a trailing `.local`), but falls back to a platform label when the
/// hostname is missing or useless — notably iOS, where it is `localhost`.
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

/// Resolves the app config directory (holds `config.json` and, from Phase 2,
/// the Syncthing home). Honors `FLTS_CONFIG_DIR` so E2E harnesses get a fully
/// isolated config; otherwise the per-platform default.
///
/// On Android the `directories` crate returns no `ProjectDirs` (there is no
/// XDG/HOME in the app sandbox), so we resolve via Tauri's path API, which maps
/// to the app-private internal-storage dir; every other platform keeps
/// `ProjectDirs.config_dir()`. `app` is required on Android, ignored elsewhere.
fn resolve_config_dir(app: Option<&tauri::AppHandle>) -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("FLTS_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let app = app.ok_or_else(|| anyhow::anyhow!("AppHandle required to resolve config dir on Android"))?;
        return Ok(app.path().app_config_dir()?);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let dirs = ProjectDirs::from("com", "TS", "FLTS").ok_or(AppError::ProjectDirsError)?;
        Ok(dirs.config_dir().to_path_buf())
    }
}

/// Resolves the per-platform cache directory (transient, OS-evictable). Mirrors
/// [`resolve_config_dir`]'s Android handling; non-Android keeps the historical
/// `ProjectDirs::from("", "TS", "FLTS").cache_dir()` (empty qualifier) so
/// existing installs' cache locations don't move.
fn resolve_cache_dir(app: Option<&tauri::AppHandle>) -> anyhow::Result<PathBuf> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let app = app.ok_or_else(|| anyhow::anyhow!("AppHandle required to resolve cache dir on Android"))?;
        return Ok(app.path().app_cache_dir()?);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let dirs = ProjectDirs::from("", "TS", "FLTS").ok_or(AppError::ProjectDirsError)?;
        Ok(dirs.cache_dir().to_path_buf())
    }
}

/// Resolves the app-managed library root. It is deterministic and app-private —
/// the user never picks it.
///
/// Order of precedence:
/// 1. `FLTS_LIBRARY_DIR` — explicit override (tests, power users).
/// 2. `<FLTS_CONFIG_DIR>/library` — keeps the single-env E2E isolation working.
/// 3. The per-platform app-private data dir + `/library`. On iOS this is under
///    `Library/Application Support`, which is **private** (not visible to the
///    Files app, unlike `Documents/`) and **backed up by default** — so no
///    `isExcludedFromBackup` handling is needed. On Android it resolves via
///    Tauri's path API (internal storage); elsewhere `ProjectDirs.data_dir()`.
///    `app` is required on Android, ignored elsewhere.
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
        let app = app.ok_or_else(|| anyhow::anyhow!("AppHandle required to resolve library root on Android"))?;
        return Ok(app.path().app_data_dir()?.join("library"));
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let dirs = ProjectDirs::from("com", "TS", "FLTS").ok_or(AppError::ProjectDirsError)?;
        Ok(dirs.data_dir().join("library"))
    }
}

/// What a legacy-library migration actually did. Returned so callers can log
/// the right message and tests can assert behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MigrationOutcome {
    /// Source absent, or already at the destination — nothing moved.
    NothingToDo,
    /// Source relocated into the (previously empty) destination.
    Moved,
    /// Destination already had content; kept it, left the source untouched.
    KeptExisting,
}

/// Moves a legacy library at `old` into `new_root`, non-destructively: only when
/// the destination is absent or empty. Pure filesystem logic (no config), so it
/// is unit-testable. Uses `rename` with a cross-filesystem recursive-copy
/// fallback, removing the source only after a fully successful copy.
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

/// Recursively copies `src` into `dst`, used as the cross-filesystem fallback
/// when a plain `rename` of the legacy library fails (e.g. moving from a
/// user-picked external volume into the app data dir).
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
    sync_task: Mutex<Option<Arc<crate::app::sync_daemon::SyncTask>>>,
    /// Stable across re-spawns, like `anki_sync_status`.
    sync_status: Arc<watch::Sender<crate::app::sync_daemon::SyncStatus>>,
    translations_cache: tokio::sync::OnceCell<Arc<TranslationsCache>>,
    stats_cache: tokio::sync::OnceCell<Arc<TranslationSizeCache>>,
    gemini_prompt_cache: tokio::sync::OnceCell<Arc<GeminiPromptCache>>,
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
            sync_task: Mutex::new(None),
            sync_status: Arc::new(
                watch::channel(crate::app::sync_daemon::SyncStatus::default()).0,
            ),
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

    pub fn config_borrow_sync_device_name(&self) -> Option<String> {
        self.config
            .borrow()
            .sync_device_name
            .clone()
            .filter(|s| !s.trim().is_empty())
    }

    /// Persist the `syncEnabled` flag and re-evaluate config (starts/stops the
    /// embedded engine). Used by the `sync_set_enabled` command.
    pub async fn set_sync_enabled(&self, enabled: bool) -> anyhow::Result<()> {
        let mut config = self.config.borrow().clone();
        config.sync_enabled = enabled;
        self.update_config(config).await
    }

    /// Called when the app returns to the foreground. On iOS the system tears
    /// down the engine's sockets (incl. its loopback REST listener) while the
    /// app is suspended, and it doesn't reliably rebind — so if the engine is
    /// now unreachable, restart it. A healthy engine is left alone (the poller
    /// refreshes peers on its own).
    pub async fn wake_sync(&self) {
        if !self.config.borrow().sync_enabled {
            return;
        }
        let healthy = match self.sync_engine().await {
            Some(engine) => engine.client().my_id().await.is_ok(),
            None => false,
        };
        if healthy {
            return;
        }
        info!("Sync engine unreachable after wake; restarting");
        let config = self.config.borrow().clone();
        match resolve_library_root(Some(&self.app)) {
            Ok(root) => self.eval_sync(&config, &root).await,
            Err(err) => warn!("wake_sync: cannot resolve library root: {err}"),
        }
    }

    /// Persist the device's display name and apply it live to the running engine
    /// (no restart). Used by the `sync_set_device_name` command.
    pub async fn set_sync_device_name(&self, name: String) -> anyhow::Result<()> {
        let trimmed = name.trim().to_string();
        let mut config = self.config.borrow().clone();
        config.sync_device_name = Some(trimmed.clone()).filter(|s| !s.is_empty());
        config.save(&self.config_path)?;
        self.config.send_replace(config);

        if !trimmed.is_empty() {
            if let Some(engine) = self.sync_engine().await {
                engine.set_device_name(&trimmed).await?;
            }
        }
        Ok(())
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

    pub fn subscribe_sync_status(
        &self,
    ) -> watch::Receiver<crate::app::sync_daemon::SyncStatus> {
        self.sync_status.subscribe()
    }

    pub fn sync_status(&self) -> crate::app::sync_daemon::SyncStatus {
        self.sync_status.borrow().clone()
    }

    /// The running sync engine, if a task is installed (for sync Tauri commands).
    pub async fn sync_engine(
        &self,
    ) -> Option<Arc<library::sync::engine::SyncEngine>> {
        self.sync_task
            .lock()
            .await
            .as_ref()
            .map(|task| task.engine())
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
        // Translator settings (provider/key/model) are captured when the translation queue is created.
        // Reset it so the next translation uses the latest config.
        self.stop_translation_queue().await;

        // The library location is now app-managed (resolve_library_root); the
        // frontend no longer sends a path, so there's nothing to compute here.
        info!("config = {:?}", config);

        config.save(&self.config_path)?;
        self.config.send_replace(config);
        self.eval_config().await?;
        Ok(())
    }

    pub async fn eval_config(&self) -> anyhow::Result<()> {
        let config = self.config.borrow().clone();

        // The library root is now app-managed (no user picker). Resolve it,
        // migrate any legacy user-picked library into it (once), then open it.
        let library_root = resolve_library_root(Some(&self.app))?;
        info!("library_root = {library_root:?}");
        self.migrate_legacy_library(&config, &library_root).await?;

        let library = Arc::new(Library::open(library_root.clone()).await?);
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
            *self.anki_sync_task.lock().await = Some(task);
            info!("Anki sync task spawned (interval = {interval_secs}s)");
        }

        self.watcher
            .lock()
            .await
            .set_path(&library_root)
            .unwrap_or_else(|err| {
                warn!("Failed to set watcher path to {}: {}", library_root.display(), err)
            });

        self.eval_sync(&config, &library_root).await;

        Ok(())
    }

    /// (Re)starts or tears down the native sync task to match config + env.
    /// Opt-in via `syncEnabled`; `FLTS_DISABLE_SYNC` / `FLTS_MOCK_SYNC` force it
    /// off (CI / E2E); `FLTS_SYNC_HERMETIC` keeps it local (tests / Docker).
    /// Never fails `eval_config` — a sync start error is surfaced via status.
    async fn eval_sync(&self, config: &Config, library_root: &Path) {
        use crate::app::sync_daemon::{SyncStatus, SyncTask, sync_disabled};

        // Stop any prior task first (config may have changed).
        if let Some(task) = self.sync_task.lock().await.take() {
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
                self.sync_status.send_replace(SyncStatus::error(err.to_string()));
                return;
            }
        };
        let hermetic = std::env::var_os("FLTS_SYNC_HERMETIC").is_some_and(|v| !v.is_empty());
        // Device display name: the user's choice, else a sensible default
        // (hostname on desktop; a platform label where the hostname is useless,
        // e.g. iOS returns "localhost").
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
                *self.sync_task.lock().await = Some(task);
                info!("Sync task spawned");
            }
            Err(err) => {
                warn!("Sync engine failed to start: {err}");
                self.sync_status.send_replace(SyncStatus::error(err.to_string()));
            }
        }
    }

    /// One-time, idempotent migration of a legacy user-picked library (the old
    /// `config.library_path`, including the old mobile `Documents/FLTSLibrary`
    /// default) into the app-managed root. Non-destructive: never clobbers a
    /// populated destination. Clears the legacy pointer when done so subsequent
    /// runs are no-ops.
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

    /// Drops the legacy `library_path` from the persisted config (it is now
    /// migration-read-only). No-op if already cleared.
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

    pub async fn save_all(&self) {
        if let Some(library) = self.library.borrow().clone() {
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
        // Best effort only: do not let app exit hang forever on any shutdown step.
        run_exit_step(
            "translation queue shutdown",
            EXIT_STOP_QUEUE_TIMEOUT,
            self.stop_translation_queue(),
        )
        .await;
        // Pull task out of the slot under lock; release the lock before awaiting
        // so we never block on a long-running tick from inside the mutex.
        // No final sync_pass here: the task already syncs on every card-store
        // change and the next launch syncs immediately, while a flush against
        // a slow/unreachable AnkiConnect made app exit hang for many seconds.
        let anki_task = self.anki_sync_task.lock().await.take();
        if let Some(task) = anki_task {
            run_exit_step("anki sync shutdown", EXIT_STOP_QUEUE_TIMEOUT, task.shutdown()).await;
        }
        let sync_task = self.sync_task.lock().await.take();
        if let Some(task) = sync_task {
            run_exit_step("sync engine shutdown", EXIT_STOP_QUEUE_TIMEOUT, task.shutdown()).await;
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

    pub async fn translate_chapter(
        &self,
        book_id: Uuid,
        chapter_id: usize,
        model: TranslationModel,
        use_cache: bool,
    ) -> anyhow::Result<usize> {
        let library = self
            .library
            .borrow()
            .clone()
            .ok_or(AppError::NoLibraryError)?;

        let target_language_id = { self.config.borrow().target_language_id.clone() };
        let target_language = Language::from_639_3(&target_language_id)
            .ok_or_else(|| anyhow::anyhow!("invalid target language: {target_language_id}"))?;

        // Collect untranslated paragraph ids under the book lock, then drop it
        // before enqueueing — queue.translate re-acquires the book lock per item.
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

        let queue = self.get_or_init_translation_queue(library).await?;
        for paragraph_id in &untranslated {
            // Dedup-on-enqueue is handled by TranslationQueue::translate.
            // Swallow per-item errors so one bad paragraph doesn't abandon the rest.
            if let Err(err) = queue
                .translate(book_id, *paragraph_id, model, use_cache)
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

    /// A unique scratch directory under the OS temp dir (no tempfile dep).
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

/// Deletes all FLTS-created Gemini server-side context caches (display
/// name prefix "flts-") and clears the local cache pointers. Safe during
/// active translation: affected chapters self-heal via the 403/404
/// evict-and-retry path. Returns the number of deleted caches.
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

/// The app-managed library storage location, for read-only display in settings
/// (the folder picker is gone — see `resolve_library_root`).
#[tauri::command]
pub async fn get_library_root(app: tauri::AppHandle) -> Result<String, String> {
    resolve_library_root(Some(&app))
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|err| err.to_string())
}

/// Opens the library storage location in the OS file manager (desktop).
#[tauri::command]
pub async fn reveal_library_root(app: tauri::AppHandle) -> Result<(), String> {
    let path = resolve_library_root(Some(&app)).map_err(|err| err.to_string())?;
    let _ = fs::create_dir_all(&path);
    reveal_in_file_manager(&path).map_err(|err| err.to_string())
}

fn reveal_in_file_manager(path: &Path) -> anyhow::Result<()> {
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
    model: TranslationModel,
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
    model: TranslationModel,
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
