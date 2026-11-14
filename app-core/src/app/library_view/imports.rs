use base64::Engine;
use isolang::Language;
use library::epub_importer::EpubBook;
use uuid::Uuid;

use crate::app::AppState;

use super::LibraryView;

pub async fn import_plain_text(
    state: crate::host::State<'_, AppState>,
    title: String,
    text: String,
    source_language_id: String,
) -> Result<Uuid, String> {
    let library = state.library().await?;

    let source_language = Language::from_639_3(&source_language_id)
        .ok_or_else(|| format!("Failed to resolve source language: {}", source_language_id))?;

    let mut library_view = LibraryView::create(state.inner(), library);
    library_view
        .import_plain_text(&title, &text, &source_language)
        .await
        .map_err(|err| err.to_string())
}

pub async fn parse_epub(epub_base64: String) -> Result<EpubBook, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(epub_base64.as_bytes())
        .map_err(|err| err.to_string())?;
    tokio::task::spawn_blocking(move || EpubBook::from_bytes(bytes))
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())
}

pub async fn import_epub(
    state: crate::host::State<'_, AppState>,
    book: EpubBook,
    source_language_id: String,
) -> Result<Uuid, String> {
    let library = state.library().await?;

    let source_language = Language::from_639_3(&source_language_id)
        .ok_or_else(|| format!("Failed to resolve source language: {}", source_language_id))?;

    let mut library_view = LibraryView::create(state.inner(), library);
    library_view
        .import_epub(&book, &source_language)
        .await
        .map_err(|err| err.to_string())
}
