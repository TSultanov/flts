use std::sync::Arc;

use htmlentity::entity::{ICodedDataTrait, decode};
use isolang::Language;
use library::library::file_watcher::LibraryFileChange;
use library::{book::translation::ParagraphTranslationView, library::Library};
use library::epub_importer::EpubBook;
use tauri::async_runtime::Mutex;
use tauri::Emitter;
use uuid::Uuid;

use crate::app::App;

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
    translation: Option<String>,
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
    app: tauri::AppHandle,
    library: Library,
}

impl LibraryView {
    pub fn create(app: tauri::AppHandle, library: Library) -> Self {
        Self { app, library }
    }

    pub fn list_books(
        &self,
        target_language: Option<&Language>,
    ) -> anyhow::Result<Vec<LibraryBookMetadataView>> {
        let books = self.library.list_books()?;
        Ok(books
            .into_iter()
            .map(|b| {
                let translation = target_language.and_then(|tl| b
                    .translations_metadata
                    .iter()
                    .find(|t| t.target_language == tl.to_639_3()));

                let translation_ratio = translation
                    .map(|t| t.translated_paragraphs_count as f64 / b.paragraphs_count as f64)
                    .unwrap_or(0.0);

                LibraryBookMetadataView {
                    id: b.id,
                    title: b.title,
                    chapters_count: b.chapters_count,
                    paragraphs_count: b.paragraphs_count,
                    translation_ratio,
                }
            })
            .collect())
    }

    pub fn list_book_chapters(&mut self, book_id: Uuid) -> anyhow::Result<Vec<ChapterView>> {
        let book = self.library.get_book(&book_id)?;
        let book = book.blocking_lock();
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

    pub async fn list_book_chapter_paragraphs(
        &mut self,
        book_id: Uuid,
        chapter_id: usize,
        target_language: &Language,
    ) -> anyhow::Result<Vec<ParagraphView>> {
        let book = self.library.get_book(&book_id)?;
        let mut book = book.lock().await;

        let book_translation = book.get_or_create_translation(target_language).await;

        let mut views = Vec::new();
        for p in book.book.chapter_view(chapter_id).paragraphs() {
            let original = p.original_html.unwrap_or(p.original_text);

            let bt = book_translation.lock().await;
            let t_view = bt.paragraph_view(p.id);
            let translation = t_view.map(|t| {
                translation_to_html(p.id, &original, &t).unwrap_or_else(|err| err.to_string())
            });

            views.push(ParagraphView {
                id: p.id,
                original: original.to_string(),
                translation,
            });
        }

        Ok(views)
    }

    pub async fn get_word_info(
        &mut self,
        book_id: Uuid,
        paragraph_id: usize,
        sentence_id: usize,
        word_id: usize,
        target_language: &Language,
    ) -> anyhow::Result<Option<WordView>> {
        let book = self.library.get_book(&book_id)?;
        let mut book = book.lock().await;

        let book_translation = book.get_or_create_translation(target_language).await;

        Ok(
            if let Some(paragraph) = book_translation.lock().await.paragraph_view(paragraph_id)
            {
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
        target_language: Option<&Language>,
    ) -> anyhow::Result<Uuid> {
        let id = self
            .library
            .create_book_plain(title, text, source_language)
            .await?;

        // Emit updated library view after successful import
        let books = self.list_books(target_language)?;
        self.app.emit("library_updated", books)?;

        Ok(id)
    }

    pub async fn import_epub(
        &mut self,
        book: &EpubBook,
        source_language: &Language,
        target_language: Option<&Language>,
    ) -> anyhow::Result<Uuid> {
        let id = self
            .library
            .create_book_epub(book, source_language)
            .await?;

        // Emit updated library view after successful import
        let books = self.list_books(target_language)?;
        self.app.emit("library_updated", books)?;

        Ok(id)
    }

    pub async fn handle_file_change_event(&mut self, event: &LibraryFileChange) -> anyhow::Result<()> {
        self.library.handle_file_change_event(event).await
    }
}

#[tauri::command]
pub async fn list_books(
    state: tauri::State<'_, Arc<Mutex<App>>>,
) -> Result<Vec<LibraryBookMetadataView>, String> {
    let app = state.lock().await;

    let target_language = app
        .config
        .target_language_id
        .as_ref()
        .and_then(|l| Language::from_639_3(l));

    if let Some(library) = &app.library {
        library
            .list_books(target_language.as_ref())
            .map_err(|err| err.to_string())
    } else {
        Ok(vec![])
    }
}

#[tauri::command]
pub fn list_book_chapters(
    state: tauri::State<'_, Arc<Mutex<App>>>,
    book_id: Uuid,
) -> Result<Vec<ChapterView>, String> {
    let mut app = state.blocking_lock();
    if let Some(library) = &mut app.library {
        library
            .list_book_chapters(book_id)
            .map_err(|err| err.to_string())
    } else {
        Ok(vec![])
    }
}

#[tauri::command]
pub async fn get_book_chapter_paragraphs(
    state: tauri::State<'_, Arc<Mutex<App>>>,
    book_id: Uuid,
    chapter_id: usize,
) -> Result<Vec<ParagraphView>, String> {
    let mut app = state.lock().await;

    let target_language = app
        .config
        .target_language_id
        .as_ref()
        .and_then(|l| Language::from_639_3(l));

    if let Some(library) = &mut app.library
        && let Some(target_language) = target_language
    {
        library
            .list_book_chapter_paragraphs(book_id, chapter_id, &target_language)
            .await
            .map_err(|err| err.to_string())
    } else {
        Ok(vec![])
    }
}

#[tauri::command]
pub async fn get_word_info(
    state: tauri::State<'_, Arc<Mutex<App>>>,
    book_id: Uuid,
    paragraph_id: usize,
    sentence_id: usize,
    word_id: usize,
) -> Result<Option<WordView>, String> {
    let mut app = state.lock().await;

    let target_language = app
        .config
        .target_language_id
        .as_ref()
        .and_then(|l| Language::from_639_3(l));

    if let Some(library) = &mut app.library
        && let Some(target_language) = target_language
    {
        library
            .get_word_info(
                book_id,
                paragraph_id,
                sentence_id,
                word_id,
                &target_language,
            )
            .await
            .map_err(|err| err.to_string())
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub async fn import_plain_text(
    state: tauri::State<'_, Arc<Mutex<App>>>,
    title: String,
    text: String,
    source_language_id: String,
) -> Result<Uuid, String> {
    let mut app = state.lock().await;

    let target_language = app
        .config
        .target_language_id
        .as_ref()
        .and_then(|l| Language::from_639_3(l));

    if let Some(library) = &mut app.library {
        let source_language = Language::from_639_3(&source_language_id)
            .ok_or_else(|| format!("Failed to resolve source language: {}", source_language_id))?;
        let id = library
            .import_plain_text(&title, &text, &source_language, target_language.as_ref())
            .await
            .map_err(|err| err.to_string())?;

        Ok(id)
    } else {
        Err("Library is not configured".into())
    }
}

#[tauri::command]
pub async fn import_epub(
    state: tauri::State<'_, Arc<Mutex<App>>>,
    book: EpubBook,
    source_language_id: String,
) -> Result<Uuid, String> {
    let mut app = state.lock().await;

    // Pre-compute target language for later emit while avoiding borrow conflicts
    let target_language = app
        .config
        .target_language_id
        .as_ref()
        .and_then(|l| Language::from_639_3(l));

    if let Some(library) = &mut app.library {
        let source_language = Language::from_639_3(&source_language_id)
            .ok_or_else(|| format!("Failed to resolve source language: {}", source_language_id))?;
        let id = library
            .import_epub(&book, &source_language, target_language.as_ref())
            .await
            .map_err(|err| err.to_string())?;

        Ok(id)
    } else {
        Err("Library is not configured".into())
    }
}

fn translation_to_html(
    paragraph_id: usize,
    original: &str,
    translation: &ParagraphTranslationView,
) -> anyhow::Result<String> {
    let mut result = Vec::new();

    let original: Vec<char> = original.chars().collect();

    let mut p_idx = 0_usize;
    let mut sentence_idx = 0_usize;
    for sentence in translation.sentences() {
        let mut word_idx = 0;
        for word in sentence.words() {
            if word.is_punctuation {
                word_idx += 1;
                continue;
            }

            let w =
                decode(word.original.replace("\n", "").replace("\r", "").as_bytes()).to_string()?;
            let len = w.chars().count();
            let mut offset = 0_usize;
            while offset < original.len() {
                let start = p_idx + offset;
                let mut clamped_end = p_idx + offset + len;
                if clamped_end >= original.len() {
                    clamped_end = original.len();
                }

                if start >= clamped_end {
                    break;
                }

                let p_word =
                    decode(String::from_iter(original[start..clamped_end].iter()).as_bytes())
                        .to_string()?;

                if w.len() <= 2 {
                    if w.to_lowercase() == p_word.to_lowercase() {
                        break;
                    }
                } else if levenshtein_distance(&w.to_lowercase(), &p_word.to_lowercase()) < 2 {
                    break;
                }

                offset += 1;
            }

            if offset > 0 {
                let text = String::from_iter(original[p_idx..(p_idx + offset)].iter());
                result.push(text);
            }

            p_idx += offset;

            let mut clamped_end = p_idx + len;
            if clamped_end >= original.len() {
                clamped_end = original.len();
            }

            if p_idx < clamped_end {
                let text = String::from_iter(original[p_idx..clamped_end].iter());
                result.push(format!("<span class=\"word-span\" data-paragraph=\"{paragraph_id}\" data-sentence=\"{sentence_idx}\" data-word=\"{word_idx}\">{text}</span>"));
            }

            p_idx += len;
            word_idx += 1;
        }

        sentence_idx += 1;
    }

    if p_idx < original.len() {
        let text = String::from_iter(original[p_idx..(original.len())].iter());
        result.push(text);
    }

    Ok(result.join(""))
}

fn levenshtein_distance(str1: &str, str2: &str) -> usize {
    if str1 == str2 {
        return 0;
    }

    let a: Vec<char> = str1.chars().collect();
    let b: Vec<char> = str2.chars().collect();

    let n = a.len();
    let m = b.len();

    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }

    let mut previous: Vec<usize> = (0..=m).collect();
    let mut current: Vec<usize> = vec![0; m + 1];

    for i in 1..=n {
        current[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            let deletion = previous[j] + 1; // delete from a
            let insertion = current[j - 1] + 1; // insert into a
            let substitution = previous[j - 1] + cost;
            current[j] = deletion.min(insertion).min(substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[m]
}
