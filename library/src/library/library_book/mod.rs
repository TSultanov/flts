use std::sync::Arc;

use isolang::Language;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    book::{
        book::Book,
        chapter_summaries::{ChapterSummaries, ChapterSummary},
        translation_import,
    },
    library::Library,
    store::{Annotations, BookDelta, BookInfo, LibraryStore, LoadedBook, NewVersion},
};

mod handle;
#[cfg(test)]
mod tests;

pub use handle::{BookHandle, BookRef, BookSnapshot, BookStatus, BookSubscription};
pub(crate) use handle::{StopMode, owner, serve_book};

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

/// What an open book holds: its row, and the small things the reader
/// changes. Its text and translations stay in the store and are read there.
pub struct LibraryBook {
    store: LibraryStore,
    pub book: Arc<BookInfo>,
    /// The store generation the annotations were last read at.
    adopted: u64,
    /// Translations staged in the store and not committed yet.
    staged: bool,
    user_state: BookUserState,
    pending_reading_state: Option<BookReadingState>,
    pending_folder_path: Option<Vec<String>>,
    /// Commits attempted.
    #[cfg(test)]
    save_count: usize,
    summaries: Arc<ChapterSummaries>,
    dirty_summaries: Vec<usize>,
    status: BookStatus,
}

impl LibraryBook {
    pub(crate) async fn load(store: LibraryStore, id: Uuid) -> anyhow::Result<Self> {
        let LoadedBook { info, annotations } = store.read(move |db| db.load_book(id)).await?;
        Ok(Self {
            store,
            book: Arc::new(info),
            adopted: annotations.generation,
            staged: false,
            user_state: annotations.user_state,
            pending_reading_state: None,
            pending_folder_path: None,
            #[cfg(test)]
            save_count: 0,
            summaries: Arc::new(annotations.summaries),
            dirty_summaries: Vec::new(),
            status: BookStatus::Active,
        })
    }

    fn adopt(&mut self, annotations: Annotations) {
        self.summaries = Arc::new(annotations.summaries);
        self.user_state = annotations.user_state;
        self.adopted = annotations.generation;
    }

    pub(crate) fn snapshot(&mut self) -> Arc<BookSnapshot> {
        Arc::new(BookSnapshot {
            book: self.book.clone(),
            summaries: self.summaries.clone(),
            user_state: self.user_state.clone(),
            store_generation: self.adopted,
            status: self.status,
        })
    }

    /// Changes not committed yet.
    pub fn has_unsaved_changes(&self) -> bool {
        self.staged || !self.delta().is_empty()
    }

    pub(crate) fn only_position_unsaved(&self) -> bool {
        let delta = self.delta();
        !self.staged
            && delta.reading_state.is_some()
            && delta.summaries.is_empty()
            && delta.folder_path.is_none()
    }

    fn delta(&self) -> BookDelta {
        BookDelta {
            book: None,
            versions: Vec::new(),
            summaries: self
                .dirty_summaries
                .iter()
                .map(|&chapter| (chapter, self.summaries.entries[chapter].clone()))
                .collect(),
            reading_state: self.pending_reading_state.clone(),
            folder_path: self.pending_folder_path.clone(),
        }
    }

    /// Stages a translation at once, so that whoever reads the paragraph
    /// next finds it; the commit follows with the batch.
    pub(crate) async fn add_translation(
        &mut self,
        paragraph: usize,
        target: &Language,
        translation: translation_import::ParagraphTranslation,
        model: String,
    ) -> anyhow::Result<()> {
        let id = self.book.id;
        let version = NewVersion {
            source_language: self.book.language.clone(),
            target_language: target.to_639_3().to_owned(),
            paragraph,
            translation,
            model,
        };
        self.store
            .run(move |db| db.add_version(id, version))
            .await?;
        self.staged = true;
        Ok(())
    }

    /// Hands everything unsaved to the store and commits. Once staged it is
    /// the store's to land, so a failed commit is reported and not kept here.
    pub(crate) async fn persist(&mut self) -> anyhow::Result<()> {
        let delta = self.delta();
        if delta.is_empty() && !self.staged {
            return Ok(());
        }
        #[cfg(test)]
        {
            self.save_count += 1;
        }
        let id = self.book.id;
        let committed = self
            .store
            .run(move |db| {
                db.stage(id, &delta, false)?;
                Ok(db.publish().map_err(|err| format!("{err:#}")))
            })
            .await?;
        self.staged = false;
        self.dirty_summaries.clear();
        self.pending_reading_state = None;
        self.pending_folder_path = None;
        committed.map_err(anyhow::Error::msg)
    }

    /// Ok once nothing of this device's is waiting for a commit.
    pub(crate) async fn flush(&mut self) -> anyhow::Result<()> {
        self.persist().await?;
        self.store.run(|db| db.publish()).await
    }

    /// Adopts what the store holds now, including what other devices wrote.
    /// True when anything a reader sees changed.
    pub async fn reload(&mut self) -> anyhow::Result<bool> {
        // Staged is enough: the store reads back what it was handed.
        if let Err(err) = self.persist().await
            && self.has_unsaved_changes()
        {
            return Err(err);
        }
        let (id, chapters) = (self.book.id, self.book.chapter_count());
        let (loaded, changed_at) = self
            .store
            .read(move |db| Ok((db.load_annotations(id, chapters)?, db.book_generation(id))))
            .await?;
        // Translations are not held here; the store says when they changed.
        let changed = changed_at > self.adopted
            || loaded.summaries.entries != self.summaries.entries
            || loaded.user_state != self.user_state;
        self.adopt(loaded);
        Ok(changed)
    }

    /// The newest stamp per chapter is the current summary once stored, so
    /// a replacement must carry a newer one than what it replaces.
    pub(crate) fn set_summary(&mut self, chapter: usize, mut summary: ChapterSummary) {
        let held = &self.summaries.entries[chapter];
        if held.generated && summary.timestamp <= held.timestamp {
            summary.timestamp = held.timestamp + 1;
        }
        self.summaries_mut().entries[chapter] = summary;
        self.mark_summary_dirty(chapter);
    }

    pub(crate) fn mark_summary_dirty(&mut self, chapter: usize) {
        if !self.dirty_summaries.contains(&chapter) {
            self.dirty_summaries.push(chapter);
        }
    }

    pub(crate) fn summaries_mut(&mut self) -> &mut ChapterSummaries {
        Arc::make_mut(&mut self.summaries)
    }

    /// Staged with the next commit; a page turn is not worth one of its own.
    pub fn set_reading_state(&mut self, state: BookReadingState) {
        self.pending_reading_state = Some(state.clone());
        self.user_state.reading_state = Some(state);
    }

    pub async fn update_folder_path(&mut self, folder_path: Vec<String>) -> anyhow::Result<()> {
        self.pending_folder_path = Some(folder_path.clone());
        self.user_state.folder_path = folder_path;
        self.persist().await
    }
}

impl Library {
    pub async fn create_book(&self, book: Book) -> anyhow::Result<BookHandle> {
        let book: Arc<Book> = Arc::new(book);
        let id = self.store.run(move |db| db.create_book(book)).await?;
        self.get_book(&id).await
    }

    pub async fn delete_book(&self, uuid: &Uuid) -> anyhow::Result<()> {
        self.remove_book(*uuid).await
    }
}
