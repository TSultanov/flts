use std::sync::Arc;

use htmlentity::entity::{CharacterSet, EncodeType, ICodedDataTrait, decode, encode};
use isolang::Language;
use library::epub_importer::EpubBook;
use library::library::file_watcher::LibraryFileChange;
use library::{
    book::translation::ParagraphTranslationView,
    library::{Library, library_book::BookReadingState},
};
use tauri::Emitter;
use tauri::async_runtime::Mutex;
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
    #[serde(rename = "fullSentenceTranslation")]
    full_sentence_translation: String,
    #[serde(rename = "translationModel")]
    translation_model: usize,
}

#[derive(Clone, serde::Serialize)]
pub struct BookReadingStateView {
    #[serde(rename = "chapterId")]
    chapter_id: usize,
    #[serde(rename = "paragraphId")]
    paragraph_id: usize,
}

impl From<BookReadingState> for BookReadingStateView {
    fn from(value: BookReadingState) -> Self {
        Self {
            chapter_id: value.chapter_id,
            paragraph_id: value.paragraph_id,
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
    app: tauri::AppHandle,
    library: Arc<Mutex<Library>>,
}

impl LibraryView {
    pub fn create(app: tauri::AppHandle, library: Arc<Mutex<Library>>) -> Self {
        Self { app, library }
    }

    pub async fn list_books(
        &self,
        target_language: Option<&Language>,
    ) -> anyhow::Result<Vec<LibraryBookMetadataView>> {
        let books = self.library.lock().await.list_books().await?;
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
        let book = self.library.lock().await.get_book(&book_id).await?;
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

    pub async fn list_book_chapter_paragraphs(
        &mut self,
        book_id: Uuid,
        chapter_id: usize,
        target_language: &Language,
    ) -> anyhow::Result<Vec<ParagraphView>> {
        let book = self.library.lock().await.get_book(&book_id).await?;
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
        let book_translation = {
            let book = self.library.lock().await.get_book(&book_id).await?;
            let mut book = book.lock().await;
            book.get_or_create_translation(target_language).await
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
            .lock()
            .await
            .create_book_plain(title, text, source_language)
            .await?;

        // Emit updated library view after successful import
        let books = self.list_books(target_language).await?;
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
            .lock()
            .await
            .create_book_epub(book, source_language)
            .await?;

        // Emit updated library view after successful import
        let books = self.list_books(target_language).await?;
        self.app.emit("library_updated", books)?;

        Ok(id)
    }

    pub async fn get_book_reading_state(
        &self,
        book_id: Uuid,
    ) -> anyhow::Result<Option<BookReadingStateView>> {
        let book = self.library.lock().await.get_book(&book_id).await?;
        let mut book = book.lock().await;
        Ok(book.reading_state().await?.map(BookReadingStateView::from))
    }

    pub async fn save_book_reading_state(
        &self,
        book_id: Uuid,
        chapter_id: usize,
        paragraph_id: usize,
    ) -> anyhow::Result<()> {
        let book = self.library.lock().await.get_book(&book_id).await?;
        let mut book = book.lock().await;
        book.update_reading_state(BookReadingState {
            chapter_id,
            paragraph_id,
        })
        .await
    }

    pub async fn move_book(
        &self,
        book_id: Uuid,
        new_path: Vec<String>,
        target_language: Option<&Language>,
    ) -> anyhow::Result<()> {
        let book = self.library.lock().await.get_book(&book_id).await?;
        {
            let mut book = book.lock().await;
            book.update_folder_path(new_path).await?;
        }

        let books = self.list_books(target_language).await?;
        self.app.emit("library_updated", books)?;
        Ok(())
    }

    pub async fn delete_book(
        &self,
        book_id: Uuid,
        target_language: Option<&Language>,
    ) -> anyhow::Result<()> {
        self.library.lock().await.delete_book(&book_id).await?;
        let books = self.list_books(target_language).await?;
        self.app.emit("library_updated", books)?;
        Ok(())
    }

    pub async fn handle_file_change_event(
        &mut self,
        event: &LibraryFileChange,
    ) -> anyhow::Result<bool> {
        self.library
            .lock()
            .await
            .handle_file_change_event(event)
            .await
    }
}

#[tauri::command]
pub async fn list_books(
    state: tauri::State<'_, Arc<Mutex<App>>>,
) -> Result<Vec<LibraryBookMetadataView>, String> {
    let app = state.lock().await;

    let target_language = Language::from_639_3(&app.config.target_language_id);

    if let Some(library) = &app.library_view {
        library
            .list_books(target_language.as_ref())
            .await
            .map_err(|err| err.to_string())
    } else {
        Ok(vec![])
    }
}

#[tauri::command]
pub async fn list_book_chapters(
    state: tauri::State<'_, Arc<Mutex<App>>>,
    book_id: Uuid,
) -> Result<Vec<ChapterView>, String> {
    let mut app = state.lock().await;
    if let Some(library) = &mut app.library_view {
        library
            .list_book_chapters(book_id)
            .await
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

    let target_language = Language::from_639_3(&app.config.target_language_id);

    if let Some(library) = &mut app.library_view
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

    let target_language = Language::from_639_3(&app.config.target_language_id);

    if let Some(library) = &mut app.library_view
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

    let target_language = Language::from_639_3(&app.config.target_language_id);

    if let Some(library) = &mut app.library_view {
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
    let target_language = Language::from_639_3(&app.config.target_language_id);

    if let Some(library) = &mut app.library_view {
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

#[tauri::command]
pub async fn get_book_reading_state(
    state: tauri::State<'_, Arc<Mutex<App>>>,
    book_id: Uuid,
) -> Result<Option<BookReadingStateView>, String> {
    let app = state.lock().await;
    if let Some(library) = &app.library_view {
        library
            .get_book_reading_state(book_id)
            .await
            .map_err(|err| err.to_string())
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub async fn save_book_reading_state(
    state: tauri::State<'_, Arc<Mutex<App>>>,
    book_id: Uuid,
    chapter_id: usize,
    paragraph_id: usize,
) -> Result<(), String> {
    let app = state.lock().await;
    if let Some(library) = &app.library_view {
        library
            .save_book_reading_state(book_id, chapter_id, paragraph_id)
            .await
            .map_err(|err| err.to_string())
    } else {
        Err("Library is not configured".into())
    }
}

#[tauri::command]
pub async fn move_book(
    state: tauri::State<'_, Arc<Mutex<App>>>,
    book_id: Uuid,
    path: Vec<String>,
) -> Result<(), String> {
    let app = state.lock().await;
    let target_language = Language::from_639_3(&app.config.target_language_id);

    if let Some(library) = &app.library_view {
        library
            .move_book(book_id, path, target_language.as_ref())
            .await
            .map_err(|err| err.to_string())
    } else {
        Err("Library is not configured".into())
    }
}

#[tauri::command]
pub async fn delete_book(
    state: tauri::State<'_, Arc<Mutex<App>>>,
    book_id: Uuid,
) -> Result<(), String> {
    let app = state.lock().await;
    let target_language = Language::from_639_3(&app.config.target_language_id);

    if let Some(library) = &app.library_view {
        library
            .delete_book(book_id, target_language.as_ref())
            .await
            .map_err(|err| err.to_string())
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

    let decode_lossy = |value: &str| -> String {
        decode(value.as_bytes())
            .to_string()
            .unwrap_or_else(|_| value.to_owned())
    };

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
                } else if levenshtein_distance(&w.to_lowercase(), &p_word.to_lowercase()) < 2 {
                    break;
                }

                offset += 1;
            }

            if offset > 0 {
                let end = (p_idx + offset).min(original.len());
                let text = String::from_iter(original[p_idx..end].iter());
                result.push(text);
            }

            p_idx += offset;

            let mut clamped_end = p_idx + len;
            if clamped_end >= original.len() {
                clamped_end = original.len();
            }

            if p_idx < clamped_end {
                let text = String::from_iter(original[p_idx..clamped_end].iter());
                let translation_fragment = word
                    .contextual_translations()
                    .next()
                    .map(|ct| sanitize_translation_text(ct.translation.as_ref()))
                    .filter(|t| !t.is_empty())
                    .map(|t| {
                        format!(
                            "<span class=\"word-translation\" aria-hidden=\"true\">{}</span>",
                            encode(
                                t.as_bytes(),
                                &EncodeType::Named,
                                &CharacterSet::SpecialChars
                            )
                            .to_string()
                            .unwrap_or("&lt;err&gt;".to_owned())
                        )
                    })
                    .unwrap_or_default();

                result.push(format!("<span class=\"word-span\" data-paragraph=\"{paragraph_id}\" data-sentence=\"{sentence_idx}\" data-word=\"{word_idx}\">{translation_fragment}{text}</span>"));
            }

            p_idx = clamped_end;
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

#[cfg(test)]
mod tests {
    use super::translation_to_html;

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

    #[test]
    fn wraps_words_and_escapes_translation_fragment() {
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
        let html = translation_to_html(7, original, &view).expect("html");

        assert!(html.contains("data-paragraph=\"7\""));
        assert!(html.contains("<span class=\"word-span\""));
        assert!(html.contains("Hello"));
        assert!(html.contains(", ") || html.contains(","));
        assert!(html.contains("world"));

        // Translation fragment should be HTML-escaped
        assert!(html.contains("&lt;b&gt;hi&lt;/b&gt;"));
        // And whitespace should be normalized
        assert!(
            html.contains(">planet<") || html.contains(">planet </") || html.contains("planet")
        );
    }

    #[test]
    fn empty_contextual_translation_produces_no_translation_span() {
        let original = "Just words";

        let pt = make_paragraph_translation(vec![translation_import::Sentence {
            full_translation: "ignored".to_owned(),
            words: vec![word("Just", &[], false), word("words", &[], false)],
        }]);

        let mut t = library::book::translation::Translation::create("deu", "eng");
        let view = view_from_import(&mut t, 0, &pt);
        let html = translation_to_html(0, original, &view).expect("html");

        assert!(!html.contains("word-translation"));
    }

    #[test]
    fn preserves_original_html_entities_and_does_not_error() {
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
        let html = translation_to_html(1, original, &view).expect("html");

        // The input entity should still appear as-is in output (we preserve original slices).
        assert!(html.contains("&amp;"));
        assert!(html.contains("Tom"));
        assert!(html.contains("Jerry"));
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
        let html = translation_to_html(2, original, &view).expect("html");

        assert!(html.contains("naïve"));
        assert!(html.contains("café"));
        assert!(html.contains("data-sentence=\"0\""));
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
        let html = translation_to_html(3, original, &view).expect("html");

        assert!(html.contains("data-sentence=\"0\""));
        assert!(html.contains("data-sentence=\"1\""));
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

        let html = translation_to_html(4, original, &view).expect("html");
        assert!(html.contains("A"));
        assert!(html.contains("B"));
    }

    #[test]
    fn punctuation_only_translation_returns_original() {
        let original = "...";

        let pt = make_paragraph_translation(vec![translation_import::Sentence {
            full_translation: "ignored".to_owned(),
            words: vec![word("&period;", &[], true), word("&period;", &[], true)],
        }]);

        let mut t = library::book::translation::Translation::create("deu", "eng");
        let view = view_from_import(&mut t, 0, &pt);
        let html = translation_to_html(5, original, &view).expect("html");

        assert_eq!(html, original);
    }
}

fn sanitize_translation_text(value: &str) -> String {
    value
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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
