use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use isolang::Language;
use library::{
    cache::TranslationsCache,
    library::Library,
    translation_stats::TranslationSizeCache,
    translator::{TranslationModel, TranslationProvider, get_translator},
};
use log::{info, warn};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::{Mutex, watch};
use uuid::Uuid;

use crate::app::config::Config;
use tauri::Emitter;

const TRANSLATION_PROGRESS_UPDATE_INTERVAL: Duration = Duration::from_millis(500);

struct TranslationRequest {
    request_id: usize,
    book_id: Uuid,
    paragraph_id: usize,
    model: TranslationModel,
    use_cache: bool,
}

#[derive(Clone, Copy)]
struct SaveNotify {
    request_id: usize,
    book_id: Uuid,
    paragraph_id: usize,
}

#[derive(Clone, serde::Serialize)]
struct ParagraphUpdatedEvent {
    #[serde(rename = "bookId")]
    book_id: Uuid,
    #[serde(rename = "paragraphId")]
    paragraph_id: usize,
}

#[derive(Clone, serde::Serialize)]
struct ParagraphTranslationStartedEvent {
    #[serde(rename = "bookId")]
    book_id: Uuid,
    #[serde(rename = "paragraphId")]
    paragraph_id: usize,
    #[serde(rename = "requestId")]
    request_id: usize,
    #[serde(rename = "expectedChars")]
    expected_chars: usize,
}

#[derive(Clone, serde::Serialize)]
struct ParagraphTranslationProgressEvent {
    #[serde(rename = "bookId")]
    book_id: Uuid,
    #[serde(rename = "paragraphId")]
    paragraph_id: usize,
    #[serde(rename = "requestId")]
    request_id: usize,
    #[serde(rename = "progressChars")]
    progress_chars: usize,
    #[serde(rename = "expectedChars")]
    expected_chars: usize,
}

#[derive(Clone, serde::Serialize)]
struct ParagraphTranslationFinishedEvent {
    #[serde(rename = "bookId")]
    book_id: Uuid,
    #[serde(rename = "paragraphId")]
    paragraph_id: usize,
    #[serde(rename = "requestId")]
    request_id: usize,
    error: Option<String>,
}

#[derive(Clone, Copy, serde::Serialize)]
pub struct ParagraphTranslationActivity {
    #[serde(rename = "requestId")]
    pub request_id: usize,
    #[serde(rename = "progressChars")]
    pub progress_chars: usize,
    #[serde(rename = "expectedChars")]
    pub expected_chars: usize,
}

struct TranslationQueueState {
    active_translations: HashMap<(Uuid, usize), ParagraphTranslationActivity>,
}

struct TranslationQueueTasks {
    translate_task: tokio::task::JoinHandle<()>,
    saver_task: tokio::task::JoinHandle<()>,
}

impl TranslationQueueTasks {
    fn abort(&self) {
        self.translate_task.abort();
        self.saver_task.abort();
    }

    async fn wait_for_shutdown(self) {
        wait_for_shutdown_task("translate", self.translate_task).await;
        wait_for_shutdown_task("saver", self.saver_task).await;
    }
}

pub struct TranslationQueue {
    next_request_index: AtomicUsize,
    translate_tx: UnboundedSender<TranslationRequest>,

    state: Arc<Mutex<TranslationQueueState>>,
    app: tauri::AppHandle,

    tasks: Mutex<Option<TranslationQueueTasks>>,
}

impl Drop for TranslationQueue {
    fn drop(&mut self) {
        if let Ok(mut tasks) = self.tasks.try_lock()
            && let Some(tasks) = tasks.take()
        {
            info!("TranslationQueue dropped — aborting background tasks");
            tasks.abort();
        }
    }
}

impl TranslationQueue {
    pub fn init(
        library: Arc<Library>,
        cache: Arc<TranslationsCache>,
        stats_cache: Arc<TranslationSizeCache>,
        config: &Config,
        app: tauri::AppHandle,
        library_tx: Arc<watch::Sender<Option<Arc<Library>>>>,
    ) -> Option<Arc<Self>> {
        let gemini_api_key = config.gemini_api_key.clone();
        let openai_api_key = config.openai_api_key.clone();
        let target_language = Language::from_639_3(&config.target_language_id)?;

        let (tx_save, rx_save) = unbounded_channel::<SaveNotify>();

        let state = Arc::new(Mutex::new(TranslationQueueState {
            active_translations: HashMap::new(),
        }));

        let saver_task = tokio::spawn(run_saver(
            library.clone(),
            app.clone(),
            library_tx,
            state.clone(),
            rx_save,
        ));

        let (tx_translate, mut rx_translate) = unbounded_channel::<TranslationRequest>();

        let translate_task = {
            let state = state.clone();
            let app = app.clone();
            tokio::spawn(async move {
                while let Some(request) = rx_translate.recv().await {
                    let library = library.clone();
                    let cache = cache.clone();
                    let gemini_api_key = gemini_api_key.clone();
                    let openai_api_key = openai_api_key.clone();
                    let app = app.clone();

                    let outcome = handle_request(
                        library,
                        cache,
                        stats_cache.clone(),
                        target_language,
                        gemini_api_key,
                        openai_api_key,
                        app.clone(),
                        state.clone(),
                        &tx_save,
                        &request,
                    )
                    .await;

                    if let Err(err) = outcome {
                        warn!(
                            "Failed to translate {}/{}: {}",
                            request.book_id, request.paragraph_id, err
                        );
                        state
                            .lock()
                            .await
                            .active_translations
                            .remove(&(request.book_id, request.paragraph_id));
                        emit_finished(
                            &app,
                            request.book_id,
                            request.paragraph_id,
                            request.request_id,
                            Some(err.to_string()),
                        );
                    }
                }
            })
        };

        Some(Arc::new(Self {
            next_request_index: 0.into(),
            translate_tx: tx_translate,
            state,
            app,
            tasks: Mutex::new(Some(TranslationQueueTasks {
                translate_task,
                saver_task,
            })),
        }))
    }

    pub async fn shutdown(&self) {
        let tasks = self.tasks.lock().await.take();
        if let Some(tasks) = tasks {
            info!("TranslationQueue shutdown — aborting background tasks");
            tasks.abort();
            tasks.wait_for_shutdown().await;
        }
    }

    pub async fn translate(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
        model: TranslationModel,
        use_cache: bool,
    ) -> anyhow::Result<usize> {
        // Hold lock across check + insert to prevent TOCTOU race where two
        // concurrent calls both pass the dedup check and send duplicate requests.
        let mut state = self.state.lock().await;
        if let Some(activity) = state.active_translations.get(&(book_id, paragraph_id)) {
            return Ok(activity.request_id);
        }

        let request_id = self.next_request_index.fetch_add(1, Ordering::SeqCst);
        state.active_translations.insert(
            (book_id, paragraph_id),
            ParagraphTranslationActivity {
                request_id,
                progress_chars: 0,
                expected_chars: 0,
            },
        );
        drop(state);

        // Announce activity at enqueue, not when the worker picks the request
        // up. Otherwise queued paragraphs sit silently until earlier ones finish
        // — the frontend would show a spinner only on the in-flight item, not
        // on the ones the user clicked while one was still running. expected_chars
        // is unknown until the worker estimates; it is updated via the progress
        // event emitted at the start of handle_request.
        let _ = self.app.emit(
            "paragraph_translation_started",
            ParagraphTranslationStartedEvent {
                book_id,
                paragraph_id,
                request_id,
                expected_chars: 0,
            },
        );

        if let Err(err) = self.translate_tx.send(TranslationRequest {
            request_id,
            book_id,
            paragraph_id,
            model,
            use_cache,
        }) {
            self.state
                .lock()
                .await
                .active_translations
                .remove(&(book_id, paragraph_id));
            return Err(err.into());
        }

        Ok(request_id)
    }

    pub async fn get_active_translation(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
    ) -> Option<ParagraphTranslationActivity> {
        self.state
            .lock()
            .await
            .active_translations
            .get(&(book_id, paragraph_id))
            .copied()
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_request(
    library: Arc<Library>,
    cache: Arc<TranslationsCache>,
    stats_cache: Arc<TranslationSizeCache>,
    target_language: Language,
    gemini_api_key: Option<String>,
    openai_api_key: Option<String>,
    app: tauri::AppHandle,
    state: Arc<Mutex<TranslationQueueState>>,
    save_notify: &UnboundedSender<SaveNotify>,
    request: &TranslationRequest,
) -> anyhow::Result<()> {
    let (translation, paragraph_text, source_language) = {
        let book = library.get_book(&request.book_id).await?;
        let mut book = book.lock().await;
        let translation = book.get_or_create_translation(&target_language).await;
        let paragraph = book.book.paragraph_view(request.paragraph_id);
        (
            translation,
            paragraph.original_text.to_string(),
            Language::from_639_3(&book.book.language).unwrap(),
        )
    };

    info!(
        "Translating paragraph {} with model {:?}: \"{}...\"",
        request.paragraph_id,
        request.model,
        String::from_iter(paragraph_text.chars().take(40))
    );

    let provider = request
        .model
        .provider()
        .ok_or(anyhow::anyhow!("Unknown model provider"))?;

    let api_key = match provider {
        TranslationProvider::Google => {
            gemini_api_key.ok_or(anyhow::anyhow!("No Gemini API key"))?
        }
        TranslationProvider::Openai => {
            openai_api_key.ok_or(anyhow::anyhow!("No OpenAI API key"))?
        }
    };

    let translator = get_translator(
        cache,
        provider,
        request.model,
        api_key,
        source_language,
        target_language,
    )?;

    let source_len = paragraph_text.len();
    let stats = stats_cache.get(&source_language, &target_language).await;
    let expected_size = stats.estimate(source_len);
    info!(
        "Estimated translation size: {} (source len: {}, ratio: {:.1}, observations: {})",
        expected_size, source_len, stats.ratio, stats.n
    );

    // Record expected size in the activity snapshot and push it to the UI as
    // a progress event with progress_chars=0. The started event already fired
    // at enqueue with expected_chars=0; this is the first refinement once the
    // worker actually picks up the request.
    {
        let mut s = state.lock().await;
        if let Some(activity) = s
            .active_translations
            .get_mut(&(request.book_id, request.paragraph_id))
        {
            activity.expected_chars = expected_size;
        }
    }
    let _ = app.emit(
        "paragraph_translation_progress",
        ParagraphTranslationProgressEvent {
            book_id: request.book_id,
            paragraph_id: request.paragraph_id,
            request_id: request.request_id,
            progress_chars: 0,
            expected_chars: expected_size,
        },
    );

    let callback = {
        let app = app.clone();
        let state = state.clone();
        let request_id = request.request_id;
        let book_id = request.book_id;
        let paragraph_id = request.paragraph_id;
        struct EmitState {
            last_emit: Instant,
            last_progress: usize,
        }
        let emit_state = Arc::new(std::sync::Mutex::new(EmitState {
            last_emit: Instant::now(),
            last_progress: 0,
        }));
        Box::new(move |progress_len: usize| {
            let mut s = emit_state.lock().unwrap();
            if s.last_progress == progress_len {
                return;
            }
            if s.last_emit.elapsed() < TRANSLATION_PROGRESS_UPDATE_INTERVAL {
                return;
            }

            s.last_emit = Instant::now();
            s.last_progress = progress_len;
            drop(s);

            // Update the in-memory snapshot so a late-mounting UI fetching
            // the current activity sees fresh progress.
            let state = state.clone();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                {
                    let mut s = state.lock().await;
                    if let Some(activity) = s.active_translations.get_mut(&(book_id, paragraph_id))
                    {
                        activity.progress_chars = progress_len;
                        activity.expected_chars = expected_size;
                    }
                }
                let _ = app.emit(
                    "paragraph_translation_progress",
                    ParagraphTranslationProgressEvent {
                        book_id,
                        paragraph_id,
                        request_id,
                        progress_chars: progress_len,
                        expected_chars: expected_size,
                    },
                );
            });
        })
    };

    let p_translation = translator
        .get_translation(&paragraph_text, request.use_cache, Some(callback))
        .await?;
    info!("Translated paragraph {}", request.paragraph_id);

    // Measure actual translation JSON size and update stats
    let actual_size = serde_json::to_string(&p_translation)
        .map(|s| s.len())
        .unwrap_or(0);
    stats_cache
        .record_observation(&source_language, &target_language, source_len, actual_size)
        .await;
    info!(
        "Recorded translation stats: source_len={}, actual_size={}, ratio={:.1}",
        source_len,
        actual_size,
        actual_size as f64 / source_len as f64
    );

    // F4 fix: Re-read paragraph text and verify it hasn't changed since we started translating.
    // Between our initial read and now, the book could have been reloaded (e.g., file watcher
    // picked up a sync update), which would make this translation stale.
    {
        let book_handle = library.get_book(&request.book_id).await?;
        let book = book_handle.lock().await;
        if request.paragraph_id >= book.book.paragraphs_count() {
            return Err(anyhow::anyhow!(
                "Paragraph {} no longer exists (book now has {} paragraphs) — discarding stale translation",
                request.paragraph_id,
                book.book.paragraphs_count()
            ));
        }
        let current_text = book
            .book
            .paragraph_view(request.paragraph_id)
            .original_text
            .to_string();
        if current_text != paragraph_text {
            return Err(anyhow::anyhow!(
                "Paragraph {} content changed during translation — discarding stale translation",
                request.paragraph_id
            ));
        }
    }

    translation
        .lock()
        .await
        .add_paragraph_translation(request.paragraph_id, &p_translation, request.model);

    save_notify.send(SaveNotify {
        request_id: request.request_id,
        book_id: request.book_id,
        paragraph_id: request.paragraph_id,
    })?;

    Ok(())
}

async fn run_saver(
    library: Arc<Library>,
    app: tauri::AppHandle,
    library_tx: Arc<watch::Sender<Option<Arc<Library>>>>,
    state: Arc<Mutex<TranslationQueueState>>,
    mut rx: UnboundedReceiver<SaveNotify>,
) {
    let savers = Arc::new(Mutex::new(HashMap::new()));

    while let Some(msg) = rx.recv().await {
        let book_id = msg.book_id;

        // Use entry API to avoid double lock
        let mut savers_guard = savers.lock().await;
        match savers_guard.entry(book_id) {
            std::collections::hash_map::Entry::Vacant(_) => {
                // No existing saver, save immediately
                drop(savers_guard); // Drop lock before await
                save_and_emit(library.clone(), app.clone(), &library_tx, msg)
                    .await
                    .unwrap_or_else(|err| warn!("Failed to autosave book {book_id}: {err}"));
                finalize_request(&state, &app, msg).await;
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                // Existing saver, spawn delayed save
                let saver = {
                    let library = library.clone();
                    let app = app.clone();
                    let library_tx = library_tx.clone();
                    let savers = savers.clone();
                    let state = state.clone();
                    async move {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        save_and_emit(library.clone(), app.clone(), &library_tx, msg)
                            .await
                            .unwrap_or_else(|err| {
                                warn!("Failed to autosave book {book_id}: {err}")
                            });
                        savers.lock().await.remove(&book_id);
                        finalize_request(&state, &app, msg).await;
                    }
                };

                let task = tokio::spawn(saver);
                savers_guard.insert(book_id, task);
            }
        }
    }
}

async fn finalize_request(
    state: &Arc<Mutex<TranslationQueueState>>,
    app: &tauri::AppHandle,
    msg: SaveNotify,
) {
    state
        .lock()
        .await
        .active_translations
        .remove(&(msg.book_id, msg.paragraph_id));
    emit_finished(app, msg.book_id, msg.paragraph_id, msg.request_id, None);
}

fn emit_finished(
    app: &tauri::AppHandle,
    book_id: Uuid,
    paragraph_id: usize,
    request_id: usize,
    error: Option<String>,
) {
    let _ = app.emit(
        "paragraph_translation_finished",
        ParagraphTranslationFinishedEvent {
            book_id,
            paragraph_id,
            request_id,
            error,
        },
    );
}

async fn save_book(library: Arc<Library>, book_id: Uuid) -> anyhow::Result<()> {
    let book_handle = library.get_book(&book_id).await?;
    let mut book = book_handle.lock().await;
    book.save().await
}

async fn save_and_emit(
    library: Arc<Library>,
    app: tauri::AppHandle,
    library_tx: &watch::Sender<Option<Arc<Library>>>,
    msg: SaveNotify,
) -> anyhow::Result<()> {
    save_book(library, msg.book_id).await?;
    info!(
        "Emitting \"paragraph_updated\" for {}/{}",
        msg.book_id, msg.paragraph_id
    );
    app.emit(
        "paragraph_updated",
        ParagraphUpdatedEvent {
            book_id: msg.book_id,
            paragraph_id: msg.paragraph_id,
        },
    )?;
    library_tx.send_modify(|_| {});
    Ok(())
}

async fn wait_for_shutdown_task(task_name: &str, task: tokio::task::JoinHandle<()>) {
    match task.await {
        Ok(()) => {}
        Err(err) if err.is_cancelled() => {}
        Err(err) => warn!("Translation queue {task_name} task failed during shutdown: {err}"),
    }
}
