use std::sync::Mutex;

use library::library::Library;
use uuid::Uuid;

use crate::app::{App, AppError};

#[derive(Clone, serde::Serialize)]
pub struct LibraryBookMetadataView {
    id: Uuid,
    title: String,
    chapters_count: usize,
    paragraphs_count: usize,
}

pub struct LibraryView {
    library: Library
}

impl LibraryView {
    pub fn create(library: Library) -> Self {
        Self {
            library
        }
    }

    pub fn list_books(&self) -> anyhow::Result<Vec<LibraryBookMetadataView>> {
        let books = self.library.list_books()?;
        Ok(books.into_iter().map(|b| LibraryBookMetadataView {
            id: b.id,
            title: b.title,
            chapters_count: b.chapters_count,
            paragraphs_count: b.paragraphs_count,
        }).collect())
    }
}

#[tauri::command]
pub fn list_books(state: tauri::State<'_, Mutex<App>>) -> Result<Vec<LibraryBookMetadataView>, String> {
    let app = state
        .lock()
        .map_err(|_| AppError::StatePoisonError.to_string())?;
    if let Some(library) = &app.library {
        library.list_books().map_err(|err| err.to_string())
    } else {
        Ok(vec![])
    }
}