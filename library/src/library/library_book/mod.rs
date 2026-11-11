use std::sync::Arc;

use isolang::Language;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    book::{
        book::Book,
        chapter_summaries::{ChapterSummaries, ChapterSummary},
        translation::{ParagraphTranslationView, Translation},
        translation_import,
    },
    library::Library,
    store::{Annotations, BookDelta, LibraryStore, LoadedBook, NewVersion, translation_uuid},
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

pub struct LibraryBook {
    store: LibraryStore,
    pub book: Arc<Book>,
    /// The store generation the annotations were last read at.
    adopted: u64,
    translations: Vec<LibraryTranslation>,
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

pub struct LibraryTranslation {
    translation: Translation,
    /// Versions the store does not hold yet.
    pending: Vec<NewVersion>,
    /// The translation as last handed to a snapshot; dropped by every
    /// mutation so the next snapshot clones again.
    published: Option<Arc<Translation>>,
}

impl LibraryTranslation {
    fn stored(translation: Translation) -> Self {
        Self {
            translation,
            pending: Vec::new(),
            published: None,
        }
    }

    pub fn is_changed(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn add_paragraph_translation(
        &mut self,
        paragraph_index: usize,
        translation: &translation_import::ParagraphTranslation,
        model: &str,
    ) {
        let mut translation = translation.clone();
        // Versions order by timestamp once stored, so the newest must also
        // carry the newest stamp.
        if let Some(latest) = self.translation.latest_timestamp(paragraph_index)
            && translation.timestamp <= latest
        {
            translation.timestamp = latest + 1;
        }
        self.translation
            .add_paragraph_translation(paragraph_index, &translation, model);
        self.pending.push(NewVersion {
            source_language: self.translation.source_language.clone(),
            target_language: self.translation.target_language.clone(),
            paragraph: paragraph_index,
            translation,
            model: model.to_owned(),
        });
        self.published = None;
    }

    pub fn translated_paragraphs_count(&self) -> usize {
        self.translation.translated_paragraphs_count()
    }

    pub fn paragraph_view(&'_ self, paragraph: usize) -> Option<ParagraphTranslationView<'_>> {
        self.translation.paragraph_view(paragraph)
    }
}

impl LibraryBook {
    fn new(store: LibraryStore, book: Book) -> Self {
        let summaries = Arc::new(ChapterSummaries::empty_for(book.id, book.chapter_count()));
        Self {
            store,
            book: Arc::new(book),
            adopted: 0,
            translations: vec![],
            user_state: BookUserState::default(),
            pending_reading_state: None,
            pending_folder_path: None,
            #[cfg(test)]
            save_count: 0,
            summaries,
            dirty_summaries: Vec::new(),
            status: BookStatus::Active,
        }
    }

    pub(crate) async fn load(store: LibraryStore, id: Uuid) -> anyhow::Result<Self> {
        let LoadedBook { book, annotations } = store.read(move |db| db.load_book(id)).await?;
        let mut book = Self::new(store, book);
        book.adopt(annotations);
        Ok(book)
    }

    fn adopt(&mut self, annotations: Annotations) {
        self.translations = annotations
            .translations
            .into_iter()
            .map(LibraryTranslation::stored)
            .collect();
        self.summaries = Arc::new(annotations.summaries);
        self.user_state = annotations.user_state;
        self.adopted = annotations.generation;
    }

    pub(crate) fn snapshot(&mut self) -> Arc<BookSnapshot> {
        let translations = self
            .translations
            .iter_mut()
            .map(|t| {
                t.published
                    .get_or_insert_with(|| Arc::new(t.translation.clone()))
                    .clone()
            })
            .collect();
        Arc::new(BookSnapshot {
            book: self.book.clone(),
            translations,
            summaries: self.summaries.clone(),
            user_state: self.user_state.clone(),
            store_generation: self.adopted,
            status: self.status,
        })
    }

    /// Changes the store has not been handed yet.
    pub fn has_unsaved_changes(&self) -> bool {
        !self.delta().is_empty()
    }

    pub(crate) fn only_position_unsaved(&self) -> bool {
        let delta = self.delta();
        delta.reading_state.is_some()
            && delta.versions.is_empty()
            && delta.summaries.is_empty()
            && delta.folder_path.is_none()
    }

    fn delta(&self) -> BookDelta {
        BookDelta {
            book: None,
            versions: self
                .translations
                .iter()
                .flat_map(|t| t.pending.iter().map(NewVersion::clone))
                .collect(),
            summaries: self
                .dirty_summaries
                .iter()
                .map(|&chapter| (chapter, self.summaries.entries[chapter].clone()))
                .collect(),
            reading_state: self.pending_reading_state.clone(),
            folder_path: self.pending_folder_path.clone(),
        }
    }

    /// Hands everything unsaved to the store and commits. Once staged it is
    /// the store's to land, so a failed commit is reported and not kept here.
    pub(crate) async fn persist(&mut self) -> anyhow::Result<()> {
        let delta = self.delta();
        if delta.is_empty() {
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
        for translation in &mut self.translations {
            translation.pending.clear();
        }
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
        let loaded = self
            .store
            .read(move |db| db.load_annotations(id, chapters))
            .await?;
        let changed = loaded.translations.len() != self.translations.len()
            || loaded.translations.iter().any(|loaded| {
                self.translations
                    .iter()
                    .find(|held| held.translation.id == loaded.id)
                    .is_none_or(|held| held.translation.version_count() != loaded.version_count())
            })
            || loaded.summaries.entries != self.summaries.entries
            || loaded.user_state != self.user_state;
        if changed {
            self.adopt(loaded);
        } else {
            self.adopted = loaded.generation;
        }
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

    fn translation_index(&self, target_language: &Language) -> Option<usize> {
        let source_language = &self.book.language;
        let target_language = target_language.to_639_3();
        self.translations.iter().position(|t| {
            &t.translation.source_language == source_language
                && t.translation.target_language == target_language
        })
    }

    pub fn get_translation(&self, target_language: &Language) -> Option<&LibraryTranslation> {
        self.translation_index(target_language)
            .map(|idx| &self.translations[idx])
    }

    pub fn get_or_create_translation(
        &mut self,
        target_language: &Language,
    ) -> &mut LibraryTranslation {
        let idx = match self.translation_index(target_language) {
            Some(idx) => idx,
            None => {
                let source_language = &self.book.language;
                let mut translation =
                    Translation::create(source_language, target_language.to_639_3());
                translation.id =
                    translation_uuid(self.book.id, source_language, target_language.to_639_3());
                self.translations
                    .push(LibraryTranslation::stored(translation));
                self.translations.len() - 1
            }
        };
        &mut self.translations[idx]
    }
}

impl Library {
    pub async fn create_book(&self, book: Book) -> anyhow::Result<BookHandle> {
        let book = Arc::new(book);
        let id = self.store.run(move |db| db.create_book(book)).await?;
        self.get_book(&id).await
    }

    pub async fn delete_book(&self, uuid: &Uuid) -> anyhow::Result<()> {
        self.remove_book(*uuid).await
    }
}
