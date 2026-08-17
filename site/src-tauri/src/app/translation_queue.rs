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
    library::{Library, library_book::LibraryBook},
    tla_trace::mutex::TracedMutex,
    translation_stats::TranslationSizeCache,
    translator::{
        ChapterContextProvider, TranslationContext, TranslationModel,
        gemini_cache::GeminiPromptCache, get_translator, is_transient_translation_error,
    },
};
use log::{info, warn};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::{Mutex, Semaphore, watch};
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::app::config::Config;
use tauri::Emitter;

const TRANSLATION_PROGRESS_UPDATE_INTERVAL: Duration = Duration::from_millis(500);

/// Attempts (initial + restarts) before a transient failure reaches the user.
/// Restarts jump the queue, so on instant failures this cap alone bounds the
/// retry loop.
const MAX_TRANSLATION_ATTEMPTS: u32 = 4;

struct TranslationRequest {
    request_id: usize,
    book_id: Uuid,
    paragraph_id: usize,
    model: TranslationModel,
    use_cache: bool,
    /// 0 on first enqueue, bumped per transient-failure requeue.
    attempt: u32,
}

/// Whether a failure earns a restart instead of a terminal error.
fn should_requeue(err: &anyhow::Error, attempt: u32) -> bool {
    attempt + 1 < MAX_TRANSLATION_ATTEMPTS && is_transient_translation_error(err)
}

#[derive(Debug, PartialEq)]
enum FailureDisposition {
    /// On the priority retry lane; the activity entry survives with progress
    /// reset, and the caller pushes a progress-reset event.
    Requeued { expected_chars: usize },
    /// Permanent error, restarts exhausted, or shutting down: the activity entry
    /// is gone and the caller emits the finished-with-error event.
    Terminal,
}

/// The failure path minus the Tauri events, so requeue mechanics stay testable
/// against a real state map and channel.
async fn handle_translation_failure(
    state: &Arc<Mutex<TranslationQueueState>>,
    retry_tx: &UnboundedSender<TranslationRequest>,
    request: &TranslationRequest,
    err: &anyhow::Error,
) -> FailureDisposition {
    if should_requeue(err, request.attempt) {
        let next_attempt = request.attempt + 1;
        // The entry must survive so request_id, the spinner, and translate()'s
        // dedup outlive the restart; only progress resets.
        let expected_chars = {
            let mut s = state.lock().await;
            match s
                .active_translations
                .get_mut(&(request.book_id, request.paragraph_id))
            {
                Some(activity) => {
                    activity.progress_chars = 0;
                    activity.expected_chars
                }
                None => 0,
            }
        };
        let requeued = retry_tx.send(TranslationRequest {
            request_id: request.request_id,
            book_id: request.book_id,
            paragraph_id: request.paragraph_id,
            model: request.model,
            use_cache: request.use_cache,
            attempt: next_attempt,
        });
        if requeued.is_ok() {
            warn!(
                "Transient failure translating {}/{} (attempt {}/{}): {}; requeued",
                request.book_id, request.paragraph_id, next_attempt, MAX_TRANSLATION_ATTEMPTS, err
            );
            return FailureDisposition::Requeued { expected_chars };
        }
        // Channel gone: fall through so the failure is still reported.
    }

    warn!(
        "Failed to translate {}/{}: {}",
        request.book_id, request.paragraph_id, err
    );
    state
        .lock()
        .await
        .active_translations
        .remove(&(request.book_id, request.paragraph_id));
    FailureDisposition::Terminal
}

type BookHandle = Arc<TracedMutex<LibraryBook>>;

#[derive(Clone)]
struct SaveNotify {
    request_id: usize,
    book_id: Uuid,
    paragraph_id: usize,
    /// Pins the exact LibraryBook the translation went into until it is
    /// persisted; otherwise a cache eviction between write and save drops the
    /// only copy of dirty data the UI already reported as saved.
    book: BookHandle,
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
        // Abort the translate loop first: its exit drops every tx_save clone and
        // closes the saver channel, which is what triggers run_saver's graceful
        // drain instead of an abort mid-save.
        self.translate_task.abort();
        wait_for_shutdown_task("translate", self.translate_task).await;

        let mut saver_task = self.saver_task;
        match tokio::time::timeout(SAVER_DRAIN_TIMEOUT, &mut saver_task).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) if err.is_cancelled() => {}
            Ok(Err(err)) => warn!("Translation queue saver task failed during shutdown: {err}"),
            Err(_) => {
                warn!(
                    "Translation queue saver did not drain within {SAVER_DRAIN_TIMEOUT:?}; aborting"
                );
                saver_task.abort();
                wait_for_shutdown_task("saver", saver_task).await;
            }
        }
    }
}

/// Fits a pending batch save plus its retry sleeps, but stays under the app's
/// exit-step timeouts.
const SAVER_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

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
        gemini_prompt_cache: Arc<GeminiPromptCache>,
        context_provider: Arc<dyn ChapterContextProvider>,
        config: &Config,
        app: tauri::AppHandle,
        library_tx: Arc<watch::Sender<Option<Arc<Library>>>>,
    ) -> Option<Arc<Self>> {
        let api_keys = config.api_keys();
        let target_language = Language::from_639_3(&config.target_language_id)?;
        // Clamp so a stray 0 can never deadlock the semaphore.
        let concurrency = config.translation_concurrency.max(1) as usize;

        let (tx_save, rx_save) = unbounded_channel::<SaveNotify>();

        let state = Arc::new(Mutex::new(TranslationQueueState {
            active_translations: HashMap::new(),
        }));

        let saver_task = tokio::spawn(run_saver(
            app.clone(),
            library_tx,
            state.clone(),
            rx_save,
        ));

        let (tx_translate, mut rx_translate) = unbounded_channel::<TranslationRequest>();

        let translate_task = {
            let state = state.clone();
            let app = app.clone();
            // Restarts get their own channel so the select can prioritize them
            // over queued fresh requests. The loop owns a sender, so it never
            // closes while the loop lives.
            let (tx_retry, mut rx_retry) = unbounded_channel::<TranslationRequest>();
            let semaphore = Arc::new(Semaphore::new(concurrency));
            tokio::spawn(async move {
                // Dropping the JoinSet aborts the children with the parent.
                let mut join_set: JoinSet<()> = JoinSet::new();
                loop {
                    let request = tokio::select! {
                        // Reap finished handles, then prefer retries; biased so
                        // that priority is deterministic.
                        biased;
                        Some(_) = join_set.join_next() => continue,
                        Some(request) = rx_retry.recv() => request,
                        maybe_request = rx_translate.recv() => {
                            let Some(request) = maybe_request else { break };
                            request
                        }
                    };

                    // Acquired before the next receive, so an in-flight limit of
                    // `concurrency` parks the loop as backpressure. Held for the
                    // task's lifetime.
                    let permit = semaphore
                        .clone()
                        .acquire_owned()
                        .await
                        .expect("translation semaphore never closed");

                    let library = library.clone();
                    let cache = cache.clone();
                    let context_provider = context_provider.clone();
                    let gemini_prompt_cache = gemini_prompt_cache.clone();
                    let stats_cache = stats_cache.clone();
                    let api_keys = api_keys.clone();
                    let app = app.clone();
                    let state = state.clone();
                    let tx_save = tx_save.clone();
                    let tx_retry = tx_retry.clone();

                    join_set.spawn(async move {
                        let _permit = permit;
                        let outcome = async {
                            let provider = request
                                .model
                                .provider()
                                .ok_or_else(|| anyhow::anyhow!("Unknown model provider"))?;
                            let api_key = api_keys
                                .for_provider(provider)
                                .ok_or_else(|| anyhow::anyhow!("no api key for provider {provider:?}"))?
                                .to_owned();
                            let model = request.model;
                            let make_translator = move |source_language: Language| {
                                get_translator(
                                    cache,
                                    context_provider,
                                    gemini_prompt_cache,
                                    provider,
                                    model,
                                    api_key,
                                    source_language,
                                    target_language,
                                )
                            };
                            handle_request(
                                library,
                                make_translator,
                                stats_cache,
                                target_language,
                                app.clone(),
                                state.clone(),
                                &tx_save,
                                &request,
                            )
                            .await
                        }
                        .await;

                        if let Err(err) = outcome {
                            match handle_translation_failure(&state, &tx_retry, &request, &err)
                                .await
                            {
                                FailureDisposition::Requeued { expected_chars } => {
                                    // Zero the progress ring so the restart shows.
                                    let _ = app.emit(
                                        "paragraph_translation_progress",
                                        ParagraphTranslationProgressEvent {
                                            book_id: request.book_id,
                                            paragraph_id: request.paragraph_id,
                                            request_id: request.request_id,
                                            progress_chars: 0,
                                            expected_chars,
                                        },
                                    );
                                }
                                FailureDisposition::Terminal => {
                                    emit_finished(
                                        &app,
                                        request.book_id,
                                        request.paragraph_id,
                                        request.request_id,
                                        Some(err.to_string()),
                                    );
                                }
                            }
                        }
                    });
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
            info!("TranslationQueue shutdown — stopping background tasks");
            // Not a blanket abort: the saver must drain so completed
            // translations reach disk.
            tasks.wait_for_shutdown().await;
        }

        // The abort strands in-flight and pending requests with no finished
        // event, and the frontend drives activity state purely from events, so
        // every stranded entry needs a terminal one or its spinner never stops.
        let stranded: Vec<_> = {
            let mut state = self.state.lock().await;
            state.active_translations.drain().collect()
        };
        for ((book_id, paragraph_id), activity) in stranded {
            emit_finished(
                &self.app,
                book_id,
                paragraph_id,
                activity.request_id,
                Some("translation cancelled".to_string()),
            );
        }
    }

    pub async fn translate(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
        model: TranslationModel,
        use_cache: bool,
    ) -> anyhow::Result<usize> {
        // One lock across check + insert, or two callers both pass the dedup.
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

        // Announced at enqueue, not pickup, so every clicked paragraph spins
        // immediately. expected_chars stays 0 until handle_request estimates it.
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
            attempt: 0,
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

    /// Snapshot of every active translation. iOS suspends the WKWebView and
    /// loses events meanwhile, so this is the frontend's only way to recover
    /// activity whose `started` event never arrived.
    pub async fn list_active_translations(&self) -> Vec<ActiveParagraphTranslation> {
        self.state
            .lock()
            .await
            .active_translations
            .iter()
            .map(
                |(&(book_id, paragraph_id), &activity)| ActiveParagraphTranslation {
                    book_id,
                    paragraph_id,
                    activity,
                },
            )
            .collect()
    }
}

#[derive(Clone, Copy, serde::Serialize)]
pub struct ActiveParagraphTranslation {
    #[serde(rename = "bookId")]
    pub book_id: Uuid,
    #[serde(rename = "paragraphId")]
    pub paragraph_id: usize,
    #[serde(flatten)]
    pub activity: ParagraphTranslationActivity,
}

async fn handle_request(
    library: Arc<Library>,
    make_translator: impl FnOnce(Language) -> anyhow::Result<Box<dyn library::translator::Translator>>,
    stats_cache: Arc<TranslationSizeCache>,
    target_language: Language,
    app: tauri::AppHandle,
    state: Arc<Mutex<TranslationQueueState>>,
    save_notify: &UnboundedSender<SaveNotify>,
    request: &TranslationRequest,
) -> anyhow::Result<()> {
    let (paragraph_text, source_language, chapter_id) = {
        let book = library.get_book(&request.book_id).await?;
        let book = book.lock().await;
        if request.paragraph_id >= book.book.paragraphs_count() {
            anyhow::bail!(
                "Paragraph {} out of range (book has {} paragraphs)",
                request.paragraph_id,
                book.book.paragraphs_count()
            );
        }
        let paragraph = book.book.paragraph_view(request.paragraph_id);
        let chapter_id = book
            .book
            .chapter_for_paragraph(request.paragraph_id)
            .unwrap_or(0);
        let source_language = Language::from_639_3(&book.book.language).ok_or_else(|| {
            anyhow::anyhow!(
                "book has invalid ISO-639-3 language code: {:?}",
                book.book.language
            )
        })?;
        (
            paragraph.original_text.to_string(),
            source_language,
            chapter_id,
        )
    };

    let retry_note = if request.attempt > 0 {
        format!(
            " (retry {}/{})",
            request.attempt + 1,
            MAX_TRANSLATION_ATTEMPTS
        )
    } else {
        String::new()
    };
    info!(
        "Translating paragraph {}{} with model {:?}: \"{}...\"",
        request.paragraph_id,
        retry_note,
        request.model,
        String::from_iter(paragraph_text.chars().take(40))
    );

    let translator = make_translator(source_language)?;

    let source_len = paragraph_text.len();
    let stats = stats_cache.get(&source_language, &target_language).await;
    let expected_size = stats.estimate(source_len);
    info!(
        "Estimated translation size: {} (source len: {}, ratio: {:.1}, observations: {})",
        expected_size, source_len, stats.ratio, stats.n
    );

    // First refinement of the enqueue-time expected_chars=0.
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

            // Keeps the snapshot fresh for a late-mounting UI.
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
        .get_translation(TranslationContext {
            paragraph_text: &paragraph_text,
            book_id: request.book_id,
            chapter_id,
            use_cache: request.use_cache,
            callback: Some(callback),
        })
        .await?;
    info!("Translated paragraph {}", request.paragraph_id);

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

    // The book may have been reloaded during the call (file watcher, sync), which
    // would make this translation stale.
    let book_handle = library.get_book(&request.book_id).await?;
    {
        let mut book = book_handle.lock().await;
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

        // Must come from the instance current at write time, under the same lock
        // as the staleness checks: an Arc captured before the minutes-long LLM
        // call may be detached, and writes into a detached instance are invisible
        // and never saved.
        let translation = book.get_or_create_translation(&target_language).await?;
        translation.lock().await.add_paragraph_translation(
            request.paragraph_id,
            &p_translation,
            request.model,
        );
    }

    library
        .apply_paragraph_to_cards(
            request.book_id,
            request.paragraph_id,
            &p_translation,
            target_language,
        )
        .await?;

    save_notify.send(SaveNotify {
        request_id: request.request_id,
        book_id: request.book_id,
        paragraph_id: request.paragraph_id,
        book: book_handle,
    })?;

    Ok(())
}

/// Save tries before the batch is reported failed. Transient fs races (the sync
/// daemon rewriting a file mid-save) clear within a beat, and the translations
/// stay live in memory regardless.
const MAX_SAVE_ATTEMPTS: u32 = 3;
const SAVE_RETRY_DELAY: Duration = Duration::from_secs(1);

async fn run_saver(
    app: tauri::AppHandle,
    library_tx: Arc<watch::Sender<Option<Arc<Library>>>>,
    state: Arc<Mutex<TranslationQueueState>>,
    mut rx: UnboundedReceiver<SaveNotify>,
) {
    // One actor per book coalesces saves: the first notify saves at once and
    // notifies arriving during that save batch into the next, so the save's own
    // duration is the debounce window.
    let mut actors: HashMap<Uuid, UnboundedSender<SaveNotify>> = HashMap::new();
    let mut join_set: JoinSet<()> = JoinSet::new();

    while let Some(msg) = rx.recv().await {
        let tx = actors.entry(msg.book_id).or_insert_with(|| {
            let (tx, actor_rx) = unbounded_channel::<SaveNotify>();
            join_set.spawn(book_saver(
                app.clone(),
                library_tx.clone(),
                state.clone(),
                actor_rx,
            ));
            tx
        });
        // Cannot fail: the actor outlives the sender we hold.
        let _ = tx.send(msg);
    }

    // Reached because shutdown() aborts the translate loop first, closing our rx.
    drop(actors);
    while join_set.join_next().await.is_some() {}
}

/// Saves every distinct pinned instance in the batch plus any left dirty by
/// earlier failures. More than one only when the cache swapped the book between
/// writes.
async fn save_pinned(batch: &[SaveNotify], unsaved: &[BookHandle]) -> anyhow::Result<()> {
    let mut distinct: Vec<&BookHandle> = Vec::new();
    for handle in unsaved.iter().chain(batch.iter().map(|m| &m.book)) {
        if !distinct.iter().any(|d| Arc::ptr_eq(d, handle)) {
            distinct.push(handle);
        }
    }
    for handle in distinct {
        let mut book = handle.lock().await;
        book.save().await?;
    }
    Ok(())
}

async fn book_saver(
    app: tauri::AppHandle,
    library_tx: Arc<watch::Sender<Option<Arc<Library>>>>,
    state: Arc<Mutex<TranslationQueueState>>,
    mut rx: UnboundedReceiver<SaveNotify>,
) {
    // Instances whose save failed, pinned and retried with the next batch so the
    // dirty data survives cache eviction until it reaches disk.
    let mut unsaved: Vec<BookHandle> = Vec::new();

    while let Some(first) = rx.recv().await {
        let book_id = first.book_id;
        let mut batch = vec![first];
        while let Ok(more) = rx.try_recv() {
            batch.push(more);
        }

        let mut result = save_pinned(&batch, &unsaved).await;
        let mut attempts = 1;
        while result.is_err() && attempts < MAX_SAVE_ATTEMPTS {
            tokio::time::sleep(SAVE_RETRY_DELAY).await;
            // Coalesce anything that finished while we were failing.
            while let Ok(more) = rx.try_recv() {
                batch.push(more);
            }
            result = save_pinned(&batch, &unsaved).await;
            attempts += 1;
        }

        let save_error = result.err().map(|err| {
            warn!("Failed to autosave book {book_id} after {attempts} attempts: {err}");
            format!("translated, but saving the book failed: {err}")
        });

        if save_error.is_none() {
            unsaved.clear();
        } else {
            for msg in &batch {
                if !unsaved.iter().any(|d| Arc::ptr_eq(d, &msg.book)) {
                    unsaved.push(msg.book.clone());
                }
            }
        }

        // Announced even on save failure: the translations stay live in memory
        // and the finished event below carries the error. The file watcher stays
        // silent for our own writes (in-memory == disk), so `book_updated` must
        // be raised here for the frontend's chapter-list resources.
        for msg in &batch {
            let _ = app.emit(
                "paragraph_updated",
                ParagraphUpdatedEvent {
                    book_id: msg.book_id,
                    paragraph_id: msg.paragraph_id,
                },
            );
        }
        let _ = app.emit("book_updated", book_id);
        library_tx.send_modify(|_| {});

        {
            let mut s = state.lock().await;
            for msg in &batch {
                s.active_translations
                    .remove(&(msg.book_id, msg.paragraph_id));
            }
        }
        for msg in &batch {
            emit_finished(
                &app,
                msg.book_id,
                msg.paragraph_id,
                msg.request_id,
                save_error.clone(),
            );
        }
    }

    // Graceful drain: flush what failures left dirty before the pins drop.
    for handle in &unsaved {
        let mut book = handle.lock().await;
        if let Err(err) = book.save().await {
            warn!("Final flush of dirty book failed during saver drain: {err}");
        }
    }
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


async fn wait_for_shutdown_task(task_name: &str, task: tokio::task::JoinHandle<()>) {
    match task.await {
        Ok(()) => {}
        Err(err) if err.is_cancelled() => {}
        Err(err) => warn!("Translation queue {task_name} task failed during shutdown: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requeues_transient_failure_on_first_attempt() {
        let err = anyhow::anyhow!("OpenAI request timed out");
        assert!(should_requeue(&err, 0));
    }

    #[test]
    fn stops_requeueing_once_attempts_exhausted() {
        let err = anyhow::anyhow!("OpenAI total stream timeout");
        assert!(!should_requeue(&err, MAX_TRANSLATION_ATTEMPTS - 1));
        assert!(should_requeue(&err, MAX_TRANSLATION_ATTEMPTS - 2));
    }

    #[test]
    fn never_requeues_non_transient_failure() {
        // The stale-paragraph guard in handle_request is permanent.
        let err = anyhow::anyhow!(
            "Paragraph 3 content changed during translation — discarding stale translation"
        );
        assert!(!should_requeue(&err, 0));
    }

    fn state_with_entry(
        book_id: Uuid,
        paragraph_id: usize,
        request_id: usize,
    ) -> Arc<Mutex<TranslationQueueState>> {
        let mut active_translations = HashMap::new();
        active_translations.insert(
            (book_id, paragraph_id),
            ParagraphTranslationActivity {
                request_id,
                progress_chars: 1234,
                expected_chars: 5000,
            },
        );
        Arc::new(Mutex::new(TranslationQueueState {
            active_translations,
        }))
    }

    fn request(book_id: Uuid, paragraph_id: usize, attempt: u32) -> TranslationRequest {
        TranslationRequest {
            request_id: 7,
            book_id,
            paragraph_id,
            model: TranslationModel::Gemini25Flash,
            use_cache: true,
            attempt,
        }
    }

    #[tokio::test]
    async fn transient_failure_re_adds_request_to_queue() {
        let book_id = Uuid::new_v4();
        let state = state_with_entry(book_id, 3, 7);
        let (tx, mut rx) = unbounded_channel::<TranslationRequest>();
        let err = anyhow::anyhow!("Gemini request timed out");

        let disposition =
            handle_translation_failure(&state, &tx, &request(book_id, 3, 0), &err).await;

        assert_eq!(
            disposition,
            FailureDisposition::Requeued {
                expected_chars: 5000
            }
        );
        let requeued = rx.try_recv().expect("requeued request on the channel");
        assert_eq!(requeued.attempt, 1);
        assert_eq!(requeued.request_id, 7);
        assert_eq!(requeued.book_id, book_id);
        assert_eq!(requeued.paragraph_id, 3);
        let s = state.lock().await;
        let activity = s.active_translations.get(&(book_id, 3)).unwrap();
        assert_eq!(activity.request_id, 7);
        assert_eq!(activity.progress_chars, 0);
        assert_eq!(activity.expected_chars, 5000);
    }

    #[tokio::test]
    async fn exhausted_attempts_fail_terminally() {
        let book_id = Uuid::new_v4();
        let state = state_with_entry(book_id, 3, 7);
        let (tx, mut rx) = unbounded_channel::<TranslationRequest>();
        let err = anyhow::anyhow!("Gemini request timed out");

        let disposition = handle_translation_failure(
            &state,
            &tx,
            &request(book_id, 3, MAX_TRANSLATION_ATTEMPTS - 1),
            &err,
        )
        .await;

        assert_eq!(disposition, FailureDisposition::Terminal);
        assert!(rx.try_recv().is_err(), "nothing should be requeued");
        assert!(
            state.lock().await.active_translations.is_empty(),
            "entry must be removed so the paragraph can be re-translated"
        );
    }

    #[tokio::test]
    async fn non_transient_failure_fails_terminally() {
        let book_id = Uuid::new_v4();
        let state = state_with_entry(book_id, 3, 7);
        let (tx, mut rx) = unbounded_channel::<TranslationRequest>();
        let err = anyhow::anyhow!(
            "Paragraph 3 content changed during translation — discarding stale translation"
        );

        let disposition =
            handle_translation_failure(&state, &tx, &request(book_id, 3, 0), &err).await;

        assert_eq!(disposition, FailureDisposition::Terminal);
        assert!(rx.try_recv().is_err(), "nothing should be requeued");
        assert!(state.lock().await.active_translations.is_empty());
    }

    #[tokio::test]
    async fn closed_queue_degrades_to_terminal_failure() {
        let book_id = Uuid::new_v4();
        let state = state_with_entry(book_id, 3, 7);
        let (tx, rx) = unbounded_channel::<TranslationRequest>();
        drop(rx); // queue shutting down
        let err = anyhow::anyhow!("Gemini request timed out");

        let disposition =
            handle_translation_failure(&state, &tx, &request(book_id, 3, 0), &err).await;

        assert_eq!(disposition, FailureDisposition::Terminal);
        assert!(state.lock().await.active_translations.is_empty());
    }
}
