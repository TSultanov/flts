use std::sync::Arc;

use isolang::Language;
use library::library::CardCreationOutcome;
use uuid::Uuid;

use crate::app::AppState;

use super::{CardCreationResultView, LibraryView};

#[tauri::command]
pub async fn save_book_reading_state(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    chapter_id: usize,
    paragraph_id: usize,
    page_offset: usize,
) -> Result<(), String> {
    let library = state.library().await?;

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
    let library = state.library().await?;

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
    let library = state.library().await?;

    LibraryView::create(state.inner().clone(), library)
        .delete_book(book_id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn create_card_for_word(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: Uuid,
    paragraph_id: usize,
    sentence_id: usize,
    word_id: usize,
) -> Result<CardCreationResultView, String> {
    let target_language_id = { state.config.borrow().target_language_id.clone() };
    let Some(target_language) = Language::from_639_3(&target_language_id) else {
        return Err("Library is not configured".into());
    };
    let library = state.library().await?;

    library
        .create_card_for_word(book_id, paragraph_id, sentence_id, word_id, target_language)
        .await
        .map(|outcome| match outcome {
            CardCreationOutcome::Created => CardCreationResultView::Created,
            CardCreationOutcome::AlreadyExists => CardCreationResultView::AlreadyExists,
        })
        .map_err(|err| err.to_string())
}
