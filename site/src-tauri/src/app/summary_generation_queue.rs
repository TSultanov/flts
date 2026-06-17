//! Background generator for per-chapter source-language summaries.
//!
//! Mirrors `TranslationQueue` in shape: channel-driven worker that takes
//! book ids, processes pending chapters sequentially (the chain needs the
//! prior chapter's summary as input), and persists the
//! `chapter_summaries.dat` sidecar after each chapter completes. Per-book
//! readiness is broadcast on a `watch::Sender<Option<usize>>` so the
//! Gemini / OpenAI translators can `wait_ready` before composing a
//! per-paragraph request.

use std::{collections::HashMap, sync::Arc, time::SystemTime};

use library::{
    book::chapter_summaries::{
        ChapterSummaries, ChapterSummary, chapter_summaries_path,
    },
    library::Library,
    summary_generator::generate_chapter_summary,
    translator::{TranslationModel, TranslationProvider},
};
use log::{info, warn};
use tauri::Emitter;
use tokio::sync::{
    Mutex,
    mpsc::{UnboundedSender, unbounded_channel},
    watch,
};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::app::config::Config;

/// Per-book in-memory state: the loaded sidecar (which gets mutated as
/// chapters complete) and a watch channel carrying the highest-ready
/// chapter index. `None` means chapter 0 isn't ready yet.
pub struct BookSummaryState {
    pub summaries: Mutex<ChapterSummaries>,
    pub ready_tx: watch::Sender<Option<usize>>,
}

impl BookSummaryState {
    pub fn subscribe_ready(&self) -> watch::Receiver<Option<usize>> {
        self.ready_tx.subscribe()
    }
}

#[derive(Clone, serde::Serialize)]
struct SummaryGenerationProgress {
    #[serde(rename = "bookId")]
    book_id: Uuid,
    current: usize,
    total: usize,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub struct SummaryGenerationQueue {
    enqueue_tx: UnboundedSender<Uuid>,
    book_state: Arc<Mutex<HashMap<Uuid, Arc<BookSummaryState>>>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl SummaryGenerationQueue {
    pub fn init(
        library: Arc<Library>,
        config: &Config,
        app: tauri::AppHandle,
    ) -> Arc<Self> {
        let model = config.model;
        let gemini_api_key = config.gemini_api_key.clone();
        let openai_api_key = config.openai_api_key.clone();
        let deepseek_api_key = config.deepseek_api_key.clone();
        let zai_api_key = config.zai_api_key.clone();

        let (enqueue_tx, mut enqueue_rx) = unbounded_channel::<Uuid>();
        let book_state: Arc<Mutex<HashMap<Uuid, Arc<BookSummaryState>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let book_state_for_worker = book_state.clone();
        let task = tokio::spawn(async move {
            while let Some(book_id) = enqueue_rx.recv().await {
                let outcome = process_book(
                    book_id,
                    library.clone(),
                    book_state_for_worker.clone(),
                    model,
                    gemini_api_key.as_deref(),
                    openai_api_key.as_deref(),
                    deepseek_api_key.as_deref(),
                    zai_api_key.as_deref(),
                    &app,
                )
                .await;
                if let Err(err) = outcome {
                    warn!("Summary generation for book {book_id} stopped: {err}");
                }
            }
        });

        Arc::new(Self {
            enqueue_tx,
            book_state,
            task: Mutex::new(Some(task)),
        })
    }

    /// Schedule the next pending chapter (or no-op if all done).
    /// Idempotent: extra calls just push another item on the channel; the
    /// worker will short-circuit if there's nothing to do.
    pub fn enqueue(&self, book_id: Uuid) {
        let _ = self.enqueue_tx.send(book_id);
    }

    /// Returns the in-memory state for `book_id`, loading the sidecar
    /// from disk on first call. Used by the `ChapterContextProvider` to
    /// subscribe to readiness and read summary text.
    pub async fn get_or_init_book_state(
        &self,
        library: &Library,
        book_id: Uuid,
    ) -> anyhow::Result<Arc<BookSummaryState>> {
        load_or_init(&self.book_state, library, book_id).await
    }

    pub async fn shutdown(&self) {
        if let Some(task) = self.task.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

async fn load_or_init(
    book_state: &Arc<Mutex<HashMap<Uuid, Arc<BookSummaryState>>>>,
    library: &Library,
    book_id: Uuid,
) -> anyhow::Result<Arc<BookSummaryState>> {
    {
        let guard = book_state.lock().await;
        if let Some(state) = guard.get(&book_id) {
            return Ok(state.clone());
        }
    }
    // Need to materialize. We do the I/O without holding the map lock so
    // a slow load doesn't block other books.
    let (book_path, chapter_count) = {
        let book = library.get_book(&book_id).await?;
        let book = book.lock().await;
        (book.path().to_path_buf(), book.book.chapter_count())
    };
    let path = chapter_summaries_path(&book_path);
    let loaded = if tokio::fs::try_exists(&path).await? {
        // Discover any conflict files left by an interrupted save.
        let mut conflicts = Vec::new();
        let mut dir = tokio::fs::read_dir(&book_path).await?;
        while let Some(entry) = dir.next_entry().await? {
            let p = entry.path();
            if let Some(name) = p.file_name().and_then(|n| n.to_str())
                && name.starts_with("chapter_summaries~")
                && name.ends_with(".dat")
            {
                conflicts.push(p);
            }
        }
        ChapterSummaries::load_from_metadata(&path, &conflicts).await?
    } else {
        ChapterSummaries::empty_for(book_id, chapter_count)
    };
    let initial_ready = loaded.ready_through();
    let (ready_tx, _) = watch::channel(initial_ready);
    let state = Arc::new(BookSummaryState {
        summaries: Mutex::new(loaded),
        ready_tx,
    });

    // Race-safe insert: another caller may have inserted while we were
    // doing I/O. If so, return their copy.
    let mut guard = book_state.lock().await;
    if let Some(existing) = guard.get(&book_id) {
        return Ok(existing.clone());
    }
    guard.insert(book_id, state.clone());
    Ok(state)
}

async fn process_book(
    book_id: Uuid,
    library: Arc<Library>,
    book_state: Arc<Mutex<HashMap<Uuid, Arc<BookSummaryState>>>>,
    model: TranslationModel,
    gemini_api_key: Option<&str>,
    openai_api_key: Option<&str>,
    deepseek_api_key: Option<&str>,
    zai_api_key: Option<&str>,
    app: &tauri::AppHandle,
) -> anyhow::Result<()> {
    let provider = model
        .provider()
        .ok_or_else(|| anyhow::anyhow!("model has no provider"))?;
    let api_key = match provider {
        TranslationProvider::Google => gemini_api_key,
        TranslationProvider::Openai => openai_api_key,
        TranslationProvider::Deepseek => deepseek_api_key,
        TranslationProvider::Zai => zai_api_key,
    }
    .ok_or_else(|| anyhow::anyhow!("no api key for provider {provider:?}"))?;

    let state = load_or_init(&book_state, &library, book_id).await?;

    // Pull stable book metadata once. The sidecar path also stays stable
    // for the life of the book.
    let (book_path, book_title, book_language) = {
        let book = library.get_book(&book_id).await?;
        let book = book.lock().await;
        let lang = isolang::Language::from_639_3(&book.book.language)
            .ok_or_else(|| anyhow::anyhow!("unknown book language: {}", book.book.language))?;
        (
            book.path().to_path_buf(),
            book.book.title.clone(),
            lang,
        )
    };
    let sidecar_path = chapter_summaries_path(&book_path);

    loop {
        // Snapshot the next chapter to summarize + the prior summary, all
        // under the summaries lock to avoid TOCTOU. Drop the lock before
        // making the LLM call.
        let work = {
            let summaries = state.summaries.lock().await;
            let Some(idx) = summaries.next_pending() else {
                emit_progress(
                    app,
                    book_id,
                    summaries.entries.len(),
                    summaries.entries.len(),
                    "done",
                    None,
                );
                return Ok(());
            };
            let prior = concat_prior_summaries(&summaries, idx);
            let total = summaries.entries.len();
            (idx, prior, total)
        };
        let (idx, prior, total) = work;

        // Pull this chapter's title + text snapshot.
        let (chapter_title, chapter_text) = {
            let book = library.get_book(&book_id).await?;
            let book = book.lock().await;
            let chapter = book.book.chapter_view(idx);
            let title = chapter.title.as_ref().map(|t| t.to_string());
            let mut text = String::new();
            for (i, para) in chapter.paragraphs().enumerate() {
                if i > 0 {
                    text.push_str("\n\n");
                }
                text.push_str(&para.original_text);
            }
            (title, text)
        };

        emit_progress(app, book_id, idx, total, "in_progress", None);

        info!("Generating summary: book={book_id} chapter={idx}/{total}");
        let summary_text = match generate_chapter_summary(
            provider,
            model,
            api_key,
            &book_language,
            &book_title,
            chapter_title.as_deref(),
            &chapter_text,
            if prior.is_empty() { None } else { Some(&prior) },
        )
        .await
        {
            Ok(text) => text,
            Err(err) => {
                warn!("Summary generation failed for book={book_id} ch={idx}: {err}");
                emit_progress(app, book_id, idx, total, "failed", Some(err.to_string()));
                // Per the plan: stop processing this book; the chapter
                // stays generated=false. The next enqueue retries.
                return Ok(());
            }
        };

        // Record and persist.
        {
            let mut summaries = state.summaries.lock().await;
            summaries.entries[idx] = ChapterSummary {
                generated: true,
                model,
                timestamp: SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                text: summary_text,
            };
            summaries.save(&sidecar_path).await?;
            let ready = summaries.ready_through();
            // It's fine if there are no current subscribers — the watch
            // channel holds the latest value for anyone who subscribes later.
            let _ = state.ready_tx.send(ready);
            emit_progress(app, book_id, idx + 1, total, "in_progress", None);
        }
    }
}

/// Concatenate generated summaries for chapters `0..chapter_index`, each
/// prefixed with a `Chapter X[: title]` header. Empty string for
/// `chapter_index == 0` or when no earlier chapters are generated yet.
pub fn concat_prior_summaries(
    summaries: &ChapterSummaries,
    chapter_index: usize,
) -> String {
    let mut out = String::new();
    for i in 0..chapter_index.min(summaries.entries.len()) {
        let entry = &summaries.entries[i];
        if !entry.generated {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format!("Chapter {}", i + 1));
        out.push_str(":\n");
        out.push_str(&entry.text);
    }
    out
}

fn emit_progress(
    app: &tauri::AppHandle,
    book_id: Uuid,
    current: usize,
    total: usize,
    status: &'static str,
    error: Option<String>,
) {
    let _ = app.emit(
        "summary_generation_progress",
        SummaryGenerationProgress {
            book_id,
            current,
            total,
            status,
            error,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::translator::TranslationModel;

    #[test]
    fn concat_prior_summaries_handles_gaps() {
        let mut s = ChapterSummaries::empty_for(Uuid::nil(), 4);
        s.entries[0] = ChapterSummary {
            generated: true,
            model: TranslationModel::Gemini25Flash,
            timestamp: 1,
            text: "first".into(),
        };
        // entry 1 is pending — should be skipped, not blank-included
        s.entries[2] = ChapterSummary {
            generated: true,
            model: TranslationModel::Gemini25Flash,
            timestamp: 2,
            text: "third".into(),
        };
        let prior = concat_prior_summaries(&s, 3);
        assert!(prior.contains("Chapter 1:"));
        assert!(prior.contains("first"));
        assert!(prior.contains("Chapter 3:"));
        assert!(prior.contains("third"));
        assert!(!prior.contains("Chapter 2:"));
    }

    #[test]
    fn concat_prior_summaries_empty_for_chapter_zero() {
        let s = ChapterSummaries::empty_for(Uuid::nil(), 3);
        assert!(concat_prior_summaries(&s, 0).is_empty());
    }
}
