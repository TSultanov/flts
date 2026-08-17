//! `ChapterContextProvider` over the `SummaryGenerationQueue` and the in-memory
//! library. Both dependencies are app-scoped, so it lives here rather than in
//! the library crate.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use library::{
    library::Library,
    translator::ChapterContextProvider,
};
use tokio::sync::watch;
use tokio::time::timeout;
use uuid::Uuid;

use crate::app::summary_generation_queue::{
    SummaryGenerationQueue, concat_prior_summaries,
};

/// Covers a few Flash-Lite summary calls (~1–3s each) while still bounding a
/// stuck book.
const WAIT_READY_TIMEOUT: Duration = Duration::from_secs(60);

pub struct SummaryBackedChapterContext {
    pub queue: Arc<SummaryGenerationQueue>,
    /// Borrowed per operation, so a config change swapping in a fresh `Library`
    /// is picked up rather than pinned at construction.
    pub library_rx: watch::Receiver<Option<Arc<Library>>>,
}

impl SummaryBackedChapterContext {
    fn current_library(&self) -> anyhow::Result<Arc<Library>> {
        self.library_rx
            .borrow()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no library is open"))
    }
}

#[async_trait]
impl ChapterContextProvider for SummaryBackedChapterContext {
    async fn wait_ready(&self, book_id: Uuid, chapter_index: usize) -> anyhow::Result<()> {
        if chapter_index == 0 {
            return Ok(());
        }
        // `prior_summaries(K)` reads only 0..K-1.
        let needed = chapter_index - 1;

        // No-op when already processing or complete.
        self.queue.enqueue(book_id);

        let library = self.current_library()?;
        let state = self
            .queue
            .get_or_init_book_state(&library, book_id)
            .await?;
        let mut rx = state.subscribe_ready();
        // The summaries map is authoritative; the watch only signals changes.
        if let Some(ready_through) = state.summaries.lock().await.ready_through()
            && ready_through >= needed
        {
            return Ok(());
        }

        let wait = async {
            loop {
                rx.changed().await?;
                if let Some(ready_through) = *rx.borrow()
                    && ready_through >= needed
                {
                    return Ok::<(), anyhow::Error>(());
                }
            }
        };
        // "timed out" must stay in the message: it is what makes
        // `is_transient_translation_error` requeue rather than fail.
        timeout(WAIT_READY_TIMEOUT, wait)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "chapter summaries for book {book_id} chapter {chapter_index} timed out after {WAIT_READY_TIMEOUT:?}"
                )
            })??;
        Ok(())
    }

    async fn prior_summaries(
        &self,
        book_id: Uuid,
        chapter_index: usize,
    ) -> anyhow::Result<String> {
        let library = self.current_library()?;
        let state = self
            .queue
            .get_or_init_book_state(&library, book_id)
            .await?;
        let summaries = state.summaries.lock().await;
        Ok(concat_prior_summaries(&summaries, chapter_index))
    }

    async fn chapter_text(
        &self,
        book_id: Uuid,
        chapter_index: usize,
    ) -> anyhow::Result<String> {
        let book = self.current_library()?.get_book(&book_id).await?;
        let book = book.lock().await;
        if chapter_index >= book.book.chapter_count() {
            anyhow::bail!(
                "chapter index {chapter_index} out of range for book {book_id}"
            );
        }
        let chapter = book.book.chapter_view(chapter_index);
        let mut text = String::new();
        for (i, para) in chapter.paragraphs().enumerate() {
            if i > 0 {
                text.push_str("\n\n");
            }
            text.push_str(&para.original_text);
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_ready_timeout_error_is_transient() {
        // Rewording the message away from "timed out" makes it permanent.
        let book_id = Uuid::new_v4();
        let chapter_index = 5usize;
        let err = anyhow::anyhow!(
            "chapter summaries for book {book_id} chapter {chapter_index} timed out after {WAIT_READY_TIMEOUT:?}"
        );
        assert!(library::translator::is_transient_translation_error(&err));
    }
}
