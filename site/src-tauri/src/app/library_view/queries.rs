use isolang::Language;
use uuid::Uuid;

use crate::app::AppState;

use super::{
    BookReadingStateView, BookSummaryStatusView, ChapterView, LibraryBookMetadataView, LibraryView,
    ParagraphHtml, ParagraphOriginal, ParagraphView, WordView,
};

#[tauri::command]
pub async fn list_books(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<LibraryBookMetadataView>, String> {
    let target_language_id = { state.config.borrow().target_language_id.clone() };
    let target_language = Language::from_639_3(&target_language_id);
    let library = state.library().await?;

    LibraryView::create(state.inner(), library)
        .list_books(target_language.as_ref())
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn list_book_chapters(
    state: tauri::State<'_, AppState>,
    book_id: Uuid,
) -> Result<Vec<ChapterView>, String> {
    let target_language_id = { state.config.borrow().target_language_id.clone() };
    let target_language = Language::from_639_3(&target_language_id);
    let library = state.library().await?;

    let mut library_view = LibraryView::create(state.inner(), library);
    library_view
        .list_book_chapters(book_id, target_language.as_ref())
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_book_chapter_paragraph_ids(
    state: tauri::State<'_, AppState>,
    book_id: Uuid,
    chapter_id: usize,
) -> Result<Vec<usize>, String> {
    let library = state.library().await?;

    LibraryView::create(state.inner(), library)
        .list_book_chapter_paragraph_ids(book_id, chapter_id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_word_info(
    state: tauri::State<'_, AppState>,
    book_id: Uuid,
    paragraph_id: usize,
    sentence_id: usize,
    word_id: usize,
) -> Result<Option<WordView>, String> {
    let library = state.library().await?;

    let target_language_id = { state.config.borrow().target_language_id.clone() };
    let Some(target_language) = Language::from_639_3(&target_language_id) else {
        return Ok(None);
    };

    let mut library_view = LibraryView::create(state.inner(), library);
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
    state: tauri::State<'_, AppState>,
    book_id: Uuid,
    paragraph_id: usize,
) -> Result<ParagraphView, String> {
    let library = state.library().await?;

    let target_language_id = { state.config.borrow().target_language_id.clone() };
    let Some(target_language) = Language::from_639_3(&target_language_id) else {
        return Err("Library is not configured".into());
    };

    let library_view = LibraryView::create(state.inner(), library);
    library_view
        .get_paragraph_view(book_id, paragraph_id, &target_language)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_paragraph_originals_batch(
    state: tauri::State<'_, AppState>,
    book_id: Uuid,
    paragraph_ids: Vec<usize>,
) -> Result<Vec<ParagraphOriginal>, String> {
    let library = state.library().await?;

    let library_view = LibraryView::create(state.inner(), library);
    library_view
        .get_paragraph_originals_batch(book_id, paragraph_ids)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_paragraph_html_batch(
    state: tauri::State<'_, AppState>,
    book_id: Uuid,
    paragraph_ids: Vec<usize>,
) -> Result<Vec<ParagraphHtml>, String> {
    let library = state.library().await?;

    let target_language_id = { state.config.borrow().target_language_id.clone() };
    let Some(target_language) = Language::from_639_3(&target_language_id) else {
        return Err("Library is not configured".into());
    };

    let library_view = LibraryView::create(state.inner(), library);
    library_view
        .get_paragraph_html_batch(book_id, paragraph_ids, &target_language)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_book_summary_status(
    state: tauri::State<'_, AppState>,
    book_id: Uuid,
) -> Result<BookSummaryStatusView, String> {
    let library = state.library().await?;

    let summaries = library
        .get_book(&book_id)
        .await
        .map_err(|err| err.to_string())?
        .summaries()
        .await
        .map_err(|err| err.to_string())?;
    let actively_generating = summaries.entries.iter().position(|e| !e.generated);
    Ok(BookSummaryStatusView {
        total_chapters: summaries.entries.len(),
        generated: summaries.entries.iter().map(|e| e.generated).collect(),
        actively_generating,
    })
}

#[tauri::command]
pub async fn get_book_reading_state(
    state: tauri::State<'_, AppState>,
    book_id: Uuid,
) -> Result<Option<BookReadingStateView>, String> {
    let library = state.library().await?;

    LibraryView::create(state.inner(), library)
        .get_book_reading_state(book_id)
        .await
        .map_err(|err| err.to_string())
}
