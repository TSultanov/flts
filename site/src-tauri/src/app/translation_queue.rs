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

/// Total attempts (initial + restarts) a paragraph gets before a transient
/// failure is surfaced to the user. Restarts run as the very next item (they
/// take priority over queued fresh requests), so when a failure is
/// instantaneous this cap is the only thing bounding the retry loop.
const MAX_TRANSLATION_ATTEMPTS: u32 = 4;

struct TranslationRequest {
    request_id: usize,
    book_id: Uuid,
    paragraph_id: usize,
    model: TranslationModel,
    use_cache: bool,
    /// 0 on first enqueue; incremented each time the worker requeues this
    /// paragraph after a transient failure.
    attempt: u32,
}

/// Decide whether a failed request should be restarted (re-enqueued) rather
/// than surfaced as a terminal error. Pure so it can be unit-tested without a
/// running queue.
fn should_requeue(err: &anyhow::Error, attempt: u32) -> bool {
    attempt + 1 < MAX_TRANSLATION_ATTEMPTS && is_transient_translation_error(err)
}

#[derive(Debug, PartialEq)]
enum FailureDisposition {
    /// Re-enqueued on the priority retry lane (runs as the very next item);
    /// the activity entry survives with progress reset. Caller should push a
    /// progress-reset event to the UI.
    Requeued { expected_chars: usize },
    /// Out of restarts (or error is permanent / queue is shutting down); the
    /// activity entry is removed. Caller should emit the finished-with-error
    /// event.
    Terminal,
}

/// Everything the worker does with a failed request except the Tauri event
/// emission, so the requeue mechanics are unit-testable with a real state map
/// and channel.
async fn handle_translation_failure(
    state: &Arc<Mutex<TranslationQueueState>>,
    retry_tx: &UnboundedSender<TranslationRequest>,
    request: &TranslationRequest,
    err: &anyhow::Error,
) -> FailureDisposition {
    if should_requeue(err, request.attempt) {
        let next_attempt = request.attempt + 1;
        // Keep the active_translations entry so the same request_id, the UI
        // spinner, and the translate() dedup all survive the restart; just
        // reset the visible progress.
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
        // Channel gone (queue shutting down): fall through to the terminal
        // path so the failure is still reported.
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
    /// Pins the exact LibraryBook instance the translation was written
    /// into until the saver has persisted it. Without the pin, a cache
    /// eviction between write and save (the warm LRU holds only 8 books
    /// and get() does not refresh recency) drops the only copy of the
    /// dirty data while the UI is told the translation succeeded.
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
        // Abort the translate loop first: its exit (and its JoinSet's)
        // drops every tx_save clone, closing the saver channel — which is
        // what lets run_saver's graceful drain (finish pending batches,
        // flush dirty books) actually run instead of being aborted mid-save.
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

/// Upper bound on the graceful saver drain at shutdown: enough for a
/// pending batch save plus its retry sleeps, small enough not to blow the
/// app's exit-step timeouts.
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
            // Restarts travel on their own channel so the select below can give
            // them priority: a transiently-failed paragraph runs as the very
            // next item instead of waiting behind everything already queued.
            // The loop owns a sender (cloned per child task), so this channel
            // can never close while the loop is alive.
            let (tx_retry, mut rx_retry) = unbounded_channel::<TranslationRequest>();
            // Bound how many paragraph translations run concurrently. Acquiring a
            // permit before receiving the next request applies backpressure: once
            // `concurrency` are in flight the loop parks until one finishes.
            let semaphore = Arc::new(Semaphore::new(concurrency));
            tokio::spawn(async move {
                // Child tasks live in this JoinSet so they're aborted together
                // when the parent task is aborted on shutdown (JoinSet aborts all
                // its tasks on drop).
                let mut join_set: JoinSet<()> = JoinSet::new();
                loop {
                    let request = tokio::select! {
                        // Reap finished translations so completed handles don't
                        // accumulate, then prefer retries over fresh requests.
                        // Biased so this priority order is deterministic.
                        biased;
                        Some(_) = join_set.join_next() => continue,
                        Some(request) = rx_retry.recv() => request,
                        maybe_request = rx_translate.recv() => {
                            let Some(request) = maybe_request else { break };
                            request
                        }
                    };

                    // Held for the task's lifetime; released when it ends,
                    // which is what lets a parked `acquire_owned` proceed.
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
                                    // Drop the UI's progress ring back to
                                    // zero so the restart is visible.
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
            // No blanket abort here: wait_for_shutdown aborts the translate
            // loop but lets the saver drain gracefully so completed
            // translations reach disk.
            tasks.wait_for_shutdown().await;
        }

        // Aborting dropped every in-flight and channel-pending request without
        // a finished event. The frontend keeps its per-paragraph activity state
        // purely from events (it never re-polls), so emit a terminal event for
        // each stranded entry or its spinner survives the queue forever.
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

    /// Snapshot of every active translation. The frontend reconciles
    /// against this after the webview was suspended (iOS backgrounds the
    /// WKWebView and events emitted meanwhile are lost) — it is the only
    /// way it can discover activity whose `started` event never arrived.
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
        (
            paragraph.original_text.to_string(),
            Language::from_639_3(&book.book.language).unwrap(),
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
        .get_translation(TranslationContext {
            paragraph_text: &paragraph_text,
            book_id: request.book_id,
            chapter_id,
            use_cache: request.use_cache,
            callback: Some(callback),
        })
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

        // Acquire the translation from the book instance that is current at
        // write time, under the same book lock as the staleness checks
        // above. An Arc captured before the minutes-long LLM call can be
        // detached by then (book reloaded after a sync change, evicted from
        // the cache, or its translations vec rebuilt) — a write into a
        // detached instance is invisible to readers and never saved.
        let translation = book.get_or_create_translation(&target_language).await;
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

/// How many times a book save is attempted (with a delay between tries)
/// before the batch is surfaced to the UI as failed. Transient fs races —
/// e.g. the sync daemon rewriting a file mid-save — usually clear within a
/// beat, and the translations stay live in memory either way.
const MAX_SAVE_ATTEMPTS: u32 = 3;
const SAVE_RETRY_DELAY: Duration = Duration::from_secs(1);

async fn run_saver(
    app: tauri::AppHandle,
    library_tx: Arc<watch::Sender<Option<Arc<Library>>>>,
    state: Arc<Mutex<TranslationQueueState>>,
    mut rx: UnboundedReceiver<SaveNotify>,
) {
    // One actor per book coalesces saves: the first notify saves right away,
    // and notifies arriving while that save runs are batched into the next
    // one — the save's own duration is the debounce window. (The previous
    // 1s-delay design was dead code: its Vacant arm never inserted into the
    // map, so every notify saved immediately.)
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
        // Only fails once the actor exited, which cannot happen while we
        // still hold its sender.
        let _ = tx.send(msg);
    }

    // Queue shutting down: close the actor channels and let them drain
    // (shutdown() aborts the translate loop first, which drops every
    // tx_save clone and closes our rx — so this path really runs).
    drop(actors);
    while join_set.join_next().await.is_some() {}
}

/// Save every distinct pinned book instance referenced by the batch plus
/// any instances left dirty by earlier failed batches. Normally that is
/// exactly one instance; it differs only when the cache swapped the book
/// between writes.
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
    // Instances whose last save failed terminally, kept pinned and retried
    // with the next batch so the dirty data survives cache eviction until
    // it reaches disk.
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

        // Even when the save failed, the translations are still live in
        // memory (save() no longer drops them on error) and served by the
        // read path, so the paragraphs are announced as updated either way;
        // the finished event below carries the save error, if any. The
        // file-watcher TranslationChanged path won't fire `book_updated` for
        // our own writes (reload_translations sees in-memory == disk and
        // returns had_effect=false), so emit it directly here. Chapter-list
        // `Resource`s in the frontend subscribe to it to refresh per-chapter
        // translation %.
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

    // Channel closed (graceful drain): last-chance flush of anything a
    // terminal failure left dirty, so it isn't lost when the pins drop.
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
        // On the final allowed attempt there is no restart left to grant.
        let err = anyhow::anyhow!("OpenAI total stream timeout");
        assert!(!should_requeue(&err, MAX_TRANSLATION_ATTEMPTS - 1));
        // ...but the attempt just before it still requeues.
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
        // The restart really is back on the queue, with the attempt bumped
        // and the same request_id.
        let requeued = rx.try_recv().expect("requeued request on the channel");
        assert_eq!(requeued.attempt, 1);
        assert_eq!(requeued.request_id, 7);
        assert_eq!(requeued.book_id, book_id);
        assert_eq!(requeued.paragraph_id, 3);
        // The activity entry survives (same request_id) with progress reset,
        // so the UI spinner and translate() dedup keep working.
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
