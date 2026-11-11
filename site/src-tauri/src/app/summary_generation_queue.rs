use crate::app::{config::Config, task::ManagedTask};
use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};
use library::{
    book::chapter_summaries::{ChapterSummaries, ChapterSummary},
    library::{BookHandle, Library},
    summary_generator::ChapterSummarizer,
};
use std::panic::AssertUnwindSafe;
use std::{
    collections::{HashSet, VecDeque},
    time::SystemTime,
};
use tauri::Emitter;
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

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
#[derive(Clone)]
pub struct SummaryGenerationQueue {
    enqueue: mpsc::Sender<Uuid>,
    control: library::task::TaskControl,
}
impl SummaryGenerationQueue {
    pub fn init(
        library_rx: watch::Receiver<Option<Library>>,
        config: &Config,
        app: tauri::AppHandle,
    ) -> (Self, ManagedTask) {
        let provider = config.translation_provider;
        let model = library::translator::catalog::effective_model_id(provider, &config.model);
        let key = config.api_keys().for_provider(provider).map(str::to_owned);
        let (enqueue, mut requests) = mpsc::channel(256);
        let keepalive = enqueue.clone();
        let task = ManagedTask::spawn(move |mut stop| async move {
            let _keepalive = keepalive;
            let Some(key) = key else {
                return;
            };
            let summarizer = match ChapterSummarizer::create(provider, &model, &key) {
                Ok(s) => s,
                Err(err) => {
                    log::warn!("summary setup failed: {err:#}");
                    return;
                }
            };
            let mut pending = VecDeque::new();
            let mut admitted = HashSet::new();
            let mut workers = FuturesUnordered::new();
            loop {
                if workers.is_empty() {
                    if let Some(id) = pending.pop_front() {
                        let library = library_rx.borrow().clone();
                        let app = app.clone();
                        let summarizer = &summarizer;
                        workers.push(async move {
                            let result = match library {
                                Some(library) => {
                                    AssertUnwindSafe(process_book(id, &library, summarizer, &app))
                                        .catch_unwind()
                                        .await
                                        .unwrap_or_else(|_| {
                                            Err(anyhow::anyhow!("summary worker panicked"))
                                        })
                                }
                                None => Err(anyhow::anyhow!("library closed")),
                            };
                            if let Err(err) = result {
                                emit_progress(&app, id, 0, 0, "failed", Some(err.to_string()));
                            }
                            id
                        });
                    }
                }
                tokio::select! {
                    biased;
                    _ = stop.changed() => break,
                    Some(id) = workers.next(), if !workers.is_empty() => { admitted.remove(&id); }
                    request = requests.recv(), if pending.len() < 1024 => match request {Some(id) => { if admitted.insert(id) { pending.push_back(id); } }, None => break}
                }
            }
            requests.close();
            workers.clear();
        });
        (
            Self {
                enqueue,
                control: task.control(),
            },
            task,
        )
    }
    pub async fn enqueue(&self, id: Uuid) -> anyhow::Result<()> {
        self.enqueue
            .send(id)
            .await
            .map_err(|_| anyhow::anyhow!("summary queue closed"))
    }
    pub async fn shutdown(&self) {
        if let Err(err) = self.control.shutdown().await {
            log::warn!("summary shutdown: {err}");
        }
    }
}
async fn process_book(
    id: Uuid,
    library: &Library,
    summarizer: &ChapterSummarizer,
    app: &tauri::AppHandle,
) -> anyhow::Result<()> {
    let handle: BookHandle = library.get_book(&id).await?;
    loop {
        let snapshot = handle.snapshot();
        let Some(index) = snapshot.summaries.next_pending() else {
            emit_progress(
                app,
                id,
                snapshot.summaries.entries.len(),
                snapshot.summaries.entries.len(),
                "done",
                None,
            );
            return Ok(());
        };
        anyhow::ensure!(
            index < snapshot.book.chapter_count(),
            "summary chapter out of range"
        );
        let language = isolang::Language::from_639_3(&snapshot.book.language)
            .ok_or_else(|| anyhow::anyhow!("invalid source language"))?;
        let chapter = snapshot.book.chapter_view(index);
        let title = chapter.title.as_deref().map(str::to_owned);
        let text = chapter
            .paragraphs()
            .map(|p| p.original_text.to_string())
            .collect::<Vec<_>>()
            .join("\n\n");
        let prior = concat_prior_summaries(&snapshot.summaries, index);
        emit_progress(
            app,
            id,
            index,
            snapshot.summaries.entries.len(),
            "in_progress",
            None,
        );
        let text = summarizer
            .generate(
                &language,
                &snapshot.book.title,
                title.as_deref(),
                &text,
                if prior.is_empty() { None } else { Some(&prior) },
            )
            .await?;
        handle
            .apply_summary(
                &snapshot,
                index,
                ChapterSummary {
                    generated: true,
                    model: summarizer.model.clone(),
                    timestamp: SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    text,
                },
            )
            .await?;
        handle.flush().await?;
        emit_progress(
            app,
            id,
            index + 1,
            snapshot.summaries.entries.len(),
            "in_progress",
            None,
        );
    }
}

pub fn concat_prior_summaries(summaries: &ChapterSummaries, chapter_index: usize) -> String {
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
    #[test]
    fn concat_prior_summaries_handles_gaps() {
        let mut s = ChapterSummaries::empty_for(Uuid::nil(), 4);
        s.entries[0] = ChapterSummary {
            generated: true,
            model: "models/gemini-2.5-flash".into(),
            timestamp: 1,
            text: "first".into(),
        };
        // entry 1 is pending — should be skipped, not blank-included
        s.entries[2] = ChapterSummary {
            generated: true,
            model: "models/gemini-2.5-flash".into(),
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
