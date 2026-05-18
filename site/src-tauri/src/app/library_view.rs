use std::sync::Arc;

use ahash::AHashSet;
use htmlentity::entity::{ICodedDataTrait, decode};
use isolang::Language;
use tauri::Emitter;
use library::epub_importer::EpubBook;
use library::library::file_watcher::LibraryFileChange;
use library::{
    book::translation::ParagraphTranslationView,
    library::{Library, library_book::BookReadingState},
};
use uuid::Uuid;

use crate::app::AppState;

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
}

#[derive(Clone, serde::Serialize)]
pub struct ParagraphView {
    id: usize,
    original: String,
    segments: Option<Vec<ParagraphSegment>>,
    #[serde(rename = "visibleWords")]
    visible_words: AHashSet<usize>,
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
    #[serde(rename = "visibleWords")]
    visible_words: AHashSet<usize>,
}

#[derive(Clone, serde::Serialize, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ParagraphSegment {
    Gap {
        html: String,
    },
    Word {
        text: String,
        sentence: usize,
        word: usize,
        #[serde(rename = "flatIndex")]
        flat_index: usize,
        translation: Option<String>,
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
    translation_model: usize,
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
        let mut book = book.lock().await;

        let book_translation = book.get_or_create_translation(target_language).await;

        let paragraph = book.book.paragraph_view(paragraph_id);
        let original = paragraph.original_html.unwrap_or(paragraph.original_text);

        let bt = book_translation.lock().await;
        let t_view = bt.paragraph_view(paragraph_id);
        let segments = t_view
            .as_ref()
            .map(|t| paragraph_to_segments(&original, t));
        let visible_words = t_view
            .as_ref()
            .map(|t| t.visible_words().clone())
            .unwrap_or_default();

        Ok(ParagraphView {
            id: paragraph_id,
            original: original.to_string(),
            segments,
            visible_words,
        })
    }

    pub async fn get_paragraph_originals_batch(
        &self,
        book_id: Uuid,
        paragraph_ids: Vec<usize>,
    ) -> anyhow::Result<Vec<ParagraphOriginal>> {
        let book = self.library.get_book(&book_id).await?;
        let book = book.lock().await;
        Ok(paragraph_ids
            .into_iter()
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
        let mut book = book.lock().await;

        let book_translation = book.get_or_create_translation(target_language).await;
        let bt = book_translation.lock().await;

        Ok(paragraph_ids
            .into_iter()
            .map(|id| {
                let p = book.book.paragraph_view(id);
                let original = p.original_html.unwrap_or(p.original_text);
                let t_view = bt.paragraph_view(id);
                let segments = t_view
                    .as_ref()
                    .map(|t| paragraph_to_segments(&original, t));
                let visible_words = t_view
                    .as_ref()
                    .map(|t| t.visible_words().clone())
                    .unwrap_or_default();
                ParagraphTranslationSlice {
                    id,
                    segments,
                    visible_words,
                }
            })
            .collect())
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
                    .map(|t| t.translated_paragraphs_count as f64 / b.paragraphs_count as f64)
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

    pub async fn list_book_chapters(&mut self, book_id: Uuid) -> anyhow::Result<Vec<ChapterView>> {
        let book = self.library.get_book(&book_id).await?;
        let book = book.lock().await;
        let book = &book.book;
        let chapters = book
            .chapter_views()
            .map(|v| ChapterView {
                id: v.idx,
                title: v
                    .title
                    .map(|s| s.to_string())
                    .unwrap_or("<no title>".to_owned()),
            })
            .collect();
        Ok(chapters)
    }

    pub async fn list_book_chapter_paragraph_ids(
        &self,
        book_id: Uuid,
        chapter_id: usize,
    ) -> anyhow::Result<Vec<usize>> {
        let book = self.library.get_book(&book_id).await?;
        let book = book.lock().await;
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
            let mut book = book.lock().await;
            (
                book.get_or_create_translation(target_language).await,
                book.book.language.clone(),
            )
        };

        Ok(
            if let Some(paragraph) = book_translation.lock().await.paragraph_view(paragraph_id) {
                let sentence = paragraph.sentence_view(sentence_id);
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
                    translation_model: paragraph.model as usize,
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

        Ok(id)
    }

    pub async fn import_epub(
        &mut self,
        book: &EpubBook,
        source_language: &Language,
    ) -> anyhow::Result<Uuid> {
        let id = self.library.create_book_epub(book, source_language).await?;

        self.state.notify_library_changed();

        Ok(id)
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

    pub async fn mark_word_visible(
        &self,
        book_id: Uuid,
        paragraph_id: usize,
        flat_index: usize,
        target_language: &Language,
    ) -> anyhow::Result<bool> {
        let book = self.library.get_book(&book_id).await?;
        let mut book = book.lock().await;
        let book_translation = book.get_or_create_translation(target_language).await;

        let result = {
            let mut bt = book_translation.lock().await;
            bt.mark_word_visible(paragraph_id, flat_index)
        };

        // Persist to disk
        if result {
            book.save().await?;
        }

        Ok(result)
    }

    pub async fn handle_file_change_event(
        &mut self,
        event: &LibraryFileChange,
    ) -> anyhow::Result<bool> {
        self.library.handle_file_change_event(event).await
    }
}

#[tauri::command]
pub async fn list_books(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Vec<LibraryBookMetadataView>, String> {
    let target_language_id = { state.config.borrow().target_language_id.clone() };
    let target_language = Language::from_639_3(&target_language_id);
    let library = state.library.borrow().clone();

    let Some(library) = library else {
        return Ok(vec![]);
    };

    LibraryView::create(state.inner().clone(), library)
        .list_books(target_language.as_ref())
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn list_book_chapters(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
) -> Result<Vec<ChapterView>, String> {
    let library = state.library.borrow().clone();
    let Some(library) = library else {
        return Ok(vec![]);
    };

    let mut library_view = LibraryView::create(state.inner().clone(), library);
    library_view
        .list_book_chapters(book_id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_book_chapter_paragraph_ids(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    chapter_id: usize,
) -> Result<Vec<usize>, String> {
    let library = state.library.borrow().clone();
    let Some(library) = library else {
        return Ok(vec![]);
    };

    LibraryView::create(state.inner().clone(), library)
        .list_book_chapter_paragraph_ids(book_id, chapter_id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_word_info(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    paragraph_id: usize,
    sentence_id: usize,
    word_id: usize,
) -> Result<Option<WordView>, String> {
    let library = state.library.borrow().clone();
    let Some(library) = library else {
        return Ok(None);
    };

    let target_language_id = { state.config.borrow().target_language_id.clone() };
    let Some(target_language) = Language::from_639_3(&target_language_id) else {
        return Ok(None);
    };

    let mut library_view = LibraryView::create(state.inner().clone(), library);
    library_view
        .get_word_info(
            book_id,
            paragraph_id,
            sentence_id,
            word_id,
            &target_language,
        )
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_paragraph_view(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    paragraph_id: usize,
) -> Result<ParagraphView, String> {
    let library = state.library.borrow().clone();
    let Some(library) = library else {
        return Err("Library is not configured".into());
    };

    let target_language_id = { state.config.borrow().target_language_id.clone() };
    let Some(target_language) = Language::from_639_3(&target_language_id) else {
        return Err("Library is not configured".into());
    };

    let library_view = LibraryView::create(state.inner().clone(), library);
    library_view
        .get_paragraph_view(book_id, paragraph_id, &target_language)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_paragraph_originals_batch(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    paragraph_ids: Vec<usize>,
) -> Result<Vec<ParagraphOriginal>, String> {
    let library = state.library.borrow().clone();
    let Some(library) = library else {
        return Err("Library is not configured".into());
    };

    let library_view = LibraryView::create(state.inner().clone(), library);
    library_view
        .get_paragraph_originals_batch(book_id, paragraph_ids)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_paragraph_translations_batch(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    paragraph_ids: Vec<usize>,
) -> Result<Vec<ParagraphTranslationSlice>, String> {
    let library = state.library.borrow().clone();
    let Some(library) = library else {
        return Err("Library is not configured".into());
    };

    let target_language_id = { state.config.borrow().target_language_id.clone() };
    let Some(target_language) = Language::from_639_3(&target_language_id) else {
        return Err("Library is not configured".into());
    };

    let library_view = LibraryView::create(state.inner().clone(), library);
    library_view
        .get_paragraph_translations_batch(book_id, paragraph_ids, &target_language)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn import_plain_text(
    state: tauri::State<'_, Arc<AppState>>,
    title: String,
    text: String,
    source_language_id: String,
) -> Result<Uuid, String> {
    let library = state
        .library
        .borrow()
        .clone()
        .ok_or("Library is not configured")?;

    let source_language = Language::from_639_3(&source_language_id)
        .ok_or_else(|| format!("Failed to resolve source language: {}", source_language_id))?;

    let mut library_view = LibraryView::create(state.inner().clone(), library);
    library_view
        .import_plain_text(&title, &text, &source_language)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn import_epub(
    state: tauri::State<'_, Arc<AppState>>,
    book: EpubBook,
    source_language_id: String,
) -> Result<Uuid, String> {
    let library = state
        .library
        .borrow()
        .clone()
        .ok_or("Library is not configured")?;

    let source_language = Language::from_639_3(&source_language_id)
        .ok_or_else(|| format!("Failed to resolve source language: {}", source_language_id))?;

    let mut library_view = LibraryView::create(state.inner().clone(), library);
    library_view
        .import_epub(&book, &source_language)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_book_reading_state(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
) -> Result<Option<BookReadingStateView>, String> {
    let library = state.library.borrow().clone();
    let Some(library) = library else {
        return Ok(None);
    };

    LibraryView::create(state.inner().clone(), library)
        .get_book_reading_state(book_id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn save_book_reading_state(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    chapter_id: usize,
    paragraph_id: usize,
    page_offset: usize,
) -> Result<(), String> {
    let library = state
        .library
        .borrow()
        .clone()
        .ok_or("Library is not configured")?;

    LibraryView::create(state.inner().clone(), library)
        .save_book_reading_state(book_id, chapter_id, paragraph_id, page_offset)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn move_book(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    path: Vec<String>,
) -> Result<(), String> {
    let library = state
        .library
        .borrow()
        .clone()
        .ok_or("Library is not configured")?;

    LibraryView::create(state.inner().clone(), library)
        .move_book(book_id, path)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn delete_book(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
) -> Result<(), String> {
    let library = state
        .library
        .borrow()
        .clone()
        .ok_or("Library is not configured")?;

    LibraryView::create(state.inner().clone(), library)
        .delete_book(book_id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn mark_word_visible(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    paragraph_id: usize,
    flat_index: usize,
) -> Result<bool, String> {
    let library = state
        .library
        .borrow()
        .clone()
        .ok_or("Library is not configured")?;

    let target_language_id = { state.config.borrow().target_language_id.clone() };
    let Some(target_language) = Language::from_639_3(&target_language_id) else {
        return Err("Library is not configured".into());
    };

    let changed = LibraryView::create(state.inner().clone(), library)
        .mark_word_visible(book_id, paragraph_id, flat_index, &target_language)
        .await
        .map_err(|err| err.to_string())?;

    if changed {
        // Notify the frontend so the paragraph-view Resource re-fetches and
        // the just-marked overlay sticks once the user deselects.
        let _ = app.emit(
            "paragraph_updated",
            serde_json::json!({
                "bookId": book_id,
                "paragraphId": paragraph_id,
            }),
        );
    }

    Ok(changed)
}

fn paragraph_to_segments(
    original: &str,
    translation: &ParagraphTranslationView,
) -> Vec<ParagraphSegment> {
    let mut segments: Vec<ParagraphSegment> = Vec::new();

    let push_gap = |segments: &mut Vec<ParagraphSegment>, html: String| {
        if html.is_empty() {
            return;
        }
        if let Some(ParagraphSegment::Gap { html: existing }) = segments.last_mut() {
            existing.push_str(&html);
        } else {
            segments.push(ParagraphSegment::Gap { html });
        }
    };

    let decode_lossy = |value: &str| -> String {
        decode(value.as_bytes())
            .to_string()
            .unwrap_or_else(|_| value.to_owned())
    };

    let original: Vec<char> = original.chars().collect();

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
            while p_idx + offset < original.len() {
                let start = p_idx + offset;
                let mut clamped_end = p_idx + offset + len;
                if clamped_end >= original.len() {
                    clamped_end = original.len();
                }

                if start >= clamped_end {
                    break;
                }

                let p_word_raw = String::from_iter(original[start..clamped_end].iter());
                let p_word = decode_lossy(&p_word_raw);

                if w.len() <= 2 {
                    if w.to_lowercase() == p_word.to_lowercase() {
                        break;
                    }
                } else if levenshtein_distance_lt_2(&w.to_lowercase(), &p_word.to_lowercase()) {
                    break;
                }

                offset += 1;
            }

            if offset > 0 {
                let end = (p_idx + offset).min(original.len());
                let gap = String::from_iter(original[p_idx..end].iter());
                push_gap(&mut segments, gap);
            }

            p_idx += offset;

            let mut clamped_end = p_idx + len;
            if clamped_end >= original.len() {
                clamped_end = original.len();
            }

            if p_idx < clamped_end {
                let text = String::from_iter(original[p_idx..clamped_end].iter());
                let translation_text = word
                    .contextual_translations()
                    .next()
                    .map(|ct| sanitize_translation_text(ct.translation.as_ref()))
                    .filter(|t| !t.is_empty());

                segments.push(ParagraphSegment::Word {
                    text,
                    sentence: sentence_idx,
                    word: word_idx,
                    flat_index: current_flat_index,
                    translation: translation_text,
                });
            }

            p_idx = clamped_end;
            word_idx += 1;
        }

        sentence_idx += 1;
    }

    if p_idx < original.len() {
        let gap = String::from_iter(original[p_idx..(original.len())].iter());
        push_gap(&mut segments, gap);
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

/// Optimized check for levenshtein_distance(s1, s2) < 2.
/// This is faster than computing the full distance for this specific threshold.
fn levenshtein_distance_lt_2(str1: &str, str2: &str) -> bool {
    if str1 == str2 {
        return true;
    }

    let n = str1.chars().count();
    let m = str2.chars().count();

    // If length difference >= 2, distance must be >= 2
    if n.abs_diff(m) >= 2 {
        return false;
    }

    // For distance < 2, we only need to check for 0 or 1 edits
    // Distance 0: already handled by equality check above
    // Distance 1: strings differ by exactly one insertion, deletion, or substitution

    if n == 0 {
        return m == 1;
    }
    if m == 0 {
        return n == 1;
    }

    let a: Vec<char> = str1.chars().collect();
    let b: Vec<char> = str2.chars().collect();

    // Count mismatches - for distance < 2, we can have at most 1 edit
    let mut i = 0;
    let mut j = 0;
    let mut edits = 0;

    while i < n && j < m {
        if a[i] != b[j] {
            if edits == 1 {
                return false; // Already had one edit, this is the second
            }
            edits += 1;

            // Determine if this is insertion, deletion, or substitution
            if n > m && i + 1 < n && a[i + 1] == b[j] {
                // Deletion from a (skip a[i])
                i += 1;
            } else if m > n && j + 1 < m && a[i] == b[j + 1] {
                // Insertion into a (skip b[j])
                j += 1;
            } else {
                // Substitution (advance both)
                i += 1;
                j += 1;
            }
        } else {
            i += 1;
            j += 1;
        }
    }

    // If we've consumed all chars from both, check edit count
    // If one string has remaining chars, that's another edit
    if i < n || j < m {
        edits += 1;
    }

    edits < 2
}

#[cfg(test)]
mod tests {
    use super::{ParagraphSegment, paragraph_to_segments};

    use library::book::translation_import;
    use library::dictionary::Dictionary;
    use library::{book::translation::ParagraphTranslationView, translator::TranslationModel};

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
            source_language: "deu".to_owned(),
            target_language: "eng".to_owned(),
            total_tokens: None,
        }
    }

    fn view_from_import<'a>(
        translation: &'a mut library::book::translation::Translation,
        paragraph_index: usize,
        pt: &translation_import::ParagraphTranslation,
    ) -> ParagraphTranslationView<'a> {
        let mut dictionary =
            Dictionary::create(pt.source_language.clone(), pt.target_language.clone());
        translation.add_paragraph_translation(
            paragraph_index,
            pt,
            TranslationModel::OpenAIGpt52,
            &mut dictionary,
        );
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
            sentence,
            word,
            flat_index,
            translation: translation.map(str::to_owned),
        }
    }

    fn gap_seg(html: &str) -> ParagraphSegment {
        ParagraphSegment::Gap {
            html: html.to_owned(),
        }
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
        let segments = paragraph_to_segments(original, &view);

        // Translation text is preserved raw (no HTML escaping on the backend);
        // whitespace is still normalized via sanitize_translation_text.
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
        let segments = paragraph_to_segments(original, &view);

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
    fn preserves_original_html_entities_inside_gaps() {
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
        let segments = paragraph_to_segments(original, &view);

        // The &amp; entity is carried verbatim inside a gap segment between the two words.
        assert_eq!(
            segments,
            vec![
                word_seg("Tom", 0, 0, 0, Some("Tom")),
                gap_seg(" &amp; "),
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
        let segments = paragraph_to_segments(original, &view);

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
        let segments = paragraph_to_segments(original, &view);

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
                // This is intentionally invalid / unterminated entity-like text.
                word("&bogus", &["and"], false),
                word("B", &["B"], false),
            ],
        }]);

        let mut t = library::book::translation::Translation::create("deu", "eng");
        let view = view_from_import(&mut t, 0, &pt);
        let segments = paragraph_to_segments(original, &view);

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
        let segments = paragraph_to_segments(original, &view);

        assert_eq!(segments, vec![gap_seg("...")]);
    }
}
