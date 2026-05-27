use std::sync::Arc;

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

