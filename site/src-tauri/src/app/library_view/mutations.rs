use std::sync::Arc;

use isolang::Language;
use tauri::Emitter;
use uuid::Uuid;

use crate::app::AppState;

use super::LibraryView;

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

    let now_visible = LibraryView::create(state.inner().clone(), library)
        .mark_word_visible(book_id, paragraph_id, flat_index, &target_language)
        .await
        .map_err(|err| err.to_string())?;

    // Always emit: a toggle always flips state. ChapterParagraphsStore
    // re-fetches the paragraph so the new manualToggle bit propagates.
    let _ = app.emit(
        "paragraph_updated",
        serde_json::json!({
            "bookId": book_id,
            "paragraphId": paragraph_id,
        }),
    );

    Ok(now_visible)
}
