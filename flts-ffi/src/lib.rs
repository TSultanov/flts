//! The FLTS core for native shells, as typed methods, records and events.

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use app_core::app::{self as core, config, library_view as lv, lyrics, spotify, sync};
use app_core::host::State;
use app_core::{AppHandle, AppState, Host};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

mod types;
pub use types::*;

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("{message}")]
    Failed { message: String },
}

impl CoreError {
    fn new(message: impl Into<String>) -> Self {
        CoreError::Failed {
            message: message.into(),
        }
    }
}

/// Implemented by the native app.
#[uniffi::export(with_foreign)]
pub trait Shell: Send + Sync {
    fn on_event(&self, event: CoreEvent);
    /// Must call `task.run()` on the UI thread.
    fn run_on_main(&self, task: Arc<MainTask>);
}

/// Work the core needs done on the UI thread (AppKit/UIKit-only APIs).
#[derive(uniffi::Object)]
pub struct MainTask(Mutex<Option<Box<dyn FnOnce() + Send>>>);

#[uniffi::export]
impl MainTask {
    pub fn run(&self) {
        let task = self.0.lock().ok().and_then(|mut slot| slot.take());
        if let Some(task) = task {
            task();
        }
    }
}

struct ShellHost {
    shell: Arc<dyn Shell>,
    dirs: Option<app_core::PlatformDirs>,
}

impl Host for ShellHost {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        match CoreEvent::from_wire(event, payload) {
            Ok(event) => self.shell.on_event(event),
            Err(err) => log::warn!("event {event} not delivered: {err}"),
        }
    }

    fn run_on_main_thread(&self, f: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        self.shell
            .run_on_main(Arc::new(MainTask(Mutex::new(Some(f)))));
        Ok(())
    }

    fn platform_dirs(&self) -> Option<app_core::PlatformDirs> {
        self.dirs.clone()
    }
}

/// A parsed EPUB, kept in Rust until it is imported.
#[derive(uniffi::Object)]
pub struct EpubPreview(library::epub_importer::EpubBook);

#[uniffi::export]
impl EpubPreview {
    pub fn title(&self) -> String {
        self.0.title.clone()
    }

    /// As declared by the book (often ISO 639-1).
    pub fn language(&self) -> Option<String> {
        self.0.language.clone()
    }

    /// A chapter's paragraphs as plain text, for previewing before import.
    pub fn chapter_paragraphs(&self, index: Number) -> Vec<String> {
        self.0
            .chapters
            .get(index.index())
            .map(|c| c.paragraphs.iter().map(|p| p.text.clone()).collect())
            .unwrap_or_default()
    }

    pub fn chapters(&self) -> Vec<EpubChapterSummary> {
        self.0
            .chapters
            .iter()
            .map(|c| EpubChapterSummary {
                title: c.title.clone(),
                paragraph_count: c.paragraphs.len().into(),
            })
            .collect()
    }
}

/// A core value as its record: both share serde names.
fn transcode<T: Serialize, U: DeserializeOwned>(value: T) -> Result<U, CoreError> {
    serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map_err(|e| CoreError::new(format!("unexpected value from the core: {e}")))
}

fn ids(v: Vec<Number>) -> Vec<usize> {
    v.into_iter().map(Number::index).collect()
}

#[derive(uniffi::Object)]
pub struct FltsCore {
    state: AppState,
    owner: Mutex<Option<library::task::TaskOwner>>,
}

impl FltsCore {
    fn start_on(host: ShellHost) -> Result<Arc<Self>, CoreError> {
        let fd_limit = app_core::raise_open_file_limit();
        init_logging();
        log::info!("{fd_limit}");
        let core = app_core::start(AppHandle::new(host))
            .map_err(|err| CoreError::new(format!("{err:#}")))?;
        Ok(Arc::new(Self {
            state: core.state,
            owner: Mutex::new(Some(core.owner)),
        }))
    }

    /// Runs a core command on the core runtime, whatever executor polls us.
    async fn run<T, F, Fut>(&self, f: F) -> Result<T, CoreError>
    where
        F: FnOnce(AppState) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, String>> + Send + 'static,
        T: Send + 'static,
    {
        let state = self.state.clone();
        app_core::host::async_runtime::spawn(async move { f(state).await })
            .await
            .map_err(|e| CoreError::new(format!("command task failed: {e}")))?
            .map_err(CoreError::new)
    }

    async fn get<T, U, F, Fut>(&self, f: F) -> Result<U, CoreError>
    where
        F: FnOnce(AppState) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, String>> + Send + 'static,
        T: Serialize + Send + 'static,
        U: DeserializeOwned,
    {
        transcode(self.run(f).await?)
    }
}

#[uniffi::export]
impl FltsCore {
    /// Paths come from the same resolution as the Tauri app (`FLTS_CONFIG_DIR`
    /// and friends included), so both shells open the same library.
    #[uniffi::constructor]
    pub fn start(shell: Arc<dyn Shell>) -> Result<Arc<Self>, CoreError> {
        Self::start_on(ShellHost { shell, dirs: None })
    }

    /// For platforms without HOME (Android): the app's own directories.
    #[uniffi::constructor]
    pub fn start_with_dirs(
        shell: Arc<dyn Shell>,
        config_dir: String,
        cache_dir: String,
        data_dir: String,
    ) -> Result<Arc<Self>, CoreError> {
        let dirs = app_core::PlatformDirs {
            config: PathBuf::from(config_dir),
            cache: PathBuf::from(cache_dir),
            data: PathBuf::from(data_dir),
        };
        Self::start_on(ShellHost {
            shell,
            dirs: Some(dirs),
        })
    }

    /// Flushes and stops the backend; call once before the process exits.
    pub async fn shutdown(&self) {
        let state = self.state.clone();
        let owner = self.owner.lock().ok().and_then(|mut o| o.take());
        let _ = app_core::host::async_runtime::spawn(async move {
            state.shutdown().await;
            drop(owner);
        })
        .await;
    }

    // MARK: Config

    pub async fn get_config(&self) -> Result<AppConfig, CoreError> {
        self.get(|s| async move { core::get_config(State(&s)).await })
            .await
    }

    pub async fn update_config(&self, config: AppConfig) -> Result<(), CoreError> {
        let config: config::Config = transcode(config)?;
        self.run(move |s| async move { core::update_config(State(&s), config).await })
            .await
    }

    pub async fn get_models(&self) -> Result<Vec<ModelInfo>, CoreError> {
        self.get(|s| async move { config::get_models(State(&s)).await })
            .await
    }

    pub fn get_languages(&self) -> Result<Vec<LanguageInfo>, CoreError> {
        transcode(config::get_languages())
    }

    pub fn get_translation_providers(&self) -> Result<Vec<ProviderMeta>, CoreError> {
        transcode(config::get_translation_providers())
    }

    /// A declared language code as a known ISO 639-3 id.
    pub fn parse_language_id(&self, code: Option<String>) -> Option<String> {
        config::parse_language_id(code)
    }

    pub async fn purge_gemini_caches(&self) -> Result<Number, CoreError> {
        self.run(|s| async move { core::purge_gemini_caches(State(&s)).await })
            .await
            .map(Number::from)
    }

    pub async fn get_library_root(&self) -> Result<String, CoreError> {
        self.run(|s| async move { core::get_library_root(s.host()).await })
            .await
    }

    // MARK: Library

    pub async fn list_books(&self) -> Result<Vec<BookMeta>, CoreError> {
        self.get(|s| async move { lv::list_books(State(&s)).await })
            .await
    }

    pub async fn list_book_chapters(&self, book_id: Uuid) -> Result<Vec<ChapterMeta>, CoreError> {
        self.get(move |s| async move { lv::list_book_chapters(State(&s), book_id).await })
            .await
    }

    pub async fn get_book_chapter_paragraph_ids(
        &self,
        book_id: Uuid,
        chapter_id: Number,
    ) -> Result<Vec<Number>, CoreError> {
        let ids = self
            .run(move |s| async move {
                lv::get_book_chapter_paragraph_ids(State(&s), book_id, chapter_id.index()).await
            })
            .await?;
        Ok(ids.into_iter().map(Number::from).collect())
    }

    pub async fn get_paragraph_texts(
        &self,
        book_id: Uuid,
        paragraph_ids: Vec<Number>,
    ) -> Result<ParagraphTexts, CoreError> {
        let ids = ids(paragraph_ids);
        self.run(
            move |s| async move { lv::get_paragraph_texts_batch(State(&s), book_id, ids).await },
        )
        .await
        .map(ParagraphTexts::from)
    }

    pub async fn get_word_info(
        &self,
        book_id: Uuid,
        paragraph_id: Number,
        sentence_id: Number,
        word_id: Number,
    ) -> Result<Option<WordInfo>, CoreError> {
        self.get(move |s| async move {
            lv::get_word_info(
                State(&s),
                book_id,
                paragraph_id.index(),
                sentence_id.index(),
                word_id.index(),
            )
            .await
        })
        .await
    }

    pub async fn create_card_for_word(
        &self,
        book_id: Uuid,
        paragraph_id: Number,
        sentence_id: Number,
        word_id: Number,
    ) -> Result<CardResult, CoreError> {
        self.get(move |s| async move {
            lv::create_card_for_word(
                State(&s),
                book_id,
                paragraph_id.index(),
                sentence_id.index(),
                word_id.index(),
            )
            .await
        })
        .await
    }

    pub async fn get_book_reading_state(
        &self,
        book_id: Uuid,
    ) -> Result<Option<BookReadingState>, CoreError> {
        self.get(move |s| async move { lv::get_book_reading_state(State(&s), book_id).await })
            .await
    }

    pub async fn save_book_reading_state(
        &self,
        book_id: Uuid,
        state: BookReadingState,
    ) -> Result<(), CoreError> {
        self.run(move |s| async move {
            lv::save_book_reading_state(
                State(&s),
                book_id,
                state.chapter_id.index(),
                state.paragraph_id.index(),
                state.page_offset.index(),
            )
            .await
        })
        .await
    }

    pub async fn get_book_summary_status(
        &self,
        book_id: Uuid,
    ) -> Result<BookSummaryStatus, CoreError> {
        self.get(move |s| async move { lv::get_book_summary_status(State(&s), book_id).await })
            .await
    }

    pub async fn move_book(&self, book_id: Uuid, path: Vec<String>) -> Result<(), CoreError> {
        self.run(move |s| async move { lv::move_book(State(&s), book_id, path).await })
            .await
    }

    pub async fn delete_book(&self, book_id: Uuid) -> Result<(), CoreError> {
        self.run(move |s| async move { lv::delete_book(State(&s), book_id).await })
            .await
    }

    pub async fn import_plain_text(
        &self,
        title: String,
        text: String,
        source_language_id: String,
    ) -> Result<Uuid, CoreError> {
        self.run(move |s| async move {
            lv::import_plain_text(State(&s), title, text, source_language_id).await
        })
        .await
    }

    /// Parses an EPUB file; the book stays in Rust until `import_epub`.
    pub async fn parse_epub(&self, path: String) -> Result<Arc<EpubPreview>, CoreError> {
        let book = self
            .run(move |_| async move {
                tokio::task::spawn_blocking(move || {
                    library::epub_importer::EpubBook::load(&PathBuf::from(path))
                })
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| format!("{e:#}"))
            })
            .await?;
        Ok(Arc::new(EpubPreview(book)))
    }

    /// Imports the chosen chapters (by index), in book order.
    pub async fn import_epub(
        &self,
        preview: Arc<EpubPreview>,
        source_language_id: String,
        chapters: Vec<Number>,
    ) -> Result<Uuid, CoreError> {
        let mut book = preview.0.clone();
        let keep: std::collections::HashSet<usize> =
            chapters.into_iter().map(Number::index).collect();
        book.chapters = book
            .chapters
            .into_iter()
            .enumerate()
            .filter(|(i, _)| keep.contains(i))
            .map(|(_, c)| c)
            .collect();
        if book.chapters.is_empty() {
            return Err(CoreError::new("Choose at least one chapter to import."));
        }
        self.run(move |s| async move { lv::import_epub(State(&s), book, source_language_id).await })
            .await
    }

    // MARK: Translation

    pub async fn translate_paragraph(
        &self,
        book_id: Uuid,
        paragraph_id: Number,
        model: String,
        use_cache: bool,
    ) -> Result<(), CoreError> {
        self.run(move |s| async move {
            core::translate_paragraph(State(&s), book_id, paragraph_id.index(), model, use_cache)
                .await
        })
        .await
        .map(|_| ())
    }

    pub async fn translate_chapter(
        &self,
        book_id: Uuid,
        chapter_id: Number,
        model: String,
        use_cache: bool,
    ) -> Result<(), CoreError> {
        self.run(move |s| async move {
            core::translate_chapter(State(&s), book_id, chapter_id.index(), model, use_cache).await
        })
        .await
        .map(|_| ())
    }

    pub async fn list_translation_activity(&self) -> Result<Vec<ActiveTranslation>, CoreError> {
        self.get(|s| async move { core::list_paragraph_translation_activity(State(&s)).await })
            .await
    }

    pub async fn get_system_definition(
        &self,
        word: String,
        source_lang: String,
        target_lang: String,
    ) -> Result<Option<SystemDefinition>, CoreError> {
        self.get(move |s| async move {
            core::get_system_definition(s.host(), word, source_lang, target_lang).await
        })
        .await
    }

    // MARK: Anki

    pub async fn get_anki_sync_status(&self) -> Result<AnkiSyncStatus, CoreError> {
        self.get(|s| async move { core::get_anki_sync_status(State(&s)).await })
            .await
    }

    pub async fn sync_anki_now(&self) -> Result<(), CoreError> {
        self.run(|s| async move { core::sync_anki_now(State(&s)).await })
            .await
            .map(|_| ())
    }

    // MARK: Sync

    pub async fn get_sync_status(&self) -> Result<SyncStatus, CoreError> {
        self.get(|s| async move { sync::get_sync_status(State(&s)).await })
            .await
    }

    pub async fn sync_this_device(&self) -> Result<Option<ThisDevice>, CoreError> {
        self.get(|s| async move { sync::sync_get_this_device(State(&s)).await })
            .await
    }

    pub async fn sync_web_ui_url(&self) -> Result<Option<String>, CoreError> {
        self.run(|s| async move { sync::sync_web_ui_url(State(&s)).await })
            .await
    }

    pub async fn sync_wake(&self) -> Result<(), CoreError> {
        self.run(|s| async move { sync::sync_wake(State(&s)).await })
            .await
    }

    pub async fn sync_list_devices(&self) -> Result<Vec<DeviceEntry>, CoreError> {
        self.get(|s| async move { sync::sync_list_devices(State(&s)).await })
            .await
    }

    pub async fn sync_list_pending(&self) -> Result<Vec<PendingDevice>, CoreError> {
        self.get(|s| async move { sync::sync_list_pending(State(&s)).await })
            .await
    }

    pub async fn sync_set_device_name(&self, name: String) -> Result<(), CoreError> {
        self.run(move |s| async move { sync::sync_set_device_name(State(&s), name).await })
            .await
    }

    pub async fn sync_set_enabled(&self, enabled: bool) -> Result<(), CoreError> {
        self.run(move |s| async move { sync::sync_set_enabled(State(&s), enabled).await })
            .await
    }

    pub async fn sync_add_device(&self, device_id: String, name: String) -> Result<(), CoreError> {
        self.run(move |s| async move { sync::sync_add_device(State(&s), device_id, name).await })
            .await
    }

    pub async fn sync_remove_device(&self, device_id: String) -> Result<(), CoreError> {
        self.run(move |s| async move { sync::sync_remove_device(State(&s), device_id).await })
            .await
    }

    pub async fn sync_reset_identity(&self) -> Result<(), CoreError> {
        self.run(|s| async move { sync::sync_reset_identity(State(&s)).await })
            .await
    }

    pub async fn sync_keep_identity(&self) -> Result<(), CoreError> {
        self.run(|s| async move { sync::sync_keep_identity(State(&s)).await })
            .await
    }

    // MARK: Lyrics / Spotify

    pub async fn start_spotify_watcher(&self) -> Result<(), CoreError> {
        self.run(|s| async move { lyrics::start_spotify_watcher(s.host(), State(&s)).await })
            .await
    }

    pub async fn stop_spotify_watcher(&self) -> Result<(), CoreError> {
        self.run(|s| async move { lyrics::stop_spotify_watcher(State(&s)).await })
            .await
    }

    pub async fn get_now_playing(&self) -> Result<Option<NowPlaying>, CoreError> {
        self.get(|s| async move { lyrics::get_now_playing(State(&s)).await })
            .await
    }

    pub async fn get_track_lyrics_state(
        &self,
        track_id: String,
        target_lang: String,
        model: String,
    ) -> Result<TrackLyricsState, CoreError> {
        self.get(move |s| async move {
            lyrics::get_track_lyrics_state(State(&s), track_id, target_lang, model).await
        })
        .await
    }

    pub async fn spotify_web_connect(&self, client_id: String) -> Result<(), CoreError> {
        self.run(
            move |s| async move { spotify::web::spotify_web_connect(State(&s), client_id).await },
        )
        .await
        .map(|_| ())
    }

    pub async fn spotify_web_disconnect(&self) -> Result<(), CoreError> {
        self.run(|s| async move { spotify::web::spotify_web_disconnect(State(&s)).await })
            .await
            .map(|_| ())
    }

    pub async fn spotify_web_status(&self) -> Result<SpotifyWebStatus, CoreError> {
        self.get(|s| async move { spotify::web::spotify_web_status(State(&s)).await })
            .await
    }

    pub async fn spotify_web_queue(&self) -> Result<Option<QueueSnapshot>, CoreError> {
        self.get(|s| async move { spotify::web::spotify_web_get_queue(State(&s)).await })
            .await
    }

    pub async fn spotify_cdp_status(&self) -> Result<SpotifyCdpStatus, CoreError> {
        self.get(|_| async move { spotify::cdp::spotify_cdp_status().await })
            .await
    }

    pub async fn spotify_restart_with_devtools(&self) -> Result<SpotifyCdpStatus, CoreError> {
        self.get(|_| async move { spotify::cdp::spotify_restart_with_devtools().await })
            .await
    }

    pub async fn spotify_login_agent_status(&self) -> Result<SpotifyLoginAgentStatus, CoreError> {
        self.get(|_| async move { spotify::cdp::spotify_login_agent_status().await })
            .await
    }

    pub async fn spotify_install_login_agent(&self) -> Result<SpotifyLoginAgentStatus, CoreError> {
        self.get(|_| async move { spotify::cdp::spotify_install_login_agent().await })
            .await
    }

    pub async fn spotify_remove_login_agent(&self) -> Result<SpotifyLoginAgentStatus, CoreError> {
        self.get(|_| async move { spotify::cdp::spotify_remove_login_agent().await })
            .await
    }
}

fn init_logging() {
    #[cfg(target_vendor = "apple")]
    {
        let _ = oslog::OsLogger::new("com.TS.FLTS")
            .level_filter(log::LevelFilter::Info)
            .init();
    }
    #[cfg(target_os = "android")]
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("flts"),
    );
}

#[cfg(target_os = "android")]
mod android {
    use jni::EnvUnowned;
    use jni::errors::ThrowRuntimeExAndDefault;
    use jni::objects::{JClass, JObject};

    /// Core threads stay attached to the JVM for life. A thread the bindings
    /// attach for one callback is detached when that callback returns, even
    /// when it is nested in another on the same thread, and the VM aborts.
    fn attach_runtime_thread() {
        if let Ok(vm) = jni::JavaVM::singleton() {
            let _ = vm.attach_current_thread(|_| Ok::<_, jni::errors::Error>(()));
        }
    }

    /// `com.TS.FLTS.core.Native.init(Context)`: must run before the core
    /// starts, since TLS verification and URL opening call into the JVM.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_TS_FLTS_core_Native_init<'caller>(
        mut unowned_env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        context: JObject<'caller>,
    ) {
        unowned_env
            .with_env(|env| -> Result<(), jni::errors::Error> {
                // Never released: ndk-context keeps the raw pointers.
                let global = env.new_global_ref(&context)?;
                let vm = env.get_java_vm()?;
                unsafe {
                    ndk_context::initialize_android_context(
                        vm.get_raw().cast(),
                        global.as_raw().cast(),
                    );
                }
                std::mem::forget(global);
                app_core::host::async_runtime::on_thread_start(attach_runtime_thread);
                rustls_platform_verifier::android::init_with_env(env, context)
            })
            .resolve::<ThrowRuntimeExAndDefault>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_core_event_has_a_typed_form() {
        for name in app_core::dispatch::FORWARDED_EVENTS {
            let err = CoreEvent::from_wire(name, serde_json::Value::Null);
            assert!(
                !matches!(&err, Err(e) if e.starts_with("unknown event")),
                "no CoreEvent for {name}"
            );
        }
    }

    #[test]
    fn wire_payloads_decode() {
        let id = Uuid::new_v4();
        assert_eq!(
            CoreEvent::from_wire("book_updated", serde_json::json!(id)).unwrap(),
            CoreEvent::BookUpdated { book_id: id }
        );
        let e = CoreEvent::from_wire(
            "paragraph_translation_finished",
            serde_json::json!({"bookId": id, "paragraphId": 3, "requestId": 9, "error": null}),
        )
        .unwrap();
        assert!(
            matches!(e, CoreEvent::TranslationFinished { event } if event.paragraph_id == Number(3))
        );
    }

    /// Every core value the app reads converts into its record; a renamed
    /// field fails here rather than as a silent `try?` in Swift.
    #[test]
    fn core_values_convert_into_their_records() {
        use app_core::app::{
            anki_sync, lyrics as ly, spotify::cdp, spotify::web, sync_daemon,
            translation_queue as tq,
        };
        let id = Uuid::new_v4();

        let _: SyncStatus = transcode(sync_daemon::SyncStatus::default()).unwrap();
        let _: StoreIssues = transcode(sync_daemon::StoreIssues::default()).unwrap();
        let _: AnkiSyncStatus = transcode(anki_sync::AnkiSyncStatus::default()).unwrap();
        let _: ThisDevice = transcode(sync::ThisDevice {
            device_id: "D".into(),
            name: None,
        })
        .unwrap();
        let _: DeviceEntry = transcode(sync::DeviceEntry {
            device_id: "D".into(),
            name: "n".into(),
            connected: true,
        })
        .unwrap();
        let _: PendingDevice = transcode(sync::PendingEntry {
            device_id: "D".into(),
            name: "n".into(),
        })
        .unwrap();
        let _: Vec<LanguageInfo> = transcode(config::get_languages()).unwrap();
        let _: Vec<ProviderMeta> = transcode(config::get_translation_providers()).unwrap();
        let _: ActiveTranslation = transcode(tq::ActiveParagraphTranslation {
            book_id: id,
            paragraph_id: 1,
            activity: tq::ParagraphTranslationActivity {
                request_id: 2,
                progress_chars: 3,
                expected_chars: 4,
            },
        })
        .unwrap();
        let _: BookSummaryStatus = transcode(lv::BookSummaryStatusView {
            total_chapters: 2,
            generated: vec![true, false],
            actively_generating: Some(1),
        })
        .unwrap();

        let track = web::TrackMeta {
            id: "t".into(),
            name: "n".into(),
            artist: "a".into(),
            album: None,
            duration_ms: 1000,
        };
        let _: TrackMeta = transcode(track.clone()).unwrap();
        let _: QueueSnapshot = transcode(web::QueueSnapshot {
            context_type: Some("album".into()),
            currently_playing_id: Some("t".into()),
            upcoming: vec![track],
        })
        .unwrap();
        let _: SpotifyWebStatus = transcode(web::SpotifyWebStatus {
            connected: true,
            premium_required: false,
            last_error: None,
        })
        .unwrap();
        let _: SpotifyCdpStatus = transcode(cdp::SpotifyCdpStatus {
            available: false,
            port: 9222,
            hint: None,
        })
        .unwrap();
        let _: SpotifyLoginAgentStatus = transcode(cdp::SpotifyLoginAgentStatus {
            installed: false,
            path: "/p".into(),
        })
        .unwrap();

        let lyrics = library::lyrics::Lyrics {
            track_id: "t".into(),
            lines: vec![library::lyrics::LyricsLine {
                time_ms: Some(10),
                text: "uno".into(),
            }],
            synced: true,
        };
        let translation = library::lyrics::LyricsTranslation {
            track_id: "t".into(),
            target_lang: isolang::Language::Eng,
            model: "m".into(),
            lines: vec![library::lyrics::LyricsLineTranslation {
                translation: "one".into(),
                glosses: vec![library::lyrics::Gloss {
                    fragment: "u".into(),
                    gloss: "o".into(),
                    note: String::new(),
                }],
            }],
        };
        let state: TrackLyricsState = transcode(ly::TrackLyricsState {
            lyrics: Some(lyrics.clone()),
            translation: Some(translation.clone()),
        })
        .unwrap();
        assert_eq!(state.translation.unwrap().target_lang, "eng");
        let wire = |name: &str, v: serde_json::Value| CoreEvent::from_wire(name, v).unwrap();
        wire(
            "lyrics_resolved",
            serde_json::to_value(ly::LyricsResolved {
                track_id: "t".into(),
                lyrics: Some(lyrics),
            })
            .unwrap(),
        );
        wire(
            "lyrics_translation_done",
            serde_json::to_value(ly::LyricsTranslationDone {
                track_id: "t".into(),
                translation,
            })
            .unwrap(),
        );
        wire(
            "lyrics_translation_progress",
            serde_json::to_value(ly::LyricsTranslationProgress {
                track_id: "t".into(),
                bytes: 5,
            })
            .unwrap(),
        );
        wire(
            "lyrics_translation_error",
            serde_json::to_value(ly::LyricsTranslationError {
                track_id: "t".into(),
                error: "e".into(),
            })
            .unwrap(),
        );
        #[cfg(target_os = "macos")]
        {
            let np = app_core::app::spotify::applescript::NowPlaying::empty(
                app_core::app::spotify::applescript::PlayerState::NotRunning,
            );
            let _: NowPlaying = transcode(&np).unwrap();
            wire("spotify_state", serde_json::to_value(np).unwrap());
        }
    }

    #[test]
    fn config_round_trips_through_the_core_type() {
        let json = serde_json::json!({
            "targetLanguageId": "eng", "translationProvider": "google", "geminiApiKey": "k",
            "model": "models/gemini-2.5-flash", "syncEnabled": false
        });
        let core: config::Config = serde_json::from_value(json).unwrap();
        let ffi: AppConfig = transcode(&core).unwrap();
        assert_eq!(ffi.target_language_id, "eng");
        let back: config::Config = transcode(ffi).unwrap();
        assert_eq!(back.gemini_api_key.as_deref(), Some("k"));
    }
}
