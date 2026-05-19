use std::sync::Arc;

use isolang::Language;
use uuid::Uuid;

use crate::app::AppState;

use super::{
    BookReadingStateView, ChapterView, LibraryBookMetadataView, LibraryView, ParagraphOriginal,
    ParagraphTranslationSlice, ParagraphView, WordView,
};

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
