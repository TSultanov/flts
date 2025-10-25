use std::{sync::Mutex};

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

#[derive(Clone, serde::Serialize)]
pub struct ChapterView {
    id: usize,
    title: String,
}

#[derive(Clone, serde::Serialize)]
pub struct ParagraphView {
    id: usize,
    original: String,
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

    pub fn list_book_chapters(&mut self, book_id: Uuid) -> anyhow::Result<Vec<ChapterView>> {
        let book = self.library.get_book(&book_id)?;
        let book = book.blocking_lock();
        let book = &book.book;
        let chapters = book.chapter_views().map(|v| ChapterView {
            id: v.idx,
            title: v.title.map(|s| s.to_string()).unwrap_or("<no title>".to_owned()),
        }).collect();
        Ok(chapters)
    }

    pub fn list_book_chapter_paragraphs(&mut self, book_id: Uuid, chapter_id: usize) -> anyhow::Result<Vec<ParagraphView>> {
        let book = self.library.get_book(&book_id)?;
        let book = book.blocking_lock();
        let book = &book.book;
        Ok(book.chapter_view(chapter_id).paragraphs().map(|p| ParagraphView {
            id: p.id,
            original: p.original_html.unwrap_or(p.original_text).to_string()
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

#[tauri::command]
pub fn list_book_chapters(state: tauri::State<'_, Mutex<App>>, book_id: Uuid) -> Result<Vec<ChapterView>, String> {
    let mut app = state
        .lock()
        .map_err(|_| AppError::StatePoisonError.to_string())?;
    if let Some(library) = &mut app.library {
        library.list_book_chapters(book_id).map_err(|err| err.to_string())
    } else {
        Ok(vec![])
    }
}

#[tauri::command]
pub fn get_book_chapter_paragraphs(state: tauri::State<'_, Mutex<App>>, book_id: Uuid, chapter_id: usize) -> Result<Vec<ParagraphView>, String> {
    let mut app = state
        .lock()
        .map_err(|_| AppError::StatePoisonError.to_string())?;
    if let Some(library) = &mut app.library {
        library.list_book_chapter_paragraphs(book_id, chapter_id).map_err(|err| err.to_string())
    } else {
        Ok(vec![])
    }
}