//! `ChapterContextProvider` implementation backed by the
//! `SummaryGenerationQueue` (for summaries) and the in-memory library
//! (for chapter text). Lives in the Tauri app crate because both
//! dependencies are app-scoped; the library crate stays unaware.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use library::{
    library::Library,
    translator::ChapterContextProvider,
};
use tokio::time::timeout;
use uuid::Uuid;

use crate::app::summary_generation_queue::{
    SummaryGenerationQueue, concat_prior_summaries,
};

/// How long a paragraph translation will wait for `wait_ready` before
/// degrading to the no-summaries cache variant. Picked to comfortably
/// cover a few Flash-Lite summary calls (each ~1–3s) while still bounding
/// pathological "stuck book" scenarios.
const WAIT_READY_TIMEOUT: Duration = Duration::from_secs(60);

pub struct SummaryBackedChapterContext {
    pub queue: Arc<SummaryGenerationQueue>,
    pub library: Arc<Library>,
}

#[async_trait]
impl ChapterContextProvider for SummaryBackedChapterContext {
    async fn wait_ready(&self, book_id: Uuid, chapter_index: usize) -> anyhow::Result<()> {
        // Make sure the book is enqueued; harmless no-op if already
        // processing or already complete.
        self.queue.enqueue(book_id);

        let state = self
            .queue
            .get_or_init_book_state(&self.library, book_id)
            .await?;
        let mut rx = state.subscribe_ready();
        // Quick check before subscribing for the next change.
        if let Some(ready_through) = *rx.borrow()
            && ready_through >= chapter_index
        {
            return Ok(());
        }

        let wait = async {
            loop {
                rx.changed().await?;
                if let Some(ready_through) = *rx.borrow()
                    && ready_through >= chapter_index
                {
                    return Ok::<(), anyhow::Error>(());
                }
            }
        };
        timeout(WAIT_READY_TIMEOUT, wait)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "chapter summaries not ready for book {book_id} chapter {chapter_index} within {WAIT_READY_TIMEOUT:?}"
                )
            })??;
        Ok(())
    }

    async fn prior_summaries(
        &self,
        book_id: Uuid,
        chapter_index: usize,
    ) -> anyhow::Result<String> {
        let state = self
            .queue
            .get_or_init_book_state(&self.library, book_id)
            .await?;
        let summaries = state.summaries.lock().await;
        Ok(concat_prior_summaries(&summaries, chapter_index))
    }

    async fn chapter_text(
        &self,
        book_id: Uuid,
        chapter_index: usize,
    ) -> anyhow::Result<String> {
        let book = self.library.get_book(&book_id).await?;
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
