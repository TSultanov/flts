use crate::app::{
    config::{ApiKeys, Config},
    task::ManagedTask,
};
use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};
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
use std::panic::AssertUnwindSafe;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};
use tauri::Emitter;
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;
const MAX_TRANSLATION_ATTEMPTS: u32 = 4;
const MAX_PENDING: usize = 1024;
fn should_requeue(error: &anyhow::Error, attempt: u32) -> bool {
    attempt + 1 < MAX_TRANSLATION_ATTEMPTS && is_transient_translation_error(error)
}
#[derive(Clone, serde::Serialize)]
pub(crate) struct ParagraphUpdatedEvent {
    #[serde(rename = "bookId")]
    pub(crate) book_id: Uuid,
    #[serde(rename = "paragraphId")]
    pub(crate) paragraph_id: usize,
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
#[derive(Clone, Copy, Default)]
struct Progress {
    chars: usize,
    expected: usize,
}
struct Job {
    request_id: usize,
    book: BookHandle,
    snapshot: Arc<BookSnapshot>,
    paragraph: usize,
    model: String,
    use_cache: bool,
    attempt: u32,
}
struct ChapterJob {
    book: BookHandle,
    snapshot: Arc<BookSnapshot>,
    chapter: usize,
    /// The chapter's paragraphs, and how many of them were looked at.
    paragraphs: Vec<usize>,
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
struct WorkerContext {
    library: Library,
    cache: TranslationsCache,
    stats: TranslationSizeCache,
    gemini: GeminiPromptCache,
    context: Box<dyn ChapterContextProvider>,
    provider: library::translator::TranslationProvider,
    keys: ApiKeys,
    target: Language,
}
#[derive(Clone)]
pub struct TranslationQueue {
    requests: mpsc::Sender<Request>,
    activity: watch::Receiver<Arc<Activities>>,
    control: library::task::TaskControl,
}
impl TranslationQueue {
    pub fn init(
        library: Library,
        cache: TranslationsCache,
        stats: TranslationSizeCache,
        gemini: GeminiPromptCache,
        context: Box<dyn ChapterContextProvider>,
        config: &Config,
        app: tauri::AppHandle,
        library_tx: watch::Sender<Option<Library>>,
    ) -> Option<(Self, ManagedTask)> {
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
        let keepalive = requests.clone();
        let task = ManagedTask::spawn(move |stop| async move {
            let _keepalive = keepalive;
            scheduler(
                worker,
                concurrency,
                receiver,
                stop,
                publish,
                app,
                library_tx,
            )
            .await;
        });
        Some((
            Self {
                requests,
                activity,
                control: task.control(),
            },
            task,
        ))
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
        if let Err(err) = self.control.shutdown().await {
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
    make_job(request_id, book, snapshot, paragraph, model, use_cache)
}
fn make_job(
    request_id: usize,
    book: BookHandle,
    snapshot: Arc<BookSnapshot>,
    paragraph: usize,
    model: String,
    use_cache: bool,
) -> Job {
    Job {
        request_id,
        book,
        snapshot,
        paragraph,
        model,
        use_cache,
        attempt: 0,
    }
}

#[cfg(feature = "ownership-bench")]
pub fn benchmark_pending_jobs(
    book: &BookHandle,
    snapshot: &Arc<BookSnapshot>,
    count: usize,
) -> impl Sized {
    (0..count)
        .map(|id| {
            make_job(
                id,
                book.clone(),
                snapshot.clone(),
                id,
                "benchmark".to_owned(),
                false,
            )
        })
        .collect::<VecDeque<_>>()
}

async fn scheduler(
    context: WorkerContext,
    concurrency: usize,
    mut requests: mpsc::Receiver<Request>,
    mut stop: watch::Receiver<bool>,
    publish: watch::Sender<Arc<Activities>>,
    app: tauri::AppHandle,
    library_tx: watch::Sender<Option<Library>>,
) {
    let mut activities = HashMap::new();
    let mut next_id = 0;
    let mut pending: VecDeque<Job> = VecDeque::new();
    let mut chapters: VecDeque<ChapterJob> = VecDeque::new();
    let mut workers = FuturesUnordered::new();
    let mut running: HashMap<usize, (Uuid, usize, watch::Receiver<Progress>)> = HashMap::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(500));
    // Idle while nothing runs; the first tick after that must not replay the gap.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Set by every edit of `activities`; cleared when the copy is published.
    let mut changed = false;
    loop {
        while pending.len() < MAX_PENDING && !chapters.is_empty() {
            let chapter = chapters.front_mut().unwrap();
            let Some(&paragraph) = chapter.paragraphs.get(chapter.offset) else {
                chapters.pop_front();
                continue;
            };
            chapter.offset += 1;
            // Paragraphs translated since the request must not be paid for again.
            let target = context.target;
            let translated = chapter
                .book
                .read(move |book| book.is_translated(&target, paragraph))
                .await;
            match translated {
                Ok(false) if !activities.contains_key(&(chapter.snapshot.book.id, paragraph)) => {}
                Ok(_) => continue,
                Err(err) => {
                    log::warn!("chapter translation dropped: {err:#}");
                    chapters.pop_front();
                    continue;
                }
            }
            changed = true;
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
            let (progress, observed) = watch::channel(Progress::default());
            running.insert(
                job.request_id,
                (job.snapshot.book.id, job.paragraph, observed),
            );
            let context = &context;
            workers.push(async move {
                let result = AssertUnwindSafe(execute(&job, context, &progress))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("translation worker panicked")));
                (job, result)
            });
        }
        if std::mem::take(&mut changed) {
            publish.send_replace(Arc::new(activities.clone()));
        }
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            Some(first) = workers.next(), if !workers.is_empty() => {
                // Workers sharing one save finish together; the listeners of
                // `book_updated` and the library watch re-read whole books, so
                // each batch raises them once.
                let mut books: Vec<Uuid> = Vec::new();
                let mut finished = Vec::new();
                let mut next = Some(first);
                while let Some((mut job, result)) = next {
                    next = workers.next().now_or_never().flatten();
                    changed = true;
                    running.remove(&job.request_id);
                    let id = job.snapshot.book.id;
                    if let Err(error) = &result {
                        if should_requeue(error, job.attempt) {
                            job.attempt += 1;
                            if let Some(activity) = activities.get_mut(&(id, job.paragraph)) { activity.progress_chars = 0; }
                            pending.push_front(job); continue;
                        }
                    }
                    activities.remove(&(id, job.paragraph));
                    let _ = app.emit("paragraph_updated", ParagraphUpdatedEvent {book_id: id, paragraph_id: job.paragraph});
                    if !books.contains(&id) { books.push(id); }
                    finished.push(ParagraphTranslationFinishedEvent {book_id: id, paragraph_id: job.paragraph, request_id: job.request_id, error: result.err().map(|e| e.to_string())});
                }
                for id in &books { let _ = app.emit("book_updated", id); }
                if !books.is_empty() { library_tx.send_modify(|_| {}); }
                for event in finished { let _ = app.emit("paragraph_translation_finished", event); }
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
                        if result.is_ok() { publish.send_replace(Arc::new(activities.clone())); }
                        let _ = reply.send(result);
                    }
                    Request::Chapter {book_id, chapter, model, use_cache, reply} => {
                        let result = async {
                            anyhow::ensure!(chapters.len() < 256, "translation queue busy");
                            let book = context.library.get_book(&book_id).await?; let snapshot = book.snapshot();
                            anyhow::ensure!(chapter < snapshot.book.chapter_count(), "chapter out of range");
                            let target = context.target;
                            let (paragraphs, count) = book.read(move |book| {
                                let outline = book.outline()?;
                                let paragraphs = outline.chapters.get(chapter).map(|c| c.paragraphs.clone()).unwrap_or_default();
                                let count = book.translated(&target, &paragraphs)?.into_iter().filter(|t| !t).count();
                                Ok((paragraphs, count))
                            }).await?;
                            if !chapters.iter().any(|c| c.snapshot.book.id == book_id && c.chapter == chapter) { chapters.push_back(ChapterJob {book, snapshot, chapter, paragraphs, offset: 0, model, use_cache}); }
                            Ok(count)
                        }.await;
                        let _ = reply.send(result);
                    }
                }
            }
            _ = ticker.tick(), if !running.is_empty() => {
                for (request_id, (id, paragraph, progress)) in &running {
                    let latest = *progress.borrow(); let chars = latest.chars; let estimate = latest.expected;
                    if let Some(activity) = activities.get_mut(&(*id, *paragraph)) {
                        if activity.request_id != *request_id || (activity.progress_chars == chars && activity.expected_chars == estimate) { continue; }
                        activity.progress_chars = chars; activity.expected_chars = estimate; changed = true;
                        let _ = app.emit("paragraph_translation_progress", ParagraphTranslationProgressEvent {book_id: *id, paragraph_id: *paragraph, request_id: *request_id, progress_chars: chars, expected_chars: estimate});
                    }
                }
            }
        }
    }
    requests.close();
    workers.clear();
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
/// Measures a serialization without keeping it.
struct ByteCount(usize);
impl std::io::Write for ByteCount {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
async fn execute(
    job: &Job,
    context: &WorkerContext,
    progress: &watch::Sender<Progress>,
) -> anyhow::Result<()> {
    let source = Language::from_639_3(&job.snapshot.book.language)
        .ok_or_else(|| anyhow::anyhow!("invalid source language"))?;
    let paragraph = job.paragraph;
    let (text, chapter) = job
        .book
        .read(move |book| {
            Ok((
                book.paragraph(paragraph)?.text,
                book.outline()?.chapter_for_paragraph(paragraph),
            ))
        })
        .await?;
    let api_key = context
        .keys
        .for_provider(context.provider)
        .ok_or_else(|| anyhow::anyhow!("no API key configured"))?;
    let translator = get_translator(
        &context.cache,
        context.context.as_ref(),
        &context.gemini,
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
    progress.send_modify(|p| p.expected = estimate);
    let translation = translator
        .get_translation(TranslationContext {
            paragraph_text: &text,
            book_id: job.snapshot.book.id,
            chapter_id: chapter.unwrap_or(0),
            use_cache: job.use_cache,
            callback: Some(Box::new(move |chars| {
                progress.send_modify(|p| p.chars = chars);
            })),
        })
        .await?;
    let mut actual = ByteCount(0);
    serde_json::to_writer(&mut actual, &translation)?;
    let actual = actual.0;
    context
        .stats
        .record_observation(&source, &context.target, text.len(), actual)
        .await;
    job.book
        .apply_translation(
            job.paragraph,
            context.target,
            translation,
            job.model.clone(),
        )
        .await?;
    job.book
        .flush()
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
