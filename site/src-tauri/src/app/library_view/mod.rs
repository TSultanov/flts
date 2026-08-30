use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use htmlentity::entity::{ICodedDataTrait, decode};
use isolang::Language;
use library::card;
use library::epub_importer::EpubBook;
use library::library::file_watcher::LibraryFileChange;
use library::{
    book::translation::ParagraphTranslationView,
    library::{Library, library_book::BookReadingState},
};
use uuid::Uuid;

use crate::app::AppState;

pub mod imports;
pub mod mutations;
pub mod queries;

pub use imports::*;
pub use mutations::*;
pub use queries::*;

#[derive(Clone, serde::Serialize)]
pub struct LibraryBookMetadataView {
    id: Uuid,
    title: String,
    #[serde(rename = "chaptersCount")]
    chapters_count: usize,
    #[serde(rename = "paragraphsCount")]
    paragraphs_count: usize,
    #[serde(rename = "translationRatio")]
    translation_ratio: f64,
    #[serde(rename = "path")]
    path: Vec<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct ChapterView {
    id: usize,
    title: String,
    #[serde(rename = "translationRatio")]
    translation_ratio: f64,
}

#[derive(Clone, serde::Serialize)]
pub struct BookSummaryStatusView {
    #[serde(rename = "totalChapters")]
    pub total_chapters: usize,
    /// Per-chapter `generated` flag, indexed by chapter id.
    pub generated: Vec<bool>,
    /// First not-yet-generated chapter index. Chained generation makes this the
    /// worker's next chapter too, so the UI spins on it.
    #[serde(rename = "activelyGenerating", skip_serializing_if = "Option::is_none")]
    pub actively_generating: Option<usize>,
}

#[derive(Clone, serde::Serialize)]
pub struct ParagraphView {
    id: usize,
    original: String,
    segments: Option<Vec<ParagraphSegment>>,
}

#[derive(Clone, serde::Serialize)]
pub struct ParagraphOriginal {
    id: usize,
    original: String,
}

#[derive(Clone, serde::Serialize)]
pub struct ParagraphTranslationSlice {
    id: usize,
    segments: Option<Vec<ParagraphSegment>>,
}

/// Inline emphasis carried by a segment. The EPUB sanitizer allows only
/// `em, i, b, br`, so `i`/`em` normalize to `Emphasis` and `b` to `Strong`.
#[derive(Clone, Copy, serde::Serialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Mark {
    Emphasis,
    Strong,
}

/// The single source of truth for the mounted and the virtualized rendering.
/// A segment carries decoded text and structured marks, not raw HTML: a tag
/// that spans several segments cannot survive parsing as independent
/// `{@html}` fragments.
#[derive(Clone, serde::Serialize, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ParagraphSegment {
    Gap {
        text: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        marks: Vec<Mark>,
    },
    Break {
        #[serde(skip_serializing_if = "Vec::is_empty")]
        marks: Vec<Mark>,
    },
    Word {
        text: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        marks: Vec<Mark>,
        sentence: usize,
        word: usize,
        #[serde(rename = "flatIndex")]
        flat_index: usize,
        translation: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        familiarity: Option<f32>,
    },
}

#[derive(Clone, serde::Serialize)]
pub struct WordView {
    original: String,
    note: String,
    #[serde(rename = "isPunctuation")]
    is_punctuation: bool,
    grammar: GrammarView,
    #[serde(rename = "contextualTranslations")]
    contextual_translations: Vec<String>,
    #[serde(rename = "fullSentenceTranslation")]
    full_sentence_translation: String,
    #[serde(rename = "translationModel")]
    translation_model: String,
    #[serde(rename = "sourceLanguage")]
    source_language: String,
}

#[derive(Clone, serde::Serialize)]
pub struct BookReadingStateView {
    #[serde(rename = "chapterId")]
    chapter_id: usize,
    #[serde(rename = "paragraphId")]
    paragraph_id: usize,
    #[serde(rename = "pageOffset")]
    page_offset: usize,
}

impl From<BookReadingState> for BookReadingStateView {
    fn from(value: BookReadingState) -> Self {
        Self {
            chapter_id: value.chapter_id,
            paragraph_id: value.paragraph_id,
            page_offset: value.page_offset,
        }
    }
}

#[derive(Clone, serde::Serialize)]
pub struct GrammarView {
    #[serde(rename = "originalInitialForm")]
    original_initial_form: String,
    #[serde(rename = "targetInitialForm")]
    target_initial_form: String,
    #[serde(rename = "partOfSpeech")]
    part_of_speech: String,
    plurality: Option<String>,
    person: Option<String>,
    tense: Option<String>,
    case: Option<String>,
    other: Option<String>,
}

pub struct LibraryView {
    state: Arc<AppState>,
    library: Arc<Library>,
}

impl LibraryView {
    pub fn create(state: Arc<AppState>, library: Arc<Library>) -> Self {
        Self { state, library }
    }

    pub async fn get_paragraph_view(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
        target_language: &Language,
    ) -> anyhow::Result<ParagraphView> {
        let book = self.library.get_book(&book_id).await?;
        let book = book.lock().await;

        // The frontend can hold paragraph ids from before a sync-triggered
        // book reload; indexing past the end would panic the command.
        if paragraph_id >= book.book.paragraphs_count() {
            anyhow::bail!(
                "paragraph {paragraph_id} out of range for book {book_id} ({} paragraphs)",
                book.book.paragraphs_count()
            );
        }

        // Must stay read-only: minting a translation would cement a book whose
        // translations failed to load as untranslated, and diverge translation
        // ids across synced devices.
        let book_translation = book.get_translation(target_language).await;

        let paragraph = book.book.paragraph_view(paragraph_id);
        let original = paragraph.original_html.unwrap_or(paragraph.original_text);

        let src_lang = Language::from_639_3(&book.book.language).ok_or_else(|| {
            anyhow::anyhow!(
                "book has invalid ISO-639-3 language code: {:?}",
                book.book.language
            )
        })?;
        let card_store = self.library.card_store();

        let bt = match &book_translation {
            Some(t) => Some(t.lock().await),
            None => None,
        };
        let t_view = bt.as_ref().and_then(|bt| bt.paragraph_view(paragraph_id));

        let segments = if let Some(t) = t_view.as_ref() {
            let mut slug_set: HashSet<String> = HashSet::new();
            collect_paragraph_slugs(t, src_lang, &mut slug_set);
            let slugs: Vec<String> = slug_set.into_iter().collect();
            let fam = card_store
                .familiarities(src_lang.to_639_3(), target_language.to_639_3(), &slugs)
                .await;
            Some(paragraph_to_segments(&original, t, &fam, src_lang))
        } else {
            None
        };

        Ok(ParagraphView {
            id: paragraph_id,
            original: original.to_string(),
            segments,
        })
    }

    pub async fn get_paragraph_originals_batch(
        &self,
        book_id: Uuid,
        paragraph_ids: Vec<usize>,
    ) -> anyhow::Result<Vec<ParagraphOriginal>> {
        let book = self.library.get_book(&book_id).await?;
        let book = book.lock().await;
        // Skip ids past the end: the frontend may hold state from before a
        // sync-triggered reload shrank the book.
        Ok(paragraph_ids
            .into_iter()
            .filter(|id| *id < book.book.paragraphs_count())
            .map(|id| {
                let p = book.book.paragraph_view(id);
                let original = p.original_html.unwrap_or(p.original_text).to_string();
                ParagraphOriginal { id, original }
            })
            .collect())
    }

    pub async fn get_paragraph_translations_batch(
        &self,
        book_id: Uuid,
        paragraph_ids: Vec<usize>,
        target_language: &Language,
    ) -> anyhow::Result<Vec<ParagraphTranslationSlice>> {
        let book = self.library.get_book(&book_id).await?;
        let book = book.lock().await;

        // Read-only; see get_paragraph_view. No matching translation yields
        // `segments: None` for every row.
        let book_translation = book.get_translation(target_language).await;
        let bt = match &book_translation {
            Some(t) => Some(t.lock().await),
            None => None,
        };

        let src_lang = Language::from_639_3(&book.book.language).ok_or_else(|| {
            anyhow::anyhow!(
                "book has invalid ISO-639-3 language code: {:?}",
                book.book.language
            )
        })?;
        let card_store = self.library.card_store();

        // Union the batch's lemma slugs first so the card store is hit once.
        // Ids past the end are skipped, as in get_paragraph_originals_batch.
        let mut prepared: Vec<(usize, String, Option<ParagraphTranslationView<'_>>)> =
            Vec::with_capacity(paragraph_ids.len());
        let mut slug_set: HashSet<String> = HashSet::new();
        for id in paragraph_ids {
            if id >= book.book.paragraphs_count() {
                continue;
            }
            let p = book.book.paragraph_view(id);
            let original = p.original_html.unwrap_or(p.original_text).to_string();
            let t_view = bt.as_ref().and_then(|bt| bt.paragraph_view(id));
            if let Some(t) = t_view.as_ref() {
                collect_paragraph_slugs(t, src_lang, &mut slug_set);
            }
            prepared.push((id, original, t_view));
        }

        let slugs: Vec<String> = slug_set.into_iter().collect();
        let fam = card_store
            .familiarities(src_lang.to_639_3(), target_language.to_639_3(), &slugs)
            .await;

        let out = prepared
            .iter()
            .map(|(id, original, t_view)| {
                let segments = t_view
                    .as_ref()
                    .map(|t| paragraph_to_segments(original, t, &fam, src_lang));
                ParagraphTranslationSlice { id: *id, segments }
            })
            .collect();
        Ok(out)
    }

    pub async fn list_books(
        &self,
        target_language: Option<&Language>,
    ) -> anyhow::Result<Vec<LibraryBookMetadataView>> {
        let books = self.library.list_books().await?;
        Ok(books
            .into_iter()
            .map(|b| {
                let translation = target_language.and_then(|tl| {
                    b.translations_metadata
                        .iter()
                        .find(|t| t.target_language == tl.to_639_3())
                });

                let translation_ratio = translation
                    .map(|t| {
                        if b.paragraphs_count == 0 {
                            0.0
                        } else {
                            t.translated_paragraphs_count as f64 / b.paragraphs_count as f64
                        }
                    })
                    .unwrap_or(0.0);

                LibraryBookMetadataView {
                    id: b.id,
                    title: b.title,
                    chapters_count: b.chapters_count,
                    paragraphs_count: b.paragraphs_count,
                    translation_ratio,
                    path: b.folder_path.clone(),
                }
            })
            .collect())
    }

    pub async fn list_book_chapters(
        &mut self,
        book_id: Uuid,
        target_language: Option<&Language>,
    ) -> anyhow::Result<Vec<ChapterView>> {
        let book = self.library.get_book(&book_id).await?;
        let chapters: Vec<ChapterView> = {
            let book_guard = book.lock().await;
            let translation_arc = match target_language {
                Some(tl) => book_guard.get_translation(tl).await,
                None => None,
            };
            let translation_guard = match &translation_arc {
                Some(arc) => Some(arc.lock().await),
                None => None,
            };
            book_guard
                .book
                .chapter_views()
                .map(|chapter| {
                    let total = chapter.paragraph_count();
                    let translated = if let Some(t) = translation_guard.as_ref() {
                        chapter
                            .paragraphs()
                            .filter(|p| t.paragraph_view(p.id).is_some())
                            .count()
                    } else {
                        0
                    };
                    let translation_ratio = if total == 0 {
                        0.0
                    } else {
                        translated as f64 / total as f64
                    };
                    let id = chapter.idx;
                    let title = chapter
                        .title
                        .map(|s| s.to_string())
                        .unwrap_or("<no title>".to_owned());
                    ChapterView {
                        id,
                        title,
                        translation_ratio,
                    }
                })
                .collect()
        };
        // Opening a book resumes summary generation; idempotent once complete.
        if let Ok(queue) = self.state.get_or_init_summary_generation_queue().await {
            queue.enqueue(book_id);
        }
        Ok(chapters)
    }

    pub async fn list_book_chapter_paragraph_ids(
        &self,
        book_id: Uuid,
        chapter_id: usize,
    ) -> anyhow::Result<Vec<usize>> {
        let book = self.library.get_book(&book_id).await?;
        let book = book.lock().await;
        // chapter_id is frontend/URL-supplied: chapter_view indexes raw, and
        // panic = abort would take the app down.
        if chapter_id >= book.book.chapter_count() {
            return Ok(Vec::new());
        }
        Ok(book
            .book
            .chapter_view(chapter_id)
            .paragraphs()
            .map(|p| p.id)
            .collect())
    }

    pub async fn get_word_info(
        &mut self,
        book_id: Uuid,
        paragraph_id: usize,
        sentence_id: usize,
        word_id: usize,
        target_language: &Language,
    ) -> anyhow::Result<Option<WordView>> {
        let (book_translation, source_language_code) = {
            let book = self.library.get_book(&book_id).await?;
            let book = book.lock().await;
            (
                // Read-only; see get_paragraph_view.
                book.get_translation(target_language).await,
                book.book.language.clone(),
            )
        };

        let Some(book_translation) = book_translation else {
            return Ok(None);
        };

        Ok(
            if let Some(paragraph) = book_translation.lock().await.paragraph_view(paragraph_id) {
                // The frontend refetches on `book_updated` with its selected
                // {sentence, word}, which a re-translation can shrink away. The
                // views index raw, and panic = abort would take the app down.
                if sentence_id >= paragraph.sentence_count() {
                    return Ok(None);
                }
                let sentence = paragraph.sentence_view(sentence_id);
                if word_id >= sentence.word_count() {
                    return Ok(None);
                }
                let word = sentence.word_view(word_id);
                Some(WordView {
                    original: word.original.to_string(),
                    note: word.note.to_string(),
                    is_punctuation: word.is_punctuation,
                    contextual_translations: word
                        .contextual_translations()
                        .map(|ct| ct.translation.to_string())
                        .collect(),
                    grammar: GrammarView {
                        original_initial_form: word.grammar.original_initial_form.to_string(),
                        target_initial_form: word.grammar.target_initial_form.to_string(),
                        part_of_speech: word.grammar.part_of_speech.to_string(),
                        plurality: word.grammar.plurality.map(|p| p.to_string()),
                        person: word.grammar.person.map(|p| p.to_string()),
                        tense: word.grammar.tense.map(|t| t.to_string()),
                        case: word.grammar.case.map(|c| c.to_string()),
                        other: word.grammar.other.map(|o| o.to_string()),
                    },
                    full_sentence_translation: sentence.full_translation.to_string(),
                    translation_model: paragraph.model,
                    source_language: source_language_code,
                })
            } else {
                None
            },
        )
    }

    pub async fn import_plain_text(
        &mut self,
        title: &str,
        text: &str,
        source_language: &Language,
    ) -> anyhow::Result<Uuid> {
        let id = self
            .library
            .create_book_plain(title, text, source_language)
            .await?;

        self.state.notify_library_changed();
        self.enqueue_summary_generation(id).await;

        Ok(id)
    }

    pub async fn import_epub(
        &mut self,
        book: &EpubBook,
        source_language: &Language,
    ) -> anyhow::Result<Uuid> {
        let id = self.library.create_book_epub(book, source_language).await?;

        self.state.notify_library_changed();
        self.enqueue_summary_generation(id).await;

        Ok(id)
    }

    async fn enqueue_summary_generation(&self, book_id: Uuid) {
        match self.state.get_or_init_summary_generation_queue().await {
            Ok(queue) => queue.enqueue(book_id),
            Err(err) => {
                log::warn!("Failed to init summary generation queue for book {book_id}: {err}")
            }
        }
    }

    pub async fn get_book_reading_state(
        &self,
        book_id: Uuid,
    ) -> anyhow::Result<Option<BookReadingStateView>> {
        let book = self.library.get_book(&book_id).await?;
        let mut book = book.lock().await;
        Ok(book.reading_state().await?.map(BookReadingStateView::from))
    }

    pub async fn save_book_reading_state(
        &self,
        book_id: Uuid,
        chapter_id: usize,
        paragraph_id: usize,
        page_offset: usize,
    ) -> anyhow::Result<()> {
        let book = self.library.get_book(&book_id).await?;
        let mut book = book.lock().await;
        book.update_reading_state(BookReadingState {
            chapter_id,
            paragraph_id,
            page_offset,
        })
        .await
    }

    pub async fn move_book(&self, book_id: Uuid, new_path: Vec<String>) -> anyhow::Result<()> {
        let book = self.library.get_book(&book_id).await?;
        {
            let mut book = book.lock().await;
            book.update_folder_path(new_path).await?;
        }

        self.state.notify_library_changed();
        Ok(())
    }

    pub async fn delete_book(&self, book_id: Uuid) -> anyhow::Result<()> {
        self.library.delete_book(&book_id).await?;
        self.state.notify_library_changed();
        Ok(())
    }

    pub async fn handle_file_change_event(
        &mut self,
        event: &LibraryFileChange,
    ) -> anyhow::Result<bool> {
        self.library.handle_file_change_event(event).await
    }
}

/// Adds every non-punctuation lemma slug to `out`; the caller's `HashSet` dedups
/// across paragraphs so each slug is loaded once.
fn collect_paragraph_slugs(
    translation: &ParagraphTranslationView<'_>,
    src_lang: Language,
    out: &mut HashSet<String>,
) {
    for sentence in translation.sentences() {
        for word in sentence.words() {
            if word.is_punctuation {
                continue;
            }
            let lemma_canonical =
                card::canonicalize_lemma(&word.grammar.original_initial_form, src_lang);
            if lemma_canonical.is_empty() {
                continue;
            }
            let slug = card::lemma_slug(&lemma_canonical);
            if slug.is_empty() {
                continue;
            }
            out.insert(slug);
        }
    }
}

/// One decoded character with the emphasis active at that point. `<br>` becomes
/// a `\n` flagged as a break so it neither matches word text nor is lost.
struct ProjChar {
    ch: char,
    marks: Vec<Mark>,
    is_break: bool,
}

/// Longest `&…;` an entity is worth scanning for before treating `&` as text.
const MAX_ENTITY_CHARS: usize = 32;

fn toggle_mark(marks: &mut Vec<Mark>, mark: Mark, closing: bool) {
    if closing {
        marks.retain(|m| *m != mark);
    } else if !marks.contains(&mark) {
        marks.push(mark);
        // Canonical order, so `<b><i>` and `<i><b>` compare equal.
        marks.sort();
    }
}

fn flatten_original(original: &[char]) -> Vec<ProjChar> {
    let mut out: Vec<ProjChar> = Vec::with_capacity(original.len());
    let mut marks: Vec<Mark> = Vec::new();
    let mut i = 0;
    while i < original.len() {
        if original[i] == '<'
            && let Some(gt) = (i + 1..original.len()).find(|&j| original[j] == '>')
        {
            let raw: String = original[i + 1..gt].iter().collect();
            let trimmed = raw.trim();
            let closing = trimmed.starts_with('/');
            let name = trimmed
                .trim_start_matches('/')
                .trim_end_matches('/')
                .trim()
                .to_ascii_lowercase();
            match name.as_str() {
                "br" => out.push(ProjChar {
                    ch: '\n',
                    marks: marks.clone(),
                    is_break: true,
                }),
                "b" | "strong" => toggle_mark(&mut marks, Mark::Strong, closing),
                "i" | "em" => toggle_mark(&mut marks, Mark::Emphasis, closing),
                // Anything else is stripped by the importer; drop it rather
                // than leak tag text into a word.
                _ => {}
            }
            i = gt + 1;
            continue;
        }

        if original[i] == '&' {
            let limit = original.len().min(i + MAX_ENTITY_CHARS);
            if let Some(semi) = (i + 1..limit).find(|&j| original[j] == ';') {
                let raw: String = original[i..=semi].iter().collect();
                let decoded = decode(raw.as_bytes()).to_string().unwrap_or_default();
                if !decoded.is_empty() && decoded != raw {
                    for ch in decoded.chars() {
                        out.push(ProjChar {
                            ch,
                            marks: marks.clone(),
                            is_break: false,
                        });
                    }
                    i = semi + 1;
                    continue;
                }
            }
        }

        out.push(ProjChar {
            ch: original[i],
            marks: marks.clone(),
            is_break: false,
        });
        i += 1;
    }
    out
}

/// Appends gap text, merging into the previous gap only when the marks match.
fn push_gap(segments: &mut Vec<ParagraphSegment>, text: String, marks: Vec<Mark>) {
    if text.is_empty() {
        return;
    }
    if let Some(ParagraphSegment::Gap {
        text: existing,
        marks: existing_marks,
    }) = segments.last_mut()
        && *existing_marks == marks
    {
        existing.push_str(&text);
        return;
    }
    segments.push(ParagraphSegment::Gap { text, marks });
}

/// Emits everything between two words, split wherever the marks change or a
/// break occurs, so every segment has one uniform mark set.
fn push_run(segments: &mut Vec<ParagraphSegment>, run: &[ProjChar]) {
    let mut idx = 0;
    while idx < run.len() {
        if run[idx].is_break {
            segments.push(ParagraphSegment::Break {
                marks: run[idx].marks.clone(),
            });
            idx += 1;
            continue;
        }
        let start = idx;
        let marks = run[idx].marks.clone();
        while idx < run.len() && !run[idx].is_break && run[idx].marks == marks {
            idx += 1;
        }
        let text: String = run[start..idx].iter().map(|c| c.ch).collect();
        push_gap(segments, text, marks);
    }
}

fn paragraph_to_segments(
    original: &str,
    translation: &ParagraphTranslationView,
    card_familiarity: &HashMap<String, f32>,
    src_lang: Language,
) -> Vec<ParagraphSegment> {
    let mut segments: Vec<ParagraphSegment> = Vec::new();

    let decode_lossy = |value: &str| -> String {
        decode(value.as_bytes())
            .to_string()
            .unwrap_or_else(|_| value.to_owned())
    };

    let original: Vec<char> = original.chars().collect();
    // Matching runs in decoded space, so a word's char count lines up with
    // the text it names. Entities and tags do not shift the count.
    let proj = flatten_original(&original);

    let mut p_idx = 0_usize;
    let mut sentence_idx = 0_usize;
    let mut flat_index = 0_usize;

    for sentence in translation.sentences() {
        let mut word_idx = 0;
        for word in sentence.words() {
            if word.is_punctuation {
                word_idx += 1;
                continue;
            }

            let current_flat_index = flat_index;
            flat_index += 1;

            let w_raw = word.original.replace("\n", "").replace("\r", "");
            let w = decode_lossy(&w_raw);
            let len = w.chars().count();
            let mut offset = 0_usize;
            while p_idx + offset < proj.len() {
                let start = p_idx + offset;
                let clamped_end = (start + len).min(proj.len());

                if start >= clamped_end {
                    break;
                }

                let p_word: String = proj[start..clamped_end].iter().map(|c| c.ch).collect();

                if w.len() <= 2 {
                    if w.to_lowercase() == p_word.to_lowercase() {
                        break;
                    }
                } else if levenshtein_distance_lt_2(&w.to_lowercase(), &p_word.to_lowercase()) {
                    break;
                }

                offset += 1;
            }

            let match_start = (p_idx + offset).min(proj.len());
            if match_start > p_idx {
                push_run(&mut segments, &proj[p_idx..match_start]);
            }

            p_idx = match_start;

            let clamped_end = (p_idx + len).min(proj.len());

            if p_idx < clamped_end {
                let text: String = proj[p_idx..clamped_end].iter().map(|c| c.ch).collect();
                // A word whose interior changes emphasis takes the marks of its
                // first character; one span per word cannot represent a split.
                let word_marks = proj[p_idx].marks.clone();
                let translation_text = word
                    .contextual_translations()
                    .next()
                    .map(|ct| sanitize_translation_text(ct.translation.as_ref()))
                    .filter(|t| !t.is_empty());

                let lemma_canonical =
                    card::canonicalize_lemma(&word.grammar.original_initial_form, src_lang);
                let familiarity = if lemma_canonical.is_empty() {
                    None
                } else {
                    let slug = card::lemma_slug(&lemma_canonical);
                    if slug.is_empty() {
                        None
                    } else {
                        // Missing = dormant card (Suspended/Deleted); never-synced
                        // maps to Some(0.0) in `LibraryCardStore::familiarities`.
                        card_familiarity.get(&slug).copied()
                    }
                };

                segments.push(ParagraphSegment::Word {
                    text,
                    marks: word_marks,
                    sentence: sentence_idx,
                    word: word_idx,
                    flat_index: current_flat_index,
                    translation: translation_text,
                    familiarity,
                });
            }

            p_idx = clamped_end;
            word_idx += 1;
        }

        sentence_idx += 1;
    }

    if p_idx < proj.len() {
        push_run(&mut segments, &proj[p_idx..]);
    }

    segments
}

fn sanitize_translation_text(value: &str) -> String {
    value
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `levenshtein_distance(str1, str2) < 2`, without computing the full distance.
fn levenshtein_distance_lt_2(str1: &str, str2: &str) -> bool {
    if str1 == str2 {
        return true;
    }

    let n = str1.chars().count();
    let m = str2.chars().count();

    if n.abs_diff(m) >= 2 {
        return false;
    }

    if n == 0 {
        return m == 1;
    }
    if m == 0 {
        return n == 1;
    }

    let a: Vec<char> = str1.chars().collect();
    let b: Vec<char> = str2.chars().collect();

    let mut i = 0;
    let mut j = 0;
    let mut edits = 0;

    while i < n && j < m {
        if a[i] != b[j] {
            if edits == 1 {
                return false;
            }
            edits += 1;

            if n > m && i + 1 < n && a[i + 1] == b[j] {
                i += 1; // deletion from a
            } else if m > n && j + 1 < m && a[i] == b[j + 1] {
                j += 1; // insertion into a
            } else {
                i += 1; // substitution
                j += 1;
            }
        } else {
            i += 1;
            j += 1;
        }
    }

    // Leftover chars in either string cost one more edit.
    if i < n || j < m {
        edits += 1;
    }

    edits < 2
}

#[cfg(test)]
mod tests {
    use super::{Mark, ParagraphSegment, paragraph_to_segments};

    use isolang::Language;
    use library::book::translation::ParagraphTranslationView;
    use library::book::translation_import;
    use std::collections::HashMap;

    fn grammar_stub(original: &str) -> translation_import::Grammar {
        translation_import::Grammar {
            original_initial_form: original.to_owned(),
            target_initial_form: original.to_owned(),
            part_of_speech: "stub".to_owned(),
            plurality: None,
            person: None,
            tense: None,
            case: None,
            other: None,
        }
    }

    fn word(
        original: &str,
        contextual_translations: &[&str],
        is_punctuation: bool,
    ) -> translation_import::Word {
        translation_import::Word {
            original: original.to_owned(),
            contextual_translations: contextual_translations
                .iter()
                .map(|s| s.to_string())
                .collect(),
            note: None,
            is_punctuation,
            grammar: grammar_stub(original),
        }
    }

    fn make_paragraph_translation(
        sentences: Vec<translation_import::Sentence>,
    ) -> translation_import::ParagraphTranslation {
        translation_import::ParagraphTranslation {
            timestamp: 0,
            sentences,
            total_tokens: None,
        }
    }

    fn view_from_import<'a>(
        translation: &'a mut library::book::translation::Translation,
        paragraph_index: usize,
        pt: &translation_import::ParagraphTranslation,
    ) -> ParagraphTranslationView<'a> {
        translation.add_paragraph_translation(paragraph_index, pt, "gpt-5.2");
        translation
            .paragraph_view(paragraph_index)
            .expect("paragraph view")
    }

    fn word_seg(
        text: &str,
        sentence: usize,
        word: usize,
        flat_index: usize,
        translation: Option<&str>,
    ) -> ParagraphSegment {
        ParagraphSegment::Word {
            text: text.to_owned(),
            marks: Vec::new(),
            sentence,
            word,
            flat_index,
            translation: translation.map(str::to_owned),
            familiarity: None,
        }
    }

    fn gap_seg(html: &str) -> ParagraphSegment {
        ParagraphSegment::Gap {
            text: html.to_owned(),
            marks: Vec::new(),
        }
    }

    fn marked_word_seg(
        text: &str,
        sentence: usize,
        word: usize,
        flat_index: usize,
        translation: Option<&str>,
        marks: Vec<Mark>,
    ) -> ParagraphSegment {
        ParagraphSegment::Word {
            text: text.to_owned(),
            marks,
            sentence,
            word,
            flat_index,
            translation: translation.map(str::to_owned),
            familiarity: None,
        }
    }

    #[test]
    fn bold_word_carries_strong_mark() {
        let original = "<b>Test</b>";

        let pt = make_paragraph_translation(vec![translation_import::Sentence {
            full_translation: "ignored".to_owned(),
            words: vec![word("Test", &["Prueba"], false)],
        }]);

        let mut t = library::book::translation::Translation::create("spa", "eng");
        let view = view_from_import(&mut t, 0, &pt);
        let segments = paragraph_to_segments(
            original,
            &view,
            &HashMap::new(),
            Language::from_639_3("spa").unwrap(),
        );

        assert_eq!(
            segments,
            vec![marked_word_seg(
                "Test",
                0,
                0,
                0,
                Some("Prueba"),
                vec![Mark::Strong]
            )]
        );
    }

    #[test]
    fn italic_word_carries_emphasis_mark() {
        let original = "<i>Another</i> one";

        let pt = make_paragraph_translation(vec![translation_import::Sentence {
            full_translation: "ignored".to_owned(),
            words: vec![
                word("Another", &["Otro"], false),
                word("one", &["uno"], false),
            ],
        }]);

        let mut t = library::book::translation::Translation::create("spa", "eng");
        let view = view_from_import(&mut t, 0, &pt);
        let segments = paragraph_to_segments(
            original,
            &view,
            &HashMap::new(),
            Language::from_639_3("spa").unwrap(),
        );

        assert_eq!(
            segments,
            vec![
                marked_word_seg("Another", 0, 0, 0, Some("Otro"), vec![Mark::Emphasis]),
                gap_seg(" "),
                word_seg("one", 0, 1, 1, Some("uno")),
            ]
        );
    }

    #[test]
    fn br_becomes_break_segment() {
        let original = "a<br>b";

        let pt = make_paragraph_translation(vec![translation_import::Sentence {
            full_translation: "ignored".to_owned(),
            words: vec![word("a", &["a"], false), word("b", &["b"], false)],
        }]);

        let mut t = library::book::translation::Translation::create("spa", "eng");
        let view = view_from_import(&mut t, 0, &pt);
        let segments = paragraph_to_segments(
            original,
            &view,
            &HashMap::new(),
            Language::from_639_3("spa").unwrap(),
        );

        assert_eq!(
            segments,
            vec![
                word_seg("a", 0, 0, 0, Some("a")),
                ParagraphSegment::Break { marks: Vec::new() },
                word_seg("b", 0, 1, 1, Some("b")),
            ]
        );
    }

    #[test]
    fn wraps_words_and_preserves_raw_translation() {
        let original = "Hello, world!";

        let pt = make_paragraph_translation(vec![translation_import::Sentence {
            full_translation: "ignored".to_owned(),
            words: vec![
                word("Hello", &["<b>hi</b>"], false),
                word("&comma;", &[], true),
                word("world", &["  planet  "], false),
                word("&excl;", &[], true),
            ],
        }]);

        let mut t = library::book::translation::Translation::create("deu", "eng");
        let view = view_from_import(&mut t, 0, &pt);
        let segments = paragraph_to_segments(
            original,
            &view,
            &HashMap::new(),
            Language::from_639_3("eng").unwrap(),
        );

        // Raw translation text (no backend escaping), whitespace normalized.
        assert_eq!(
            segments,
            vec![
                word_seg("Hello", 0, 0, 0, Some("<b>hi</b>")),
                gap_seg(", "),
                word_seg("world", 0, 2, 1, Some("planet")),
                gap_seg("!"),
            ]
        );
    }

    #[test]
    fn empty_contextual_translation_yields_none() {
        let original = "Just words";

        let pt = make_paragraph_translation(vec![translation_import::Sentence {
            full_translation: "ignored".to_owned(),
            words: vec![word("Just", &[], false), word("words", &[], false)],
        }]);

        let mut t = library::book::translation::Translation::create("deu", "eng");
        let view = view_from_import(&mut t, 0, &pt);
        let segments = paragraph_to_segments(
            original,
            &view,
            &HashMap::new(),
            Language::from_639_3("eng").unwrap(),
        );

        assert_eq!(
            segments,
            vec![
                word_seg("Just", 0, 0, 0, None),
                gap_seg(" "),
                word_seg("words", 0, 1, 1, None),
            ]
        );
    }

    #[test]
    fn decodes_html_entities_inside_gaps() {
        let original = "Tom &amp; Jerry";

        let pt = make_paragraph_translation(vec![translation_import::Sentence {
            full_translation: "ignored".to_owned(),
            words: vec![
                word("Tom", &["Tom"], false),
                word("&amp;", &[], true),
                word("Jerry", &["Jerry"], false),
            ],
        }]);

        let mut t = library::book::translation::Translation::create("deu", "eng");
        let view = view_from_import(&mut t, 0, &pt);
        let segments = paragraph_to_segments(
            original,
            &view,
            &HashMap::new(),
            Language::from_639_3("eng").unwrap(),
        );

        assert_eq!(
            segments,
            vec![
                word_seg("Tom", 0, 0, 0, Some("Tom")),
                gap_seg(" & "),
                word_seg("Jerry", 0, 2, 1, Some("Jerry")),
            ]
        );
    }

    #[test]
    fn handles_unicode_characters_safely() {
        let original = "naïve café";

        let pt = make_paragraph_translation(vec![translation_import::Sentence {
            full_translation: "ignored".to_owned(),
            words: vec![
                word("naïve", &["naive"], false),
                word("café", &["cafe"], false),
            ],
        }]);

        let mut t = library::book::translation::Translation::create("fra", "eng");
        let view = view_from_import(&mut t, 0, &pt);
        let segments = paragraph_to_segments(
            original,
            &view,
            &HashMap::new(),
            Language::from_639_3("eng").unwrap(),
        );

        assert_eq!(
            segments,
            vec![
                word_seg("naïve", 0, 0, 0, Some("naive")),
                gap_seg(" "),
                word_seg("café", 0, 1, 1, Some("cafe")),
            ]
        );
    }

    #[test]
    fn supports_multiple_sentences_with_distinct_sentence_indices() {
        let original = "Hello world. Bye world.";

        let pt = make_paragraph_translation(vec![
            translation_import::Sentence {
                full_translation: "ignored".to_owned(),
                words: vec![
                    word("Hello", &["hi"], false),
                    word("world", &["world"], false),
                    word("&period;", &[], true),
                ],
            },
            translation_import::Sentence {
                full_translation: "ignored".to_owned(),
                words: vec![
                    word("Bye", &["bye"], false),
                    word("world", &["world"], false),
                    word("&period;", &[], true),
                ],
            },
        ]);

        let mut t = library::book::translation::Translation::create("deu", "eng");
        let view = view_from_import(&mut t, 0, &pt);
        let segments = paragraph_to_segments(
            original,
            &view,
            &HashMap::new(),
            Language::from_639_3("eng").unwrap(),
        );

        assert_eq!(
            segments,
            vec![
                word_seg("Hello", 0, 0, 0, Some("hi")),
                gap_seg(" "),
                word_seg("world", 0, 1, 1, Some("world")),
                gap_seg(". "),
                word_seg("Bye", 1, 0, 2, Some("bye")),
                gap_seg(" "),
                word_seg("world", 1, 1, 3, Some("world")),
                gap_seg("."),
            ]
        );
    }

    #[test]
    fn invalid_entities_do_not_fail_hard() {
        let original = "A &bogus B";

        let pt = make_paragraph_translation(vec![translation_import::Sentence {
            full_translation: "ignored".to_owned(),
            words: vec![
                word("A", &["A"], false),
                // Unterminated entity, intentionally.
                word("&bogus", &["and"], false),
                word("B", &["B"], false),
            ],
        }]);

        let mut t = library::book::translation::Translation::create("deu", "eng");
        let view = view_from_import(&mut t, 0, &pt);
        let segments = paragraph_to_segments(
            original,
            &view,
            &HashMap::new(),
            Language::from_639_3("eng").unwrap(),
        );

        let texts: Vec<&str> = segments
            .iter()
            .filter_map(|s| match s {
                ParagraphSegment::Word { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"A"));
        assert!(texts.contains(&"B"));
    }

    #[test]
    fn punctuation_only_translation_returns_original_as_single_gap() {
        let original = "...";

        let pt = make_paragraph_translation(vec![translation_import::Sentence {
            full_translation: "ignored".to_owned(),
            words: vec![word("&period;", &[], true), word("&period;", &[], true)],
        }]);

        let mut t = library::book::translation::Translation::create("deu", "eng");
        let view = view_from_import(&mut t, 0, &pt);
        let segments = paragraph_to_segments(
            original,
            &view,
            &HashMap::new(),
            Language::from_639_3("eng").unwrap(),
        );

        assert_eq!(segments, vec![gap_seg("...")]);
    }

    #[test]
    fn paragraph_to_segments_threads_familiarity_from_map() {
        let original = "hola mundo";

        let pt = make_paragraph_translation(vec![translation_import::Sentence {
            full_translation: "ignored".to_owned(),
            words: vec![
                word("hola", &["hi"], false),
                word("mundo", &["world"], false),
            ],
        }]);

        let mut t = library::book::translation::Translation::create("spa", "eng");
        let view = view_from_import(&mut t, 0, &pt);

        let mut fam = HashMap::new();
        fam.insert("hola".to_string(), 0.5_f32);

        let segments =
            paragraph_to_segments(original, &view, &fam, Language::from_639_3("spa").unwrap());

        let familiarities: Vec<Option<f32>> = segments
            .iter()
            .filter_map(|s| match s {
                ParagraphSegment::Word { familiarity, .. } => Some(*familiarity),
                _ => None,
            })
            .collect();

        assert_eq!(familiarities, vec![Some(0.5), None]);
    }

    /// Every character of the paragraph must survive segmentation exactly once:
    /// decoded gaps plus word texts reconstruct the decoded original. This is
    /// what lets the mounted and virtualized renderings show the same text.
    #[test]
    fn segments_reconstruct_the_paragraph() {
        let corpus: Vec<(&str, Vec<&str>)> = vec![
            ("Hello, world!", vec!["Hello", "world"]),
            ("Tom &amp; Jerry", vec!["Tom", "Jerry"]),
            ("caf&eacute; noir", vec!["café", "noir"]),
            ("<b>Test</b>", vec!["Test"]),
            ("naïve café", vec!["naïve", "café"]),
            ("a &hellip; b", vec!["a", "b"]),
            ("x&amp;y z", vec!["x&y", "z"]),
            // Word longer than what is left: the matcher must consume the tail
            // as a gap without rewinding and re-emitting it.
            ("ab", vec!["abcdef"]),
            // No word matches anything.
            ("zzz qqq", vec!["alpha", "beta"]),
            ("", vec!["ghost"]),
        ];

        for (original, words) in corpus {
            let pt = make_paragraph_translation(vec![translation_import::Sentence {
                full_translation: "ignored".to_owned(),
                words: words.iter().map(|w| word(w, &[], false)).collect(),
            }]);
            let mut t = library::book::translation::Translation::create("fra", "eng");
            let view = view_from_import(&mut t, 0, &pt);
            let segments = paragraph_to_segments(
                original,
                &view,
                &HashMap::new(),
                Language::from_639_3("fra").unwrap(),
            );

            let chars: Vec<char> = original.chars().collect();
            let projection: String = super::flatten_original(&chars)
                .iter()
                .map(|c| c.ch)
                .collect();

            let rebuilt: String = segments
                .iter()
                .map(|s| match s {
                    ParagraphSegment::Gap { text, .. } => text.clone(),
                    ParagraphSegment::Break { .. } => "\n".to_owned(),
                    ParagraphSegment::Word { text, .. } => text.clone(),
                })
                .collect();

            assert_eq!(rebuilt, projection, "reconstructing {original:?}");
        }
    }

    #[test]
    fn entity_inside_word_stays_one_word() {
        let original = "caf&eacute; noir";

        let pt = make_paragraph_translation(vec![translation_import::Sentence {
            full_translation: "ignored".to_owned(),
            words: vec![
                word("café", &["coffee"], false),
                word("noir", &["black"], false),
            ],
        }]);

        let mut t = library::book::translation::Translation::create("fra", "eng");
        let view = view_from_import(&mut t, 0, &pt);
        let segments = paragraph_to_segments(
            original,
            &view,
            &HashMap::new(),
            Language::from_639_3("fra").unwrap(),
        );

        assert_eq!(
            segments,
            vec![
                word_seg("café", 0, 0, 0, Some("coffee")),
                gap_seg(" "),
                word_seg("noir", 0, 1, 1, Some("black")),
            ]
        );
    }
}
