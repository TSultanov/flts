//! Reads of one book's parts. Nothing here holds more than what was asked
//! for; the store keeps what it has read from disk.

use std::sync::Arc;

use anyhow::Context;
use dbsync::{Id, schema::Table};
use isolang::Language;
use uuid::Uuid;

use super::{
    BookInfo, ChapterOutline, Inner, Latest, Newest, Outline, ParagraphText, StoredVersion,
    schema::{self, book_id, derived, to_db},
    translation_uuid,
};
use crate::book::{
    chapter_summaries::ChapterSummaries,
    words::{self, ParagraphWords},
};

impl Inner {
    pub(super) fn forget_read_caches(&self) {
        self.latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.outlines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    pub(super) fn with_latest<T>(
        &self,
        translation: Id<schema::Translation>,
        read: impl FnOnce(&Latest) -> T,
    ) -> anyhow::Result<T> {
        let mut cache = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(latest) = cache.get(&translation.uuid()) {
            return Ok(read(latest));
        }
        let table = &self.db.paragraph_translations;
        let mut latest = Latest::default();
        for id in self
            .db
            .children_of::<schema::Translation, schema::ParagraphTranslation>(translation)?
        {
            latest.note(
                table.paragraph(id)?,
                Newest {
                    timestamp: table.timestamp(id)?,
                    id,
                    empty: table.empty(id)?,
                },
            );
        }
        Ok(read(cache.entry(translation.uuid()).or_insert(latest)))
    }

    /// The book's translation into `target`, if it has one.
    pub(super) fn translation_row(
        &self,
        book: Uuid,
        target: &str,
    ) -> anyhow::Result<Option<Id<schema::Translation>>> {
        let source = self.db.books.language(book_id(book))?;
        let id = Id::from_uuid(to_db(translation_uuid(book, &source, target)));
        Ok(self.db.translations.live_row_of(id)?.map(|_| id))
    }

    pub fn book_info(&self, id: Uuid) -> anyhow::Result<BookInfo> {
        anyhow::ensure!(self.has_book(id)?, "book {id} not found");
        let row = book_id(id);
        anyhow::ensure!(
            self.is_complete(row)?,
            "book {id} has not fully arrived yet"
        );
        Ok(BookInfo {
            id,
            title: self.db.books.title(row)?.into_owned(),
            language: self.db.books.language(row)?.into_owned(),
            chapters: self.db.books.chapters_count(row)? as usize,
            paragraphs: self.db.books.paragraphs_count(row)? as usize,
        })
    }

    pub fn outline(&self, id: Uuid) -> anyhow::Result<Arc<Outline>> {
        if let Some(outline) = self
            .outlines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
        {
            return Ok(outline.clone());
        }
        let info = self.book_info(id)?;
        let mut chapters = Vec::with_capacity(info.chapters);
        let mut chapter_of = vec![0; info.paragraphs];
        for chapter in self
            .db
            .children_of::<schema::Book, schema::Chapter>(book_id(id))?
        {
            let mut paragraphs = Vec::new();
            for paragraph in self
                .db
                .children_of::<schema::Chapter, schema::Paragraph>(chapter)?
            {
                let number = self.db.paragraphs.number(paragraph)? as usize;
                *chapter_of
                    .get_mut(number)
                    .with_context(|| format!("book {id} has a paragraph past its end"))? =
                    chapters.len() as u32;
                paragraphs.push(number);
            }
            paragraphs.sort_unstable();
            chapters.push(ChapterOutline {
                title: self.db.chapters.title(chapter)?.map(|t| t.into_owned()),
                paragraphs,
            });
        }
        let outline = Arc::new(Outline {
            chapters,
            chapter_of,
        });
        self.outlines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, outline.clone());
        Ok(outline)
    }

    pub fn paragraph(&self, book: Uuid, number: usize) -> anyhow::Result<ParagraphText> {
        let id = derived::<schema::Paragraph>(to_db(book), &format!("paragraph/{number}"));
        anyhow::ensure!(
            self.db.paragraphs.live_row_of(id)?.is_some(),
            "book {book} has no paragraph {number}"
        );
        Ok(ParagraphText {
            text: self.db.paragraphs.text(id)?.into_owned(),
            html: self.db.paragraphs.html(id)?.map(|h| h.into_owned()),
        })
    }

    pub fn summaries(&self, book: Uuid) -> anyhow::Result<ChapterSummaries> {
        let chapters = self.db.books.chapters_count(book_id(book))? as usize;
        self.load_summaries(book, chapters)
    }

    fn newest(&self, book: Uuid, target: &str, paragraph: usize) -> anyhow::Result<Option<Newest>> {
        let Some(translation) = self.translation_row(book, target)? else {
            return Ok(None);
        };
        let Ok(paragraph) = u32::try_from(paragraph) else {
            return Ok(None);
        };
        self.with_latest(translation, |l| l.by_paragraph.get(&paragraph).copied())
    }

    /// For each of `paragraphs`, whether `target` has a translation of it.
    /// One that came back empty is a refusal and reads as none.
    pub fn translated(
        &self,
        book: Uuid,
        target: &str,
        paragraphs: &[usize],
    ) -> anyhow::Result<Vec<bool>> {
        let Some(translation) = self.translation_row(book, target)? else {
            return Ok(vec![false; paragraphs.len()]);
        };
        self.with_latest(translation, |latest| {
            paragraphs
                .iter()
                .map(|paragraph| {
                    u32::try_from(*paragraph)
                        .ok()
                        .and_then(|p| latest.by_paragraph.get(&p))
                        .is_some_and(|newest| !newest.empty)
                })
                .collect()
        })
    }

    pub fn is_translated(
        &self,
        book: Uuid,
        target: &str,
        paragraph: usize,
    ) -> anyhow::Result<bool> {
        Ok(self.translated(book, target, &[paragraph])?[0])
    }

    /// What drawing the paragraph's current translation takes.
    pub fn paragraph_words(
        &self,
        book: Uuid,
        target: &str,
        paragraph: usize,
    ) -> anyhow::Result<Option<ParagraphWords>> {
        match self.newest(book, target, paragraph)? {
            Some(newest) if !newest.empty => Ok(Some(words::read_words(
                &self.db.paragraph_translations.words(newest.id)?,
            )?)),
            _ => Ok(None),
        }
    }

    fn version(&self, id: Id<schema::ParagraphTranslation>) -> anyhow::Result<StoredVersion> {
        let table = &self.db.paragraph_translations;
        let mut translation = words::join(&table.words(id)?, &table.details(id)?)
            .with_context(|| format!("paragraph translation {id}"))?;
        translation.timestamp = table.timestamp(id)?;
        translation.total_tokens = table.total_tokens(id)?;
        Ok(StoredVersion {
            translation,
            model: table.model(id)?.into_owned(),
        })
    }

    /// The paragraph's whole current translation, word details included.
    pub fn paragraph_translation(
        &self,
        book: Uuid,
        target: &str,
        paragraph: usize,
    ) -> anyhow::Result<Option<StoredVersion>> {
        match self.newest(book, target, paragraph)? {
            Some(newest) if !newest.empty => Ok(Some(self.version(newest.id)?)),
            _ => Ok(None),
        }
    }

    /// Every stored version of the paragraph's translation, oldest first.
    pub fn versions(
        &self,
        book: Uuid,
        target: &str,
        paragraph: usize,
    ) -> anyhow::Result<Vec<StoredVersion>> {
        let Some(translation) = self.translation_row(book, target)? else {
            return Ok(Vec::new());
        };
        let table = &self.db.paragraph_translations;
        let mut ids = Vec::new();
        for id in self
            .db
            .children_of::<schema::Translation, schema::ParagraphTranslation>(translation)?
        {
            if table.paragraph(id)? as usize == paragraph {
                ids.push((table.timestamp(id)?, id));
            }
        }
        ids.sort_unstable();
        let mut versions: Vec<StoredVersion> = Vec::with_capacity(ids.len());
        for (_, id) in ids {
            let mut version = self.version(id)?;
            // Two devices can stamp the same second; keep both, in id order.
            if let Some(last) = versions.last()
                && version.translation.timestamp <= last.translation.timestamp
            {
                version.translation.timestamp = last.translation.timestamp + 1;
            }
            versions.push(version);
        }
        Ok(versions)
    }

    /// Versions stored for `target`, of every paragraph.
    pub fn version_count(&self, book: Uuid, target: &str) -> anyhow::Result<usize> {
        Ok(match self.translation_row(book, target)? {
            Some(translation) => self
                .db
                .children_of::<schema::Translation, schema::ParagraphTranslation>(translation)?
                .len(),
            None => 0,
        })
    }

    pub fn translated_count_in(&self, book: Uuid, target: &str) -> anyhow::Result<usize> {
        match self.translation_row(book, target)? {
            Some(translation) => self.translated_count(translation),
            None => Ok(0),
        }
    }
}

/// One book's parts, read on demand.
#[derive(Clone, Copy)]
pub struct BookReader<'a> {
    db: &'a Inner,
    id: Uuid,
}

impl Inner {
    pub fn book(&self, id: Uuid) -> anyhow::Result<BookReader<'_>> {
        anyhow::ensure!(self.has_book(id)?, "book {id} not found");
        Ok(BookReader { db: self, id })
    }
}

impl BookReader<'_> {
    pub fn info(&self) -> anyhow::Result<BookInfo> {
        self.db.book_info(self.id)
    }

    pub fn outline(&self) -> anyhow::Result<Arc<Outline>> {
        self.db.outline(self.id)
    }

    pub fn paragraph(&self, number: usize) -> anyhow::Result<ParagraphText> {
        self.db.paragraph(self.id, number)
    }

    /// The plain text of a chapter's paragraphs, in reading order.
    pub fn chapter_text(&self, chapter: usize) -> anyhow::Result<Vec<String>> {
        let outline = self.outline()?;
        let chapter = outline
            .chapters
            .get(chapter)
            .with_context(|| format!("chapter {chapter} is out of range"))?;
        chapter
            .paragraphs
            .iter()
            .map(|number| Ok(self.paragraph(*number)?.text))
            .collect()
    }

    pub fn summaries(&self) -> anyhow::Result<ChapterSummaries> {
        self.db.summaries(self.id)
    }

    pub fn translated(&self, target: &Language, paragraphs: &[usize]) -> anyhow::Result<Vec<bool>> {
        self.db.translated(self.id, target.to_639_3(), paragraphs)
    }

    pub fn is_translated(&self, target: &Language, paragraph: usize) -> anyhow::Result<bool> {
        self.db.is_translated(self.id, target.to_639_3(), paragraph)
    }

    pub fn translated_count(&self, target: &Language) -> anyhow::Result<usize> {
        self.db.translated_count_in(self.id, target.to_639_3())
    }

    pub fn words(
        &self,
        target: &Language,
        paragraph: usize,
    ) -> anyhow::Result<Option<ParagraphWords>> {
        self.db
            .paragraph_words(self.id, target.to_639_3(), paragraph)
    }

    pub fn translation(
        &self,
        target: &Language,
        paragraph: usize,
    ) -> anyhow::Result<Option<StoredVersion>> {
        self.db
            .paragraph_translation(self.id, target.to_639_3(), paragraph)
    }

    pub fn versions(
        &self,
        target: &Language,
        paragraph: usize,
    ) -> anyhow::Result<Vec<StoredVersion>> {
        self.db.versions(self.id, target.to_639_3(), paragraph)
    }

    pub fn version_count(&self, target: &Language) -> anyhow::Result<usize> {
        self.db.version_count(self.id, target.to_639_3())
    }
}
