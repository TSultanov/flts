use htmlentity::entity::{ICodedDataTrait, decode};
use isolang::Language;
use library::{book::translation::ParagraphTranslationView, library::Library};
use tauri::async_runtime::Mutex;
use uuid::Uuid;

use crate::app::{App, AppError};

#[derive(Clone, serde::Serialize)]
pub struct LibraryBookMetadataView {
    id: Uuid,
    title: String,
    chapters_count: usize,
    paragraphs_count: usize,
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

pub struct LibraryView {
    library: Library,
}

impl LibraryView {
    pub fn create(library: Library) -> Self {
        Self { library }
    }

    pub fn list_books(&self) -> anyhow::Result<Vec<LibraryBookMetadataView>> {
        let books = self.library.list_books()?;
        Ok(books
            .into_iter()
            .map(|b| LibraryBookMetadataView {
                id: b.id,
                title: b.title,
                chapters_count: b.chapters_count,
                paragraphs_count: b.paragraphs_count,
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
        let book = book.lock().await;

        let book_translation = book.get_translation(target_language).await;

        let mut views = Vec::new();
        for p in book.book.chapter_view(chapter_id).paragraphs() {
            let original = p.original_html.unwrap_or(p.original_text);

            let mut translation: Option<_> = None;
            if let Some(bt) = &book_translation {
                let bt = bt.lock().await;
                let t_view = bt.paragraph_view(p.id);
                translation = t_view.map(|t| {
                    translation_to_html(&original, &t).unwrap_or_else(|err| err.to_string())
                })
            }

            views.push(ParagraphView {
                id: p.id,
                original: original.to_string(),
                translation,
            });
        }

        Ok(views)
    }
}

#[tauri::command]
pub fn list_books(
    state: tauri::State<'_, Mutex<App>>,
) -> Result<Vec<LibraryBookMetadataView>, String> {
    let app = state.blocking_lock();
    if let Some(library) = &app.library {
        library.list_books().map_err(|err| err.to_string())
    } else {
        Ok(vec![])
    }
}

#[tauri::command]
pub fn list_book_chapters(
    state: tauri::State<'_, Mutex<App>>,
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
    state: tauri::State<'_, Mutex<App>>,
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

fn translation_to_html(
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
                    clamped_end = original.len() - 1;
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
                clamped_end = original.len() - 1;
            }

            if p_idx < clamped_end {
                let text = String::from_iter(original[p_idx..clamped_end].iter());
                result.push(format!("<span class=\"word-span\" data-sentence=\"{sentence_idx}\" data-word=\"{word_idx}\">{text}</span>"));
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
