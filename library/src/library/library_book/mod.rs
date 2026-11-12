use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use isolang::Language;
use serde::{Deserialize, Serialize};
use tokio::{sync::watch, time::Instant};
use uuid::Uuid;

use crate::{
    book::{
        book::Book,
        chapter_summaries::{ChapterSummaries, ChapterSummary},
        translation_import::ParagraphTranslation,
    },
    library::Library,
    store::{BookInfo, BookReader, LibraryStore, NewVersion},
};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BookReadingState {
    #[serde(alias = "chapterId")]
    pub chapter_id: usize,
    #[serde(alias = "paragraphId")]
    pub paragraph_id: usize,
    // Which column of the saved paragraph the reader was on; zero unless the
    // paragraph flowed across columns. Tells restore which page to land on.
    #[serde(default, alias = "pageOffset")]
    pub page_offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct BookUserState {
    #[serde(default, rename = "readingState")]
    pub reading_state: Option<BookReadingState>,
    #[serde(default, rename = "folderPath")]
    pub folder_path: Vec<String>,
}

/// How long a reading position waits before it is committed. Flipping pages
/// would otherwise be a commit, and a file for every other device to fetch,
/// each.
const POSITION_DELAY: Duration = Duration::from_secs(2);

/// What every book of an open library writes through.
pub(crate) struct Writes {
    pub(crate) store: LibraryStore,
    /// Reading positions not handed to the store yet: only the last one of
    /// a run of page turns is worth a row.
    positions: Mutex<HashMap<Uuid, BookReadingState>>,
    /// When the next commit is due, for `Library::commit_when_due`.
    pub(crate) commit_due: watch::Sender<Option<Instant>>,
    /// Bumped whenever some book's summaries may have changed.
    pub(crate) summaries_changed: watch::Sender<u64>,
}

impl Writes {
    pub(crate) fn new(store: LibraryStore) -> Arc<Self> {
        Arc::new(Self {
            store,
            positions: Mutex::default(),
            commit_due: watch::channel(None).0,
            summaries_changed: watch::channel(0).0,
        })
    }

    fn commit_within(&self, delay: Duration) {
        let at = Instant::now() + delay;
        self.commit_due.send_if_modified(|due| match due {
            Some(held) if *held <= at => false,
            _ => {
                *due = Some(at);
                true
            }
        });
    }

    pub(crate) fn note_summaries_changed(&self) {
        self.summaries_changed
            .send_modify(|n| *n = n.wrapping_add(1));
    }

    pub(crate) fn forget(&self, book: Uuid) {
        self.positions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&book);
    }

    /// Commits everything this device has changed. What a failed commit
    /// leaves is staged in the store, which lands it with its next one.
    pub(crate) async fn land(&self) -> anyhow::Result<()> {
        let positions =
            std::mem::take(&mut *self.positions.lock().unwrap_or_else(|e| e.into_inner()));
        self.store
            .run(move |db| {
                for (book, position) in positions {
                    // The book may have been deleted since.
                    if db.has_book(book)? {
                        db.set_user_state(book, Some(position), None)?;
                    }
                }
                db.publish()
            })
            .await
    }
}

/// One book of an open library. It holds the book's row and nothing else:
/// every read and write goes to the store.
pub struct BookRef {
    info: BookInfo,
    writes: Arc<Writes>,
}
pub type BookHandle = Arc<BookRef>;

impl BookRef {
    pub fn info(&self) -> &BookInfo {
        &self.info
    }

    pub fn id(&self) -> Uuid {
        self.info.id
    }

    /// Reads the book's text and translations from the store. What
    /// `apply_translation` has answered for is there to read.
    pub async fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(BookReader<'_>) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let id = self.info.id;
        self.writes.store.read(move |db| f(db.book(id)?)).await
    }

    pub async fn summaries(&self) -> anyhow::Result<ChapterSummaries> {
        self.read(|book| book.summaries()).await
    }

    /// Changes whenever the summaries of some book may have.
    pub fn summaries_changed(&self) -> watch::Receiver<u64> {
        self.writes.summaries_changed.subscribe()
    }

    /// Readable at once; `flush` says whether it reached the disk.
    pub async fn apply_translation(
        &self,
        paragraph: usize,
        target: Language,
        translation: ParagraphTranslation,
        model: String,
    ) -> anyhow::Result<()> {
        let id = self.info.id;
        let version = NewVersion {
            source_language: self.info.language.clone(),
            target_language: target.to_639_3().to_owned(),
            paragraph,
            translation,
            model,
        };
        self.writes
            .store
            .run(move |db| db.add_version(id, version))
            .await?;
        self.writes.commit_within(Duration::ZERO);
        Ok(())
    }

    /// Refused when a summary before `chapter` is no longer the one in
    /// `written_from`: each one is written from those before it.
    pub async fn apply_summary(
        &self,
        written_from: &ChapterSummaries,
        chapter: usize,
        summary: ChapterSummary,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            chapter < written_from.entries.len(),
            "summary chapter out of range"
        );
        let id = self.info.id;
        let prior = written_from.entries[..chapter].to_vec();
        self.writes
            .store
            .run(move |db| db.set_summary(id, chapter, &prior, summary))
            .await?;
        self.writes.note_summaries_changed();
        self.writes.commit_within(Duration::ZERO);
        Ok(())
    }

    /// Ok once everything applied so far, to any book, is on disk.
    pub async fn flush(&self) -> anyhow::Result<()> {
        self.writes.land().await
    }

    async fn user_state(&self) -> anyhow::Result<BookUserState> {
        let id = self.info.id;
        self.writes
            .store
            .read(move |db| {
                anyhow::ensure!(db.has_book(id)?, "book {id} not found");
                db.user_state(id)
            })
            .await
    }

    pub async fn reading_state(&self) -> anyhow::Result<Option<BookReadingState>> {
        let waiting = self
            .writes
            .positions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&self.info.id)
            .cloned();
        match waiting {
            Some(position) => Ok(Some(position)),
            None => Ok(self.user_state().await?.reading_state),
        }
    }

    pub async fn update_reading_state(&self, state: BookReadingState) -> anyhow::Result<()> {
        // A deleted book takes no position; one deleted later drops it.
        self.user_state().await?;
        self.writes
            .positions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(self.info.id, state);
        self.writes.commit_within(POSITION_DELAY);
        Ok(())
    }

    pub async fn folder_path(&self) -> anyhow::Result<Vec<String>> {
        Ok(self.user_state().await?.folder_path)
    }

    pub async fn update_folder_path(&self, path: Vec<String>) -> anyhow::Result<()> {
        let id = self.info.id;
        self.writes
            .store
            .run(move |db| {
                db.set_user_state(id, None, Some(path))?;
                db.publish()
            })
            .await
    }
}

impl Library {
    pub async fn get_book(&self, uuid: &Uuid) -> anyhow::Result<BookHandle> {
        let id = *uuid;
        let info = self.store.read(move |db| db.book_info(id)).await?;
        Ok(Arc::new(BookRef {
            info,
            writes: self.writes.clone(),
        }))
    }

    pub async fn create_book(&self, book: Book) -> anyhow::Result<BookHandle> {
        let book: Arc<Book> = Arc::new(book);
        let id = self.store.run(move |db| db.create_book(book)).await?;
        self.get_book(&id).await
    }

    /// Deletes the book's data for real; whatever is written to it later is refused.
    pub async fn delete_book(&self, uuid: &Uuid) -> anyhow::Result<()> {
        self.writes.forget(*uuid);
        self.store.delete_book(*uuid).await
    }
}
