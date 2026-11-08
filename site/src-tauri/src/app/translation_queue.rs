use crate::app::{
    config::{ApiKeys, Config},
    task::ManagedTask,
};
use isolang::Language;
use library::{
    cache::TranslationsCache,
    library::{BookHandle, BookSnapshot, Library},
    translation_stats::TranslationSizeCache,
    translator::{
        ChapterContextProvider, TranslationContext, gemini_cache::GeminiPromptCache,
        get_translator, is_transient_translation_error,
    },
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tauri::Emitter;
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
};
use uuid::Uuid;
const MAX_TRANSLATION_ATTEMPTS: u32 = 4;
const MAX_PENDING: usize = 1024;
fn should_requeue(error: &anyhow::Error, attempt: u32) -> bool {
    attempt + 1 < MAX_TRANSLATION_ATTEMPTS && is_transient_translation_error(error)
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

#[derive(Clone, Copy, serde::Serialize)]
pub struct ActiveParagraphTranslation {
    #[serde(rename = "bookId")]
    pub book_id: Uuid,
    #[serde(rename = "paragraphId")]
    pub paragraph_id: usize,
    #[serde(flatten)]
    pub activity: ParagraphTranslationActivity,
}

type Activities = HashMap<(Uuid, usize), ParagraphTranslationActivity>;
struct Job {
    request_id: usize,
    book: BookHandle,
    snapshot: Arc<BookSnapshot>,
    paragraph: usize,
    model: String,
    use_cache: bool,
    attempt: u32,
    progress: Arc<AtomicUsize>,
    expected: Arc<AtomicUsize>,
}
struct ChapterJob {
    book: BookHandle,
    snapshot: Arc<BookSnapshot>,
    chapter: usize,
    offset: usize,
    model: String,
    use_cache: bool,
}
enum Request {
    Paragraph {
        book_id: Uuid,
        paragraph: usize,
        model: String,
        use_cache: bool,
        reply: oneshot::Sender<anyhow::Result<usize>>,
    },
    Chapter {
        book_id: Uuid,
        chapter: usize,
        model: String,
        use_cache: bool,
        reply: oneshot::Sender<anyhow::Result<usize>>,
    },
}
#[derive(Clone)]
struct WorkerContext {
    library: Arc<Library>,
    cache: Arc<TranslationsCache>,
    stats: Arc<TranslationSizeCache>,
    gemini: Arc<GeminiPromptCache>,
    context: Arc<dyn ChapterContextProvider>,
    provider: library::translator::TranslationProvider,
    keys: ApiKeys,
    target: Language,
}
pub struct TranslationQueue {
    requests: mpsc::Sender<Request>,
    activity: watch::Receiver<Arc<Activities>>,
    task: ManagedTask,
}
impl TranslationQueue {
    pub fn init(
        library: Arc<Library>,
        cache: Arc<TranslationsCache>,
        stats: Arc<TranslationSizeCache>,
        gemini: Arc<GeminiPromptCache>,
        context: Arc<dyn ChapterContextProvider>,
        config: &Config,
        app: tauri::AppHandle,
        library_tx: Arc<watch::Sender<Option<Arc<Library>>>>,
    ) -> Option<Arc<Self>> {
        let target = Language::from_639_3(&config.target_language_id)?;
        let worker = WorkerContext {
            library,
            cache,
            stats,
            gemini,
            context,
            provider: config.translation_provider,
            keys: config.api_keys(),
            target,
        };
        let concurrency = config.translation_concurrency.max(1) as usize;
        let (requests, receiver) = mpsc::channel(256);
        let (publish, activity) = watch::channel(Arc::new(HashMap::new()));
        let task = ManagedTask::spawn(move |stop| {
            scheduler(
                worker,
                concurrency,
                receiver,
                stop,
                publish,
                app,
                library_tx,
            )
        });
        Some(Arc::new(Self {
            requests,
            activity,
            task,
        }))
    }
    pub async fn translate(
        &self,
        book_id: Uuid,
        paragraph: usize,
        model: String,
        use_cache: bool,
    ) -> anyhow::Result<usize> {
        let (reply, result) = oneshot::channel();
        self.requests
            .send(Request::Paragraph {
                book_id,
                paragraph,
                model,
                use_cache,
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("translation queue closed"))?;
        result.await?
    }
    pub async fn translate_chapter(
        &self,
        book_id: Uuid,
        chapter: usize,
        model: String,
        use_cache: bool,
    ) -> anyhow::Result<usize> {
        let (reply, result) = oneshot::channel();
        self.requests
            .send(Request::Chapter {
                book_id,
                chapter,
                model,
                use_cache,
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("translation queue closed"))?;
        result.await?
    }
    pub async fn shutdown(&self) {
        if let Err(err) = self.task.shutdown().await {
            log::warn!("translation shutdown: {err}");
        }
    }
    pub async fn get_active_translation(
        &self,
        book_id: Uuid,
        paragraph: usize,
    ) -> Option<ParagraphTranslationActivity> {
        self.activity.borrow().get(&(book_id, paragraph)).copied()
    }
    pub async fn list_active_translations(&self) -> Vec<ActiveParagraphTranslation> {
        let snapshot = self.activity.borrow().clone();
        snapshot
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
fn admit(
    book: BookHandle,
    snapshot: Arc<BookSnapshot>,
    paragraph: usize,
    model: String,
    use_cache: bool,
    next_id: &mut usize,
    activities: &mut Activities,
    app: &tauri::AppHandle,
) -> Job {
    let request_id = *next_id;
    *next_id += 1;
    activities.insert(
        (snapshot.book.id, paragraph),
        ParagraphTranslationActivity {
            request_id,
            progress_chars: 0,
            expected_chars: 0,
        },
    );
    let _ = app.emit(
        "paragraph_translation_started",
        ParagraphTranslationStartedEvent {
            book_id: snapshot.book.id,
            paragraph_id: paragraph,
            request_id,
            expected_chars: 0,
        },
    );
    Job {
        request_id,
        book,
        snapshot,
        paragraph,
        model,
        use_cache,
        attempt: 0,
        progress: Arc::new(AtomicUsize::new(0)),
        expected: Arc::new(AtomicUsize::new(0)),
    }
}
async fn scheduler(
    context: WorkerContext,
    concurrency: usize,
    mut requests: mpsc::Receiver<Request>,
    mut stop: watch::Receiver<bool>,
    publish: watch::Sender<Arc<Activities>>,
    app: tauri::AppHandle,
    library_tx: Arc<watch::Sender<Option<Arc<Library>>>>,
) {
    let mut activities = HashMap::new();
    let mut next_id = 0;
    let mut pending: VecDeque<Job> = VecDeque::new();
    let mut chapters: VecDeque<ChapterJob> = VecDeque::new();
    let mut workers = JoinSet::new();
    let mut running: HashMap<
        tokio::task::Id,
        (Uuid, usize, usize, Arc<AtomicUsize>, Arc<AtomicUsize>),
    > = HashMap::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(500));
    loop {
        while pending.len() < MAX_PENDING && !chapters.is_empty() {
            let chapter = chapters.front_mut().unwrap();
            let view = chapter.snapshot.book.chapter_view(chapter.chapter);
            if chapter.offset >= view.paragraph_count() {
                chapters.pop_front();
                continue;
            }
            let paragraph = view.paragraph_view(chapter.offset).id;
            chapter.offset += 1;
            if activities.contains_key(&(chapter.snapshot.book.id, paragraph))
                || chapter
                    .snapshot
                    .translation(&context.target)
                    .is_some_and(|t| t.paragraph_view(paragraph).is_some())
            {
                continue;
            }
            pending.push_back(admit(
                chapter.book.clone(),
                chapter.snapshot.clone(),
                paragraph,
                chapter.model.clone(),
                chapter.use_cache,
                &mut next_id,
                &mut activities,
                &app,
            ));
        }
        while workers.len() < concurrency {
            let Some(job) = pending.pop_front() else {
                break;
            };
            let key = (
                job.snapshot.book.id,
                job.paragraph,
                job.request_id,
                job.progress.clone(),
                job.expected.clone(),
            );
            let context = context.clone();
            let task = workers.spawn(async move {
                let result = execute(&job, &context).await;
                (job, result)
            });
            running.insert(task.id(), key);
        }
        publish.send_replace(Arc::new(activities.clone()));
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            Some(result) = workers.join_next_with_id(), if !workers.is_empty() => {
                match result {
                    Ok((task, (mut job, result))) => {
                        running.remove(&task);
                        let id = job.snapshot.book.id;
                        if let Err(error) = &result {
                            if should_requeue(error, job.attempt) {
                                job.attempt += 1; job.progress.store(0, Ordering::Relaxed);
                                if let Some(activity) = activities.get_mut(&(id, job.paragraph)) { activity.progress_chars = 0; }
                                pending.push_front(job); continue;
                            }
                        }
                        activities.remove(&(id, job.paragraph));
                        let _ = app.emit("paragraph_updated", ParagraphUpdatedEvent {book_id: id, paragraph_id: job.paragraph});
                        let _ = app.emit("book_updated", id); library_tx.send_modify(|_| {});
                        let _ = app.emit("paragraph_translation_finished", ParagraphTranslationFinishedEvent {book_id: id, paragraph_id: job.paragraph, request_id: job.request_id, error: result.err().map(|e| e.to_string())});
                    }
                    Err(error) => { if let Some((id, paragraph, request, _, _)) = running.remove(&error.id()) { activities.remove(&(id, paragraph)); let _ = app.emit("paragraph_translation_finished", ParagraphTranslationFinishedEvent {book_id: id, paragraph_id: paragraph, request_id: request, error: Some(error.to_string())}); } }
                }
            }
            request = requests.recv() => {
                let Some(request) = request else { break; };
                match request {
                    Request::Paragraph {book_id, paragraph, model, use_cache, reply} => {
                        if let Some(activity) = activities.get(&(book_id, paragraph)) { let _ = reply.send(Ok(activity.request_id)); continue; }
                        if pending.len() >= MAX_PENDING { let _ = reply.send(Err(anyhow::anyhow!("translation queue busy"))); continue; }
                        let result = async {
                            let book = context.library.get_book(&book_id).await?; let snapshot = book.snapshot();
                            anyhow::ensure!(paragraph < snapshot.book.paragraphs_count(), "paragraph out of range");
                            let job = admit(book, snapshot, paragraph, model, use_cache, &mut next_id, &mut activities, &app);
                            let id = job.request_id; pending.push_back(job); Ok(id)
                        }.await;
                        publish.send_replace(Arc::new(activities.clone()));
                        let _ = reply.send(result);
                    }
                    Request::Chapter {book_id, chapter, model, use_cache, reply} => {
                        let result = async {
                            anyhow::ensure!(chapters.len() < 256, "translation queue busy");
                            let book = context.library.get_book(&book_id).await?; let snapshot = book.snapshot();
                            anyhow::ensure!(chapter < snapshot.book.chapter_count(), "chapter out of range");
                            let count = snapshot.book.chapter_view(chapter).paragraphs().filter(|p| snapshot.translation(&context.target).is_none_or(|t| t.paragraph_view(p.id).is_none())).count();
                            if !chapters.iter().any(|c| c.snapshot.book.id == book_id && c.chapter == chapter) { chapters.push_back(ChapterJob {book, snapshot, chapter, offset: 0, model, use_cache}); }
                            Ok(count)
                        }.await;
                        publish.send_replace(Arc::new(activities.clone()));
                        let _ = reply.send(result);
                    }
                }
            }
            _ = ticker.tick() => {
                for (id, paragraph, request_id, progress, expected) in running.values() {
                    let chars = progress.load(Ordering::Relaxed); let estimate = expected.load(Ordering::Relaxed);
                    if let Some(activity) = activities.get_mut(&(*id, *paragraph)) {
                        if activity.request_id != *request_id || (activity.progress_chars == chars && activity.expected_chars == estimate) { continue; }
                        activity.progress_chars = chars; activity.expected_chars = estimate;
                        let _ = app.emit("paragraph_translation_progress", ParagraphTranslationProgressEvent {book_id: *id, paragraph_id: *paragraph, request_id: *request_id, progress_chars: chars, expected_chars: estimate});
                    }
                }
            }
        }
    }
    requests.close();
    workers.abort_all();
    while workers.join_next().await.is_some() {}
    for ((book_id, paragraph_id), activity) in activities {
        let _ = app.emit(
            "paragraph_translation_finished",
            ParagraphTranslationFinishedEvent {
                book_id,
                paragraph_id,
                request_id: activity.request_id,
                error: Some("translation cancelled".into()),
            },
        );
    }
    publish.send_replace(Arc::new(HashMap::new()));
}
async fn execute(job: &Job, context: &WorkerContext) -> anyhow::Result<()> {
    let source = Language::from_639_3(&job.snapshot.book.language)
        .ok_or_else(|| anyhow::anyhow!("invalid source language"))?;
    let text = job
        .snapshot
        .book
        .paragraph_view(job.paragraph)
        .original_text
        .to_string();
    let api_key = context
        .keys
        .for_provider(context.provider)
        .ok_or_else(|| anyhow::anyhow!("no API key configured"))?;
    let translator = get_translator(
        context.cache.clone(),
        context.context.clone(),
        context.gemini.clone(),
        context.provider,
        &job.model,
        api_key.into(),
        source,
        context.target,
    )?;
    let estimate = context
        .stats
        .get(&source, &context.target)
        .await
        .estimate(text.len());
    job.expected.store(estimate, Ordering::Relaxed);
    let progress = job.progress.clone();
    let translation = translator
        .get_translation(TranslationContext {
            paragraph_text: &text,
            book_id: job.snapshot.book.id,
            chapter_id: job
                .snapshot
                .book
                .chapter_for_paragraph(job.paragraph)
                .unwrap_or(0),
            use_cache: job.use_cache,
            callback: Some(Box::new(move |chars| {
                progress.store(chars, Ordering::Relaxed);
            })),
        })
        .await?;
    let actual = serde_json::to_vec(&translation)?.len();
    context
        .stats
        .record_observation(&source, &context.target, text.len(), actual)
        .await;
    let receipt = job
        .book
        .apply_translation(
            job.snapshot.token,
            job.paragraph,
            text,
            source,
            context.target,
            translation,
            job.model.clone(),
        )
        .await?;
    job.book
        .flush_through(receipt.revision)
        .await
        .map_err(|err| anyhow::anyhow!("translated, but saving the book failed: {err}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retries_transient_errors_with_a_bound() {
        let error = anyhow::anyhow!("OpenAI request timed out");
        assert!(should_requeue(&error, 0));
        assert!(!should_requeue(&error, MAX_TRANSLATION_ATTEMPTS - 1));
        assert!(!should_requeue(
            &anyhow::anyhow!("paragraph content changed"),
            0
        ));
    }
}
