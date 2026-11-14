use std::{
    error::Error,
    fmt::Display,
    fs,
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(not(target_os = "android"))]
use directories::ProjectDirs;
use library::{
    cache::{GEMINI_PROMPT_CACHE_CAPACITY, TranslationsCache},
    library::{
        Library, PeerChanges,
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
use tokio::sync::watch;
use uuid::Uuid;

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
mod lifecycle;
pub mod lyrics;
pub mod spotify;
pub mod summary_generation_queue;
pub mod sync;
pub mod sync_daemon;
mod task;
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
    let raw = crate::host::hostname();
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
fn resolve_config_dir(app: Option<&crate::host::AppHandle>) -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("FLTS_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    #[cfg(target_os = "android")]
    {
        let app = app.ok_or_else(|| {
            anyhow::anyhow!("AppHandle required to resolve config dir on Android")
        })?;
        return Ok(app.platform_dirs()?.config);
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
fn resolve_cache_dir(app: Option<&crate::host::AppHandle>) -> anyhow::Result<PathBuf> {
    if let Some(dir) = cache_dir_override(std::env::var("FLTS_CONFIG_DIR").ok()) {
        return Ok(dir);
    }
    #[cfg(target_os = "android")]
    {
        let app = app
            .ok_or_else(|| anyhow::anyhow!("AppHandle required to resolve cache dir on Android"))?;
        return Ok(app.platform_dirs()?.cache);
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
fn resolve_library_root(app: Option<&crate::host::AppHandle>) -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("FLTS_LIBRARY_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    if let Some(cfg) = std::env::var_os("FLTS_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(cfg).join("library"));
    }
    #[cfg(target_os = "android")]
    {
        let app = app.ok_or_else(|| {
            anyhow::anyhow!("AppHandle required to resolve library root on Android")
        })?;
        return Ok(app.platform_dirs()?.data.join("library"));
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

/// True when `root` has at most what opening an empty library or starting sync
/// leaves behind: a store without shards and device-local/sync markers.
fn holds_no_library_data(root: &Path) -> bool {
    fn has_files(dir: &Path) -> bool {
        fs::read_dir(dir).map_or(true, |entries| {
            entries.flatten().any(|entry| {
                let path = entry.path();
                if path.is_dir() {
                    has_files(&path)
                } else {
                    true
                }
            })
        })
    }
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries.flatten().all(|entry| {
        let name = entry.file_name();
        match name.to_str() {
            Some(library::store::SHARED_DIR) => !has_files(&entry.path()),
            // The roster names devices, not books.
            Some(".flts" | ".flts-local" | ".flts-lock" | ".stfolder" | ".stignore") => true,
            _ => false,
        }
    })
}

/// Moves a library into `new_root` only when the destination holds no
/// library data; `Library::open` then imports any old-format directories.
fn migrate_library_files(old: &Path, new_root: &Path) -> anyhow::Result<MigrationOutcome> {
    if old == new_root || !old.exists() {
        return Ok(MigrationOutcome::NothingToDo);
    }

    if new_root.exists() && !holds_no_library_data(new_root) {
        return Ok(MigrationOutcome::KeptExisting);
    }

    if let Some(parent) = new_root.parent() {
        fs::create_dir_all(parent)?;
    }
    move_dir(old, new_root)?;
    // The device-local state goes along, or this device would return as a
    // new writer and leave its old one for the others to wait on.
    if let (Some(from), Some(to)) = (
        library::store::sibling_local_dir(old),
        library::store::sibling_local_dir(new_root),
    ) && from.is_dir()
        && let Err(err) = fs::remove_dir_all(&to)
            .or_else(|err| match err.kind() {
                std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(err),
            })
            .map_err(anyhow::Error::from)
            .and_then(|()| move_dir(&from, &to))
    {
        warn!("Could not move the library's local state: {err:#}");
    }
    Ok(MigrationOutcome::Moved)
}

/// The source is removed only after a fully successful copy.
fn move_dir(from: &Path, to: &Path) -> anyhow::Result<()> {
    if fs::rename(from, to).is_err() {
        copy_dir_recursive(from, to)?;
        fs::remove_dir_all(from)?;
    }
    Ok(())
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

fn spawn_catalog_prefetch(
    catalog: ModelCatalog,
    config: &Config,
) -> crate::host::async_runtime::JoinHandle<()> {
    let keys = config.api_keys();
    crate::host::async_runtime::spawn(async move {
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
    })
}

fn api_key_changed(old: Option<&str>, new: Option<&str>) -> bool {
    old.unwrap_or("") != new.unwrap_or("")
}

async fn invalidate_changed_api_keys(
    catalog: &ModelCatalog,
    old: &Config,
    new: &Config,
) -> anyhow::Result<()> {
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
            catalog.invalidate(provider).await?;
        }
    }
    Ok(())
}

const MAX_PARAGRAPH_EVENTS: usize = 64;

#[derive(Clone, Copy, serde::Serialize)]
struct ImportProgress {
    done: usize,
    total: usize,
}

/// The one place `PeerChanges` become frontend events; the watcher and the
/// sync poll both end here.
fn emit_peer_changes(
    app: &crate::host::AppHandle,
    gated: &gated_state::GatedState,
    changes: &PeerChanges,
) {
    if changes.library {
        // The library watch relays this as "library_updated".
        gated.notify_library_changed();
    }
    for uuid in &changes.books {
        info!("Emitting \"book_updated\" for {uuid}");
        if let Err(err) = app.emit("book_updated", uuid) {
            warn!("Failed to emit book_updated: {err}");
        }
    }
    for uuid in &changes.deleted {
        info!("Emitting \"book_deleted\" for {uuid}");
        if let Err(err) = app.emit("book_deleted", uuid) {
            warn!("Failed to emit book_deleted: {err}");
        }
    }
    // Past this many, one refetch of what is on screen is cheaper.
    let whole = changes.paragraphs.len() > MAX_PARAGRAPH_EVENTS;
    if !whole {
        for (book_id, paragraph_id) in &changes.paragraphs {
            let event = translation_queue::ParagraphUpdatedEvent {
                book_id: *book_id,
                paragraph_id: *paragraph_id,
            };
            if let Err(err) = app.emit("paragraph_updated", event) {
                warn!("Failed to emit paragraph_updated: {err}");
            }
        }
    }
    // The frontend refetches every cached paragraph on this one.
    if changes.cards || whole || !changes.books.is_empty() {
        info!("Emitting \"cards_updated\"");
        if let Err(err) = app.emit("cards_updated", ()) {
            warn!("Failed to emit cards_updated: {err}");
        }
    }
}

/// Watcher-independent adoption of peers' writes, for platforms whose watcher
/// polls or misses events.
async fn adopt_peer_changes(
    app: crate::host::AppHandle,
    gated: gated_state::GatedState,
    retired: Vec<u64>,
) -> sync_daemon::StoreIssues {
    let Some(library) = gated.library_unchecked() else {
        return sync_daemon::StoreIssues::default();
    };
    library.set_retired_writers(retired);
    match library.refresh_from_peers().await {
        Ok(changes) => emit_peer_changes(&app, &gated, &changes),
        Err(err) => warn!("Failed to adopt peer changes: {err:#}"),
    }
    let health = library.store_health().await.unwrap_or_default();
    sync_daemon::StoreIssues {
        newer_version: health.incompatible.len(),
        unreadable: health.unreadable.len(),
        strangers: health.strangers.len(),
        stalled: health.stalled,
        shared_identity: health.shared_identity,
    }
}

#[derive(Clone)]
pub struct AppState {
    app: crate::host::AppHandle,
    config: watch::Receiver<Config>,
    gated: gated_state::GatedState,
    lifecycle: lifecycle::Lifecycle,
    translation_queue: watch::Receiver<Option<TranslationQueue>>,
    anki_sync_status: watch::Receiver<anki_sync::AnkiSyncStatus>,
    sync_status: watch::Receiver<sync_daemon::SyncStatus>,
    model_catalog: ModelCatalog,
    pub lyrics_state: lyrics::LyricsState,
    pub spotify_web: spotify::web::SpotifyWebState,
}
impl AppState {
    pub fn host(&self) -> crate::host::AppHandle {
        self.app.clone()
    }
    pub fn new(
        app: crate::host::AppHandle,
        watcher: LibraryWatcher,
    ) -> anyhow::Result<(Self, task::ManagedTask)> {
        let runtime = crate::host::async_runtime::handle();
        let _enter = runtime.inner().enter();
        let backend = BackendState::new(app.clone(), watcher)?;
        let config = backend.config.subscribe();
        let gated = backend.gated.clone();
        let translation_queue = backend.translation_queue.subscribe();
        let anki_sync_status = backend.anki_sync_status.subscribe();
        let sync_status = backend.sync_status.subscribe();
        let model_catalog = backend.model_catalog.0.clone();
        let lyrics_state = backend.lyrics_state.clone();
        let spotify_web = backend.spotify_web.clone();
        let (lifecycle, owner) = lifecycle::Lifecycle::new(backend);
        Ok((
            Self {
                app,
                config,
                gated,
                lifecycle,
                translation_queue,
                anki_sync_status,
                sync_status,
                model_catalog,
                lyrics_state,
                spotify_web,
            },
            owner,
        ))
    }
    async fn get_gemini_prompt_cache(&self) -> anyhow::Result<GeminiPromptCache> {
        self.lifecycle.request(lifecycle::Command::Gemini).await
    }
    pub fn publish_ready(&self, outcome: Result<(), String>) {
        self.gated.publish_ready(outcome);
    }
    pub fn subscribe_config(&self) -> watch::Receiver<Config> {
        self.config.clone()
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
    pub async fn set_sync_enabled(&self, enabled: bool) -> anyhow::Result<()> {
        let mut config = self.config.borrow().clone();
        config.sync_enabled = enabled;
        self.update_config(config).await
    }
    /// Gives this installation an identity of its own, when it shares one
    /// with another, as a copy restored from a backup does: a new sync
    /// identity and a new store writer. It then has to be paired again.
    pub async fn reset_sync_identity(&self) -> anyhow::Result<()> {
        let enabled = self.config.borrow().sync_enabled;
        // Nothing may hold the sync home or the roster open while they go.
        self.set_sync_enabled(false).await?;
        let home = resolve_config_dir(Some(&self.app))?.join("syncthing");
        library::sync::engine::forget_identity(&home).await?;
        self.library()
            .await
            .map_err(anyhow::Error::msg)?
            .take_new_identity()
            .await?;
        if enabled {
            self.set_sync_enabled(true).await?;
        }
        Ok(())
    }

    /// The user keeps this installation's identity; the other copy is the
    /// one that takes a new one.
    pub async fn keep_sync_identity(&self) -> anyhow::Result<()> {
        self.library()
            .await
            .map_err(anyhow::Error::msg)?
            .settle_shared_identity()
            .await
    }

    /// Pairing a device with this one's own id means the two are copies.
    pub async fn note_shared_sync_identity(&self) -> anyhow::Result<()> {
        self.library()
            .await
            .map_err(anyhow::Error::msg)?
            .note_shared_identity()
            .await
    }

    pub fn subscribe_library(&self) -> watch::Receiver<Option<Library>> {
        self.gated.subscribe_library()
    }
    pub fn notify_library_changed(&self) {
        self.gated.notify_library_changed();
    }
    pub async fn library(&self) -> Result<Library, String> {
        self.gated.library().await
    }
    pub fn library_sender(&self) -> watch::Sender<Option<Library>> {
        self.gated.library_sender()
    }
    pub fn subscribe_anki_sync_status(
        &self,
    ) -> watch::Receiver<crate::app::anki_sync::AnkiSyncStatus> {
        self.anki_sync_status.clone()
    }
    pub fn anki_sync_status(&self) -> crate::app::anki_sync::AnkiSyncStatus {
        self.anki_sync_status.borrow().clone()
    }
    pub async fn sync_anki_now(&self) -> anyhow::Result<crate::app::anki_sync::SyncReportDto> {
        self.gated.sync_anki_now().await
    }
    pub fn subscribe_sync_status(&self) -> watch::Receiver<crate::app::sync_daemon::SyncStatus> {
        self.sync_status.clone()
    }
    pub fn sync_status(&self) -> crate::app::sync_daemon::SyncStatus {
        self.sync_status.borrow().clone()
    }
    pub async fn sync_engine(&self) -> Result<Option<crate::app::sync_daemon::SyncTask>, String> {
        self.gated.sync_engine().await
    }
    pub async fn handle_file_change_event(&self, event: &LibraryFileChange) -> anyhow::Result<()> {
        let Some(library) = self.gated.library_unchecked() else {
            return Ok(());
        };
        let changes = library.handle_file_change_event(event).await?;
        emit_peer_changes(&self.app, &self.gated, &changes);
        Ok(())
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
        let model = self.config.borrow().resolved_model_id(&model);
        self.get_or_init_translation_queue()
            .await?
            .translate_chapter(book_id, chapter_id, model, use_cache)
            .await
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

struct BackendState {
    app: crate::host::AppHandle,
    config_path: PathBuf,
    config: watch::Sender<Config>,
    /// Everything `eval_config` installs; see `gated_state`.
    gated: crate::app::gated_state::GatedState,
    translation_queue: watch::Sender<Option<TranslationQueue>>,
    summary_generation_queue: watch::Sender<Option<SummaryGenerationQueue>>,
    watcher: LibraryWatcher,
    translation_owner: Option<task::ManagedTask>,
    summary_owner: Option<task::ManagedTask>,
    anki_owner: Option<task::ManagedTask>,
    sync_owner: Option<task::ManagedTask>,
    catalog_prefetch: Option<crate::host::async_runtime::JoinHandle<()>>,
    /// Stable across `eval_config` re-spawns. The transient `AnkiSyncTask`
    /// holds a clone and pushes status into it on every tick.
    anki_sync_status: watch::Sender<crate::app::anki_sync::AnkiSyncStatus>,
    /// Stable across re-spawns, like `anki_sync_status`.
    sync_status: watch::Sender<crate::app::sync_daemon::SyncStatus>,
    translations_cache: Option<(TranslationsCache, library::task::TaskOwner)>,
    stats_cache: Option<(TranslationSizeCache, library::task::TaskOwner)>,
    gemini_prompt_cache: Option<(GeminiPromptCache, library::task::TaskOwner)>,
    model_catalog: (ModelCatalog, library::task::TaskOwner),
    pub lyrics_state: crate::app::lyrics::LyricsState,
    lyrics_owner: task::ManagedTask,
    spotify_owner: task::ManagedTask,
    pub spotify_web: crate::app::spotify::web::SpotifyWebState,
}

impl BackendState {
    pub fn new(app: crate::host::AppHandle, watcher: LibraryWatcher) -> anyhow::Result<Self> {
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

        let model_catalog = {
            let runtime = crate::host::async_runtime::handle();
            let _enter = runtime.inner().enter();
            ModelCatalog::new(
                resolve_cache_dir(Some(&app))?,
                Box::new(ReqwestListTransport::new()),
            )
        };
        let catalog_prefetch = spawn_catalog_prefetch(model_catalog.0.clone(), &config);

        // Unreachable until a tick proves otherwise: the UI hides the sync
        // button in that state, so it stays hidden until we know.
        let initial_anki_status = crate::app::anki_sync::AnkiSyncStatus {
            state: crate::app::anki_sync::AnkiSyncStatusState::Unreachable,
            ..Default::default()
        };

        let (lyrics_state, lyrics_owner) = crate::app::lyrics::LyricsState::new(app.clone())?;
        let config = watch::channel(config).0;
        let (spotify_web, spotify_owner) = crate::app::spotify::web::SpotifyWebState::new(
            app.clone(),
            config.subscribe(),
            lyrics_state.clone(),
        );
        Ok(Self {
            app,
            config_path,
            config,
            gated: crate::app::gated_state::GatedState::new(),
            translation_queue: watch::channel(None).0,
            summary_generation_queue: watch::channel(None).0,
            watcher,
            translation_owner: None,
            summary_owner: None,
            anki_owner: None,
            sync_owner: None,
            catalog_prefetch: Some(catalog_prefetch),
            anki_sync_status: watch::channel(initial_anki_status).0,
            sync_status: watch::channel(crate::app::sync_daemon::SyncStatus::default()).0,
            translations_cache: None,
            stats_cache: None,
            gemini_prompt_cache: None,
            model_catalog,
            lyrics_state,
            lyrics_owner,
            spotify_web,
            spotify_owner,
        })
    }

    pub fn publish_ready(&mut self, outcome: Result<(), String>) {
        self.gated.publish_ready(outcome);
    }

    /// Persists `syncEnabled` and re-evaluates config, starting/stopping the
    /// embedded engine.

    /// Call on foreground. iOS tears down the engine's sockets (incl. its
    /// loopback REST listener) during suspension and doesn't reliably rebind, so
    /// an unreachable engine is restarted; a healthy one is left alone.
    async fn restart_sync_if_needed(&mut self) {
        if !self.config.borrow().sync_enabled {
            return;
        }
        // Gate explicitly: a wake must not race startup into a second engine start.
        if let Err(err) = self.gated.await_ready().await {
            warn!("wake_sync: startup did not complete: {err}");
            return;
        }
        let healthy = match self.sync_engine().await.ok().flatten() {
            Some(engine) => engine.healthy().await,
            None => false,
        };
        if healthy {
            return;
        }
        info!("Sync engine unreachable after wake; restarting");
        // Don't bounce an engine another evaluation just restarted.
        if let Some(engine) = self.sync_engine().await.ok().flatten()
            && engine.healthy().await
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
    async fn rename_sync_device(&mut self, name: String) -> anyhow::Result<()> {
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

    pub fn subscribe_library(&mut self) -> watch::Receiver<Option<Library>> {
        self.gated.subscribe_library()
    }

    pub fn library_sender(&mut self) -> watch::Sender<Option<Library>> {
        self.gated.library_sender()
    }

    /// The running sync engine, if a task is installed.
    pub async fn sync_engine(
        &mut self,
    ) -> Result<Option<crate::app::sync_daemon::SyncTask>, String> {
        self.gated.sync_engine().await
    }

    fn set_anki_sync_unreachable(&mut self, reason: &str) {
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
    /// Called only by the lifecycle owner.
    async fn apply_config(&mut self, config: Config) -> anyhow::Result<()> {
        // The lifecycle owner serializes queue construction with this transition.
        let (config, library_root) = {
            // Queues capture translator/summarizer settings at creation.
            self.stop_translation_queue().await;
            self.stop_summary_generation_queue().await;
            self.lyrics_state.cancel_all().await;
            if let Some(task) = self.gated.take_anki_task() {
                task.shutdown().await;
            }
            if let Some(task) = self.gated.take_sync_task().await {
                task.shutdown().await;
            }

            // What cannot be saved now is retried by the store; one bad
            // commit must not block every settings change.
            if let Some(library) = self.gated.library_unchecked()
                && let Err(err) = library.flush().await
            {
                log::warn!("library flush failed: {err:#}");
            }
            self.stop_catalog_prefetch().await;

            let old = self.config.borrow().clone();
            invalidate_changed_api_keys(&self.model_catalog.0, &old, &config).await?;

            info!("Applying configuration");
            config.save(&self.config_path)?;
            self.config.send_replace(config.clone());
            self.catalog_prefetch = Some(spawn_catalog_prefetch(
                self.model_catalog.0.clone(),
                &config,
            ));
            self.eval_library_config().await?
        };
        self.eval_sync(&config, &library_root).await;
        Ok(())
    }

    async fn restore_configuration(&mut self, previous: Config) {
        self.config.send_replace(previous.clone());
        if let Err(error) = previous.save(&self.config_path) {
            log::warn!("restoring config.json failed: {error:#}");
        }
        self.stop_catalog_prefetch().await;
        self.catalog_prefetch = Some(spawn_catalog_prefetch(
            self.model_catalog.0.clone(),
            &previous,
        ));
        match self.eval_library_config().await {
            Ok((config, root)) => self.eval_sync(&config, &root).await,
            Err(error) => log::warn!("restoring session services failed: {error:#}"),
        }
    }

    async fn initialize_session(&mut self) -> anyhow::Result<()> {
        let (config, library_root) = self.eval_library_config().await?;
        self.eval_sync(&config, &library_root).await;
        Ok(())
    }

    /// Migrates + opens the library, (re)spawns the Anki task, points the
    /// watcher. Called only by the lifecycle owner.
    async fn eval_library_config(&mut self) -> anyhow::Result<(Config, PathBuf)> {
        let config = self.config.borrow().clone();

        let library_root = resolve_library_root(Some(&self.app))?;
        info!("library_root = {library_root:?}");
        self.migrate_legacy_library(&config, &library_root).await?;

        let canonical = tokio::fs::canonicalize(&library_root).await.ok();
        let library = match self.gated.library_unchecked() {
            Some(current) if canonical.as_deref() == Some(current.root()) => current,
            Some(current) => {
                let next = self.open_library(&library_root).await?;
                current.close().await?;
                next
            }
            None => self.open_library(&library_root).await?,
        };
        self.gated.install_library(library.clone());

        let prior = self.gated.take_anki_task();
        if let Some(task) = prior {
            info!("Stopping prior Anki sync task before re-spawn");
            task.shutdown().await;
        }

        if !config.anki_sync_enabled {
            info!("Anki sync disabled (ankiSyncEnabled = false)");
            self.set_anki_sync_unreachable("Anki sync is turned off in settings");
        } else {
            let endpoint = config
                .anki_endpoint
                .clone()
                .unwrap_or_else(|| "http://127.0.0.1:8765".to_owned());
            let api_key = config.anki_api_key.clone();
            let client = library::anki::connect::get_anki_connect(endpoint, api_key);
            let interval_secs = std::env::var("FLTS_ANKI_SYNC_INTERVAL_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(DEFAULT_ANKI_SYNC_INTERVAL_SECS);
            let (task, owner) = AnkiSyncTask::init(
                library.clone(),
                client,
                Duration::from_secs(interval_secs),
                self.anki_sync_status.clone(),
            );
            self.anki_owner = Some(owner);
            self.gated.install_anki_task(task);
            info!("Anki sync task spawned (interval = {interval_secs}s)");
        }

        self.watcher.set_path(&library_root).unwrap_or_else(|err| {
            warn!(
                "Failed to set watcher path to {}: {}",
                library_root.display(),
                err
            )
        });

        Ok((config, library_root))
    }

    /// (Re)starts or tears down the native sync task to match config + env.
    /// Called only by the lifecycle owner.
    /// Opt-in via `syncEnabled`; `FLTS_DISABLE_SYNC` / `FLTS_MOCK_SYNC` force it
    /// off (CI / E2E); `FLTS_SYNC_HERMETIC` keeps it local (tests / Docker).
    /// Never fails `eval_config` — a sync start error is surfaced via status.
    async fn eval_sync(&mut self, config: &Config, library_root: &Path) {
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
            {
                let (app, gated) = (self.app.clone(), self.gated.clone());
                Box::new(move |retired| {
                    Box::pin(adopt_peer_changes(app.clone(), gated.clone(), retired))
                })
            },
        )
        .await
        {
            Ok((task, owner)) => {
                self.sync_owner = Some(owner);
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
    async fn migrate_legacy_library(
        &mut self,
        config: &Config,
        new_root: &Path,
    ) -> anyhow::Result<()> {
        let Some(old) = config
            .library_path
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
        else {
            return Ok(());
        };

        // A move across filesystems copies the whole library.
        let moved = {
            let (old, new_root) = (old.clone(), new_root.to_path_buf());
            tokio::task::spawn_blocking(move || migrate_library_files(&old, &new_root)).await??
        };
        match moved {
            MigrationOutcome::Moved => info!("Migrated library {old:?} -> {new_root:?}"),
            MigrationOutcome::KeptExisting => warn!(
                "Library destination {new_root:?} already has content; keeping it and leaving \
                 the legacy library at {old:?} untouched"
            ),
            MigrationOutcome::NothingToDo => {}
        }

        self.clear_library_path().await
    }

    /// Opens the library, telling the frontend how an import of old-format
    /// files is going and holding commands back until it is done.
    async fn open_library(&self, root: &Path) -> anyhow::Result<Library> {
        let (app, gated) = (self.app.clone(), self.gated.clone());
        let progress: library::legacy::Progress = std::sync::Arc::new(move |done, total| {
            gated.set_importing(done < total);
            let event = ImportProgress { done, total };
            if let Err(err) = app.emit("library_import_progress", event) {
                warn!("Failed to emit library_import_progress: {err}");
            }
        });
        let opened = Library::open_reporting(root.to_path_buf(), progress).await;
        self.gated.set_importing(false);
        opened
    }

    /// Drops `library_path` from the persisted config; no-op if already cleared.
    async fn clear_library_path(&mut self) -> anyhow::Result<()> {
        let mut config = self.config.borrow().clone();
        if config.library_path.is_some() {
            config.library_path = None;
            config.save(&self.config_path)?;
            self.config.send_replace(config);
        }
        Ok(())
    }

    pub async fn stop_translation_queue(&mut self) {
        if let Some(queue) = self.translation_queue.send_replace(None) {
            info!("Stopping translation queue");
            queue.shutdown().await;
            info!("Translation queue stopped");
        }
        if let Some(owner) = self.translation_owner.as_ref() {
            let _ = owner.shutdown().await;
        }
        self.translation_owner.take();
    }

    pub async fn stop_summary_generation_queue(&mut self) {
        if let Some(queue) = self.summary_generation_queue.send_replace(None) {
            info!("Stopping summary generation queue");
            queue.shutdown().await;
            info!("Summary generation queue stopped");
        }
        if let Some(owner) = self.summary_owner.as_ref() {
            let _ = owner.shutdown().await;
        }
        self.summary_owner.take();
    }

    async fn get_translations_cache(&mut self) -> anyhow::Result<TranslationsCache> {
        if self.translations_cache.is_none() {
            let cache_dir = resolve_cache_dir(Some(&self.app))?;
            self.translations_cache = Some(TranslationsCache::create(&cache_dir).await?);
        }
        Ok(self.translations_cache.as_ref().unwrap().0.clone())
    }

    async fn get_stats_cache(&mut self) -> anyhow::Result<TranslationSizeCache> {
        if self.stats_cache.is_none() {
            let cache_dir = resolve_cache_dir(Some(&self.app))?;
            self.stats_cache = Some(TranslationSizeCache::create(&cache_dir).await?);
        }
        Ok(self.stats_cache.as_ref().unwrap().0.clone())
    }

    async fn get_gemini_prompt_cache(&mut self) -> anyhow::Result<GeminiPromptCache> {
        if self.gemini_prompt_cache.is_none() {
            let cache_dir = resolve_cache_dir(Some(&self.app))?;
            self.gemini_prompt_cache = Some(
                GeminiPromptCache::open(
                    &cache_dir.join("gemini_caches"),
                    GEMINI_PROMPT_CACHE_CAPACITY,
                )
                .await?,
            );
        }
        Ok(self.gemini_prompt_cache.as_ref().unwrap().0.clone())
    }

    async fn stop_catalog_prefetch(&mut self) {
        let prior = self.catalog_prefetch.take();
        if let Some(prior) = prior {
            prior.abort();
            let _ = prior.await;
        }
    }

    async fn perform_shutdown(&mut self) {
        // This work is owned by the lifecycle task even if its caller times out.
        self.stop_catalog_prefetch().await;
        self.stop_summary_generation_queue().await;
        self.lyrics_state.shutdown().await;
        self.spotify_web.stop_polling().await;
        #[cfg(target_os = "macos")]
        self.lyrics_state.watcher.stop().await;
        run_exit_step(
            "translation queue shutdown",
            EXIT_STOP_QUEUE_TIMEOUT,
            self.stop_translation_queue(),
        )
        .await;
        self.stop_translation_queue().await;
        // The controller retains every owner until its accepted work has drained.
        let anki_task = self.gated.take_anki_task();
        if let Some(task) = anki_task {
            run_exit_step(
                "anki sync shutdown",
                EXIT_STOP_QUEUE_TIMEOUT,
                task.shutdown(),
            )
            .await;
        }
        if let Some(owner) = self.anki_owner.as_ref() {
            let _ = owner.shutdown().await;
        }
        self.anki_owner.take();
        let sync_task = self.gated.take_sync_task().await;
        if let Some(task) = sync_task {
            run_exit_step(
                "sync engine shutdown",
                EXIT_STOP_QUEUE_TIMEOUT,
                task.shutdown(),
            )
            .await;
            // A slow Go stop stays owned; do not release the library lease.
            task.shutdown().await;
        }
        if let Some(owner) = self.sync_owner.as_ref() {
            let _ = owner.shutdown().await;
        }
        self.sync_owner.take();
        if let Some(library) = self.gated.library_unchecked() {
            if let Err(err) = library.close().await {
                log::warn!("library shutdown retained dirty state: {err:#}");
            }
        }
        let _ = self.spotify_owner.shutdown().await;
        let _ = self.lyrics_owner.shutdown().await;
        self.model_catalog.0.close().await;
        self.close_caches_for_exit().await;
    }

    async fn build_translation_queue(&mut self) -> anyhow::Result<TranslationQueue> {
        if let Some(queue) = self.translation_queue.borrow().clone() {
            return Ok(queue);
        }

        // Public queue requests pass the readiness gate before reaching this controller.
        self.gated
            .await_ready()
            .await
            .map_err(|err| anyhow::anyhow!(err))?;

        // Another caller may have populated the queue while we waited.
        if let Some(queue) = self.translation_queue.borrow().clone() {
            return Ok(queue);
        }

        // Service construction and library replacement are serialized by the controller.
        let library = self
            .gated
            .library_unchecked()
            .ok_or_else(|| anyhow::anyhow!("library is not configured"))?;

        let config = self.config.borrow().clone();
        let cache = self.get_translations_cache().await?;
        let stats_cache = self.get_stats_cache().await?;
        let gemini_prompt_cache = self.get_gemini_prompt_cache().await?;
        let summary_queue = self.build_summary_queue().await?;
        let context_provider: Box<dyn library::translator::ChapterContextProvider> =
            Box::new(SummaryBackedChapterContext {
                queue: summary_queue,
                library_rx: self.subscribe_library(),
            });
        let (queue, owner) = TranslationQueue::init(
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

        self.translation_owner = Some(owner);
        self.translation_queue.send_replace(Some(queue.clone()));
        Ok(queue)
    }

    async fn build_summary_queue(&mut self) -> anyhow::Result<SummaryGenerationQueue> {
        if let Some(queue) = self.summary_generation_queue.borrow().clone() {
            return Ok(queue);
        }

        if let Some(queue) = self.summary_generation_queue.borrow().clone() {
            return Ok(queue);
        }

        let config = self.config.borrow().clone();
        let (queue, owner) =
            SummaryGenerationQueue::init(self.subscribe_library(), &config, self.app.clone());

        self.summary_owner = Some(owner);
        self.summary_generation_queue
            .send_replace(Some(queue.clone()));
        Ok(queue)
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

impl BackendState {
    async fn close_caches_for_exit(&mut self) {
        if let Some((cache, owner)) = self.translations_cache.as_ref() {
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
            let _ = owner.shutdown().await;
        }
        if let Some((cache, owner)) = self.stats_cache.as_ref() {
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
            let _ = owner.shutdown().await;
        }
        if let Some((cache, owner)) = self.gemini_prompt_cache.as_ref() {
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
            let _ = owner.shutdown().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{future::pending, sync::atomic::AtomicBool, time::Instant};

    use super::*;

    #[tokio::test]
    async fn exit_step_completes_when_future_finishes() {
        let completed = std::sync::Arc::new(AtomicBool::new(false));
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
    fn migration_takes_the_device_local_state_along() {
        let base = scratch_dir("local");
        let (old, new) = (base.join("old"), base.join("new"));
        let local = |root: &Path| library::store::sibling_local_dir(root).unwrap();
        fs::create_dir_all(old.join("db").join("book")).unwrap();
        fs::write(old.join("db").join("book").join("shard"), b"rows").unwrap();
        fs::create_dir_all(local(&old)).unwrap();
        fs::write(local(&old).join("writer-id"), b"00000000000000aa").unwrap();
        // What sync leaves in a destination it was started on first.
        fs::create_dir_all(new.join(".flts").join("roster")).unwrap();
        fs::write(new.join(".flts").join("roster").join("shard"), b"roster").unwrap();
        fs::create_dir_all(local(&new)).unwrap();
        fs::write(local(&new).join("writer-id"), b"00000000000000bb").unwrap();

        let outcome = migrate_library_files(&old, &new).unwrap();

        assert_eq!(outcome, MigrationOutcome::Moved);
        assert!(!old.exists() && !local(&old).exists());
        assert_eq!(
            fs::read(local(&new).join("writer-id")).unwrap(),
            b"00000000000000aa",
            "the device stays the writer of the shards it brought"
        );
        assert_eq!(library::store::local_dir(&new), local(&new));
        assert!(new.join(".flts").join("roster").join("shard").is_file());
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
    fn migration_moves_into_a_destination_holding_only_an_empty_store() {
        let base = scratch_dir("scaffold");
        let old = base.join("old");
        let new = base.join("new");
        let book = "1b4e28ba-2fa1-11d2-883f-0016d3cca427";
        fs::create_dir_all(old.join(book)).unwrap();
        fs::write(old.join(book).join("book.dat"), b"legacy").unwrap();
        fs::create_dir_all(new.join("db").join("book")).unwrap();
        fs::create_dir_all(new.join(".flts-local")).unwrap();
        fs::write(new.join(".stignore"), b"/.flts-local\n").unwrap();

        let outcome = migrate_library_files(&old, &new).unwrap();

        assert_eq!(outcome, MigrationOutcome::Moved);
        assert!(!old.exists(), "source removed after move");
        assert_eq!(
            fs::read(new.join(book).join("book.dat")).unwrap(),
            b"legacy"
        );
        assert!(new.join("db").join("book").is_dir(), "store left in place");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn migration_keeps_a_destination_whose_store_has_shards() {
        let base = scratch_dir("shards");
        let old = base.join("old");
        let new = base.join("new");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("book.dat"), b"legacy").unwrap();
        let table = new.join("db").join("book");
        fs::create_dir_all(&table).unwrap();
        fs::write(table.join("00000000000000aa.v1.dbs"), b"shard").unwrap();

        let outcome = migrate_library_files(&old, &new).unwrap();

        assert_eq!(outcome, MigrationOutcome::KeptExisting);
        assert!(
            old.join("book.dat").exists(),
            "legacy library left untouched"
        );
        assert!(!new.join("book.dat").exists());
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

pub async fn get_anki_sync_status(
    state: crate::host::State<'_, AppState>,
) -> Result<crate::app::anki_sync::AnkiSyncStatus, String> {
    Ok(state.anki_sync_status())
}

pub async fn sync_anki_now(
    state: crate::host::State<'_, AppState>,
) -> Result<crate::app::anki_sync::SyncReportDto, String> {
    state.sync_anki_now().await.map_err(|err| err.to_string())
}

pub async fn update_config(
    state: crate::host::State<'_, AppState>,
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

pub async fn get_config(state: crate::host::State<'_, AppState>) -> Result<Config, String> {
    Ok(state.config.borrow().clone())
}

/// Deletes every FLTS-created Gemini server-side context cache (display name
/// prefix "flts-") and clears the local pointers; returns the count. Safe during
/// active translation: chapters self-heal via the 403/404 evict-and-retry path.
pub async fn purge_gemini_caches(state: crate::host::State<'_, AppState>) -> Result<usize, String> {
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
pub async fn get_library_root(app: crate::host::AppHandle) -> Result<String, String> {
    resolve_library_root(Some(&app))
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|err| err.to_string())
}

/// Opens the library location in the desktop OS file manager.
pub async fn reveal_library_root(app: crate::host::AppHandle) -> Result<(), String> {
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

pub async fn translate_paragraph(
    state: crate::host::State<'_, AppState>,
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

pub async fn translate_chapter(
    state: crate::host::State<'_, AppState>,
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

pub async fn get_paragraph_translation_activity(
    state: crate::host::State<'_, AppState>,
    book_id: Uuid,
    paragraph_id: usize,
) -> Result<Option<translation_queue::ParagraphTranslationActivity>, String> {
    state
        .get_paragraph_translation_activity(book_id, paragraph_id)
        .await
        .map_err(|err| err.to_string())
}

pub async fn list_paragraph_translation_activity(
    state: crate::host::State<'_, AppState>,
) -> Result<Vec<translation_queue::ActiveParagraphTranslation>, String> {
    state
        .list_paragraph_translation_activity()
        .await
        .map_err(|err| err.to_string())
}

pub async fn get_system_definition(
    #[allow(unused_variables)] app: crate::host::AppHandle,
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

pub async fn show_system_dictionary(
    #[allow(unused_variables)] app: crate::host::AppHandle,
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

#[cfg(all(test, feature = "manual-llm-diagnostics"))]
mod manual_llm_diagnostics;
