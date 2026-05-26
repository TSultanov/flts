use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::SystemTime,
};

use log::info;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
use log::warn;

use crate::tla_trace::mutex::{TracedLock, TracedMutex};
use isolang::Language;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use crate::{
    book::{
        book::Book,
        serialization::{Serializable, create_random_string},
        translation::{ParagraphTranslationView, Translation},
        translation_import,
    },
    library::{Library, LibraryBookMetadata, LibraryError, LibraryTranslationMetadata},
    tla_trace,
    translator::TranslationModel,
};

mod reading_state;
#[cfg(test)]
mod tests;

pub use reading_state::load_book_user_state;
use reading_state::{load_user_state_from_dir, persist_user_state};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BookReadingState {
    #[serde(alias = "chapterId")]
    pub chapter_id: usize,
    #[serde(alias = "paragraphId")]
    pub paragraph_id: usize,
    // Which column of the saved paragraph the reader was on. Zero for
    // single-column paragraphs (the desktop common case). On touch
    // devices, where break-inside: auto lets a paragraph flow across
    // multiple columns, this tells restore which page to land on.
    // Serde default keeps state.json files written before this field
    // existed loadable.
    #[serde(default, alias = "pageOffset")]
    pub page_offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct BookUserState {
    #[serde(default, rename = "readingState")]
    pub reading_state: Option<BookReadingState>,
    #[serde(default, rename = "folderPath")]
    pub folder_path: Vec<String>,
}

pub struct LibraryBook {
    path: PathBuf,
    last_modified: Option<SystemTime>,
    pub book: Book,
    translations: Vec<Arc<TracedMutex<LibraryTranslation>>>,
    user_state: BookUserState,
}

pub struct LibraryTranslation {
    translation: Translation,
    source_language: Language,
    target_language: Language,
    last_modified: Option<SystemTime>,
    changed: bool,
}

impl TracedLock for LibraryBook {
    fn lock_name(&self) -> String {
        format!("book:{}", self.book.id)
    }
}

impl TracedLock for LibraryTranslation {
    fn lock_name(&self) -> String {
        format!(
            "trans:{}_{}",
            self.source_language.to_639_3(),
            self.target_language.to_639_3()
        )
    }
}

impl LibraryBook {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl LibraryTranslation {
    pub fn is_changed(&self) -> bool {
        self.changed
    }

    fn merge(&mut self, other: LibraryTranslation) {
        let other_t = other.translation;

        let merged_translation = self.translation.merge(&other_t);

        self.translation = merged_translation;
        self.last_modified = self.last_modified.max(other.last_modified);
        self.changed = true;
    }

    async fn load(path: &Path) -> anyhow::Result<Self> {
        let metadata = tokio::fs::metadata(path).await?;
        let last_modified = metadata.modified().ok();
        let mut file = tokio::fs::File::open(path).await?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await?;
        let mut cursor = std::io::Cursor::new(buffer);
        let translation = Translation::deserialize(&mut cursor)?;
        let source_language = Language::from_str(&translation.source_language)?;
        let target_language = Language::from_str(&translation.target_language)?;

        Ok(Self {
            translation,
            source_language,
            target_language,
            last_modified,
            changed: false,
        })
    }

    async fn load_from_metadata(metadata: LibraryTranslationMetadata) -> anyhow::Result<Self> {
        let translation_path = metadata.main_path.clone();
        if !metadata.conflicting_paths.is_empty() {
            let mut translation = {
                let content = tokio::fs::read(&metadata.main_path).await?;
                let mut cursor = std::io::Cursor::new(content);
                Translation::deserialize(&mut cursor)?
            };

            for conflict in metadata.conflicting_paths {
                {
                    let content = tokio::fs::read(&conflict).await?;
                    let mut cursor = std::io::Cursor::new(content);
                    let conflict_translation = Translation::deserialize(&mut cursor)?;
                    translation = translation.merge(&conflict_translation);
                }
                tokio::fs::remove_file(&conflict).await?;
            }

            let mut buf = Vec::new();
            translation.serialize(&mut buf)?;
            tokio::fs::write(&metadata.main_path, buf).await?;
        }

        let loaded = Self::load(&metadata.main_path).await?;
        if let Some(book_dir) = translation_path.parent() {
            tla_trace::emit_translation_event(
                book_dir,
                &translation_path,
                "LoadTranslationFromMetadata",
                None,
                "idle",
                "idle",
                "idle",
            )
            .await?;
        }
        Ok(loaded)
    }

    pub fn add_paragraph_translation(
        &mut self,
        paragraph_index: usize,
        translation: &translation_import::ParagraphTranslation,
        model: TranslationModel,
    ) {
        self.translation
            .add_paragraph_translation(paragraph_index, translation, model);
        self.changed = true;
    }

    pub fn translated_paragraphs_count(&self) -> usize {
        self.translation.translated_paragraphs_count()
    }

    pub fn paragraph_view(&'_ self, paragraph: usize) -> Option<ParagraphTranslationView<'_>> {
        self.translation.paragraph_view(paragraph)
    }

    /// Marks a word index as visible (annotation shown) for the given paragraph.
    /// Returns true if the word was newly marked visible.
    pub fn mark_word_visible(&mut self, paragraph: usize, word_index: usize) -> bool {
        let result = self.translation.mark_word_visible(paragraph, word_index);
        if result {
            self.changed = true;
        }
        result
    }
}

impl LibraryBook {
    pub async fn has_unsaved_changes(&self) -> bool {
        for t_arc in &self.translations {
            if t_arc.lock().await.is_changed() {
                return true;
            }
        }
        false
    }

    async fn reload_user_state(&mut self) -> anyhow::Result<()> {
        self.user_state = load_user_state_from_dir(&self.path).await?;
        Ok(())
    }

    pub async fn reading_state(&mut self) -> anyhow::Result<Option<BookReadingState>> {
        self.reload_user_state().await?;
        Ok(self.user_state.reading_state.clone())
    }

    pub async fn update_reading_state(&mut self, state: BookReadingState) -> anyhow::Result<()> {
        self.reload_user_state().await?;
        tla_trace::emit_book_event(
            &self.path,
            "UpdateReadingStateReload",
            Some(tla_trace::TraceArg {
                reading: Some(format!("{}:{}", state.chapter_id, state.paragraph_id)),
                folder: None,
            }),
            "idle",
            "idle",
            "reading",
        )
        .await?;
        self.user_state.reading_state = Some(state);
        persist_user_state(&self.path, &self.user_state).await?;
        tla_trace::emit_book_event(
            &self.path,
            "UpdateReadingStatePersist",
            None,
            "idle",
            "idle",
            "idle",
        )
        .await?;
        Ok(())
    }

    pub async fn update_folder_path(&mut self, folder_path: Vec<String>) -> anyhow::Result<()> {
        self.reload_user_state().await?;
        tla_trace::emit_book_event(
            &self.path,
            "UpdateFolderPathReload",
            Some(tla_trace::TraceArg {
                reading: None,
                folder: Some(folder_path.join("/")),
            }),
            "idle",
            "idle",
            "folder",
        )
        .await?;
        self.user_state.folder_path = folder_path;
        persist_user_state(&self.path, &self.user_state).await?;
        tla_trace::emit_book_event(
            &self.path,
            "UpdateFolderPathPersist",
            None,
            "idle",
            "idle",
            "idle",
        )
        .await?;
        Ok(())
    }

    pub async fn folder_path(&mut self) -> anyhow::Result<Vec<String>> {
        self.reload_user_state().await?;
        Ok(self.user_state.folder_path.clone())
    }

    pub async fn get_translation(
        &self,
        target_language: &Language,
    ) -> Option<Arc<TracedMutex<LibraryTranslation>>> {
        let source_language = &self.book.language;
        for t in self.translations.iter() {
            let guard = t.lock().await;
            if &guard.translation.source_language == source_language
                && guard.translation.target_language == target_language.to_639_3()
            {
                drop(guard);
                return Some(t.clone());
            }
        }
        None
    }

    pub async fn get_or_create_translation(
        &mut self,
        target_language: &Language,
    ) -> Arc<TracedMutex<LibraryTranslation>> {
        let source_language = &self.book.language;

        for (t_idx, t) in self.translations.iter().enumerate() {
            // Double-lock pattern: check source then target language.
            // Each lock is acquired and released independently; TracedMutex
            // emits AcqTrans/RelTrans automatically for each.
            let src_match = {
                let guard = t.lock().await;
                &guard.translation.source_language == source_language
            };

            if src_match {
                let tgt_match = {
                    let guard = t.lock().await;
                    guard.translation.target_language == target_language.to_639_3()
                };

                if tgt_match {
                    return self.translations[t_idx].clone();
                }
            }
        }

        // Not found: create and push
        self.translations
            .push(Arc::new(TracedMutex::new(LibraryTranslation {
                translation: Translation::create(source_language, target_language.to_639_3()),
                source_language: Language::from_639_3(source_language).unwrap(),
                target_language: *target_language,
                last_modified: None,
                changed: true,
            })));

        let last = self.translations.len() - 1;
        self.translations[last].clone()
    }

    pub async fn load_from_metadata(metadata: LibraryBookMetadata) -> anyhow::Result<Self> {
        let mut candidates: Vec<(&PathBuf, Option<SystemTime>)> = Vec::new();
        candidates.push((
            &metadata.main_path,
            tokio::fs::metadata(&metadata.main_path)
                .await
                .ok()
                .and_then(|m| m.modified().ok()),
        ));
        for p in &metadata.conflicting_paths {
            candidates.push((
                p,
                tokio::fs::metadata(p)
                    .await
                    .ok()
                    .and_then(|m| m.modified().ok()),
            ));
        }

        let mut newest_idx = 0usize;
        let mut newest_time = candidates[0].1.unwrap_or(SystemTime::UNIX_EPOCH);
        for (i, (_, m)) in candidates.iter().enumerate().skip(1) {
            if m.unwrap_or(SystemTime::UNIX_EPOCH) > newest_time {
                newest_idx = i;
                newest_time = m.unwrap_or(SystemTime::UNIX_EPOCH);
            }
        }

        if newest_idx != 0 {
            if metadata.main_path.exists() {
                tokio::fs::remove_file(&metadata.main_path).await?;
            }
            let source = &candidates[newest_idx].0;
            if source.exists() {
                tokio::fs::rename(source, &metadata.main_path).await?;
            }
        }

        for p in metadata.conflicting_paths {
            if p.exists() {
                // It's possible we've just moved the newest conflict into main, so ignore missing
                let _ = tokio::fs::remove_file(p).await;
            }
        }

        let mut book = Self::load(&metadata.main_path).await?;

        for tm in metadata.translations_metadata {
            let translation = Arc::new(TracedMutex::new(
                LibraryTranslation::load_from_metadata(tm).await?,
            ));
            book.translations.push(translation);
        }

        book.reload_user_state().await?;
        tla_trace::emit_book_event(
            &book.path,
            "LoadBookFromMetadata",
            None,
            "idle",
            "idle",
            "idle",
        )
        .await?;

        Ok(book)
    }

    async fn load(path: &Path) -> anyhow::Result<Self> {
        let last_modified = tokio::fs::metadata(path).await?.modified().ok();
        let mut file = tokio::fs::File::open(path).await?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await?;
        let mut cursor = std::io::Cursor::new(buffer);
        let book = Book::deserialize(&mut cursor)?;

        Ok(Self {
            path: path.parent().unwrap().to_path_buf(),
            last_modified,
            book,
            translations: vec![],
            user_state: BookUserState::default(),
        })
    }

    pub async fn reload_book(&mut self, modified: SystemTime) -> anyhow::Result<bool> {
        Ok(if self.last_modified.is_none_or(|lm| lm < modified) {
            self.save().await?;
            true
        } else {
            false
        })
    }

    pub async fn reload_translations(
        &mut self,
        modified: SystemTime,
        from: Language,
        to: Language,
    ) -> anyhow::Result<bool> {
        let mut needs_save = false;

        for translation in &self.translations {
            let t = translation.lock().await;
            if t.source_language == from
                && t.target_language == to
                && t.last_modified.is_none_or(|lm| lm < modified)
            {
                needs_save = true;
            }
        }

        Ok(if needs_save {
            self.save().await?;
            true
        } else {
            false
        })
    }

    pub async fn save(&mut self) -> anyhow::Result<()> {
        if !tokio::fs::try_exists(&self.path).await? {
            tokio::fs::create_dir_all(&self.path).await?;
        }

        let book = self;

        let mut merged_translations = Vec::new();

        for translation_arc in book.translations.drain(0..) {
            let mut translation = translation_arc.lock().await;
            let source_language = translation.translation.source_language.clone();
            let target_language = translation.translation.target_language.clone();
            let translation_file_name =
                format!("translation_{}_{}.dat", source_language, target_language);
            let translation_path = book.path.join(&translation_file_name);
            let translation_path_temp = book.path.join(format!(
                "{translation_file_name}~{}",
                create_random_string(8)
            ));

            loop {
                let translation_path_modified_pre_save =
                    if tokio::fs::try_exists(&translation_path).await? {
                        tokio::fs::metadata(&translation_path)
                            .await?
                            .modified()
                            .ok()
                    } else {
                        None
                    };

                if let Some(last_modified) = translation.last_modified {
                    if tokio::fs::try_exists(&translation_path).await? {
                        let saved_translation_last_modified =
                            tokio::fs::metadata(&translation_path)
                                .await?
                                .modified()
                                .unwrap();
                        if saved_translation_last_modified > last_modified {
                            let saved_translation =
                                LibraryTranslation::load(&translation_path).await?;
                            translation.merge(saved_translation);
                        }
                    }
                } else if tokio::fs::try_exists(&translation_path).await? {
                    let saved_translation = LibraryTranslation::load(&translation_path).await?;
                    translation.merge(saved_translation);
                }

                tla_trace::emit_translation_event(
                    &book.path,
                    &translation_path,
                    "SaveTranslationBegin",
                    None,
                    "idle",
                    "ready",
                    "idle",
                )
                .await?;

                if translation.changed {
                    let mut translation_file =
                        tokio::fs::File::create(&translation_path_temp).await?;
                    let mut buffer = Vec::new();
                    translation.translation.serialize(&mut buffer)?;
                    translation_file.write_all(&buffer).await?;

                    if (if tokio::fs::try_exists(&translation_path).await? {
                        tokio::fs::metadata(&translation_path)
                            .await?
                            .modified()
                            .ok()
                    } else {
                        None
                    }) == translation_path_modified_pre_save
                        || translation_path_modified_pre_save.is_none()
                    {
                        if tokio::fs::try_exists(&translation_path).await? {
                            tokio::fs::remove_file(&translation_path).await?;
                        }
                        tokio::fs::rename(&translation_path_temp, &translation_path).await?;
                        translation.last_modified = tokio::fs::metadata(&translation_path)
                            .await?
                            .modified()
                            .ok();
                        tla_trace::emit_translation_event(
                            &book.path,
                            &translation_path,
                            "SaveTranslationFinish",
                            None,
                            "idle",
                            "idle",
                            "idle",
                        )
                        .await?;
                        merged_translations.push(translation_arc.clone());
                        break;
                    }
                } else {
                    merged_translations.push(translation_arc.clone());
                    break;
                }
            }
        }

        let book_path = book.path.join("book.dat");
        let book_path_temp = book
            .path
            .join(format!("book.dat~{}", create_random_string(8)));
        loop {
            let book_path_modified_pre_save = if tokio::fs::try_exists(&book_path).await? {
                tokio::fs::metadata(&book_path).await?.modified().ok()
            } else {
                None
            };

            // If disk is newer, load it into memory (last writer wins).
            if let Some(last_modified) = book.last_modified {
                if tokio::fs::try_exists(&book_path).await? {
                    let saved_book_last_modified =
                        tokio::fs::metadata(&book_path).await?.modified().unwrap();
                    if saved_book_last_modified > last_modified {
                        let saved_book = Self::load(&book_path).await?;
                        book.book = saved_book.book;
                        book.last_modified = saved_book.last_modified;
                    }
                }
            } else if tokio::fs::try_exists(&book_path).await? {
                let saved_book = Self::load(&book_path).await?;
                book.book = saved_book.book;
                book.last_modified = saved_book.last_modified;
            }

            let mut file = tokio::fs::File::create(&book_path_temp).await?;
            let mut buffer = Vec::new();
            book.book.serialize(&mut buffer)?;
            file.write_all(&buffer).await?;

            tla_trace::emit_book_event(&book.path, "SaveBookBegin", None, "ready", "idle", "idle")
                .await?;

            if (if tokio::fs::try_exists(&book_path).await? {
                tokio::fs::metadata(&book_path).await?.modified().ok()
            } else {
                None
            }) == book_path_modified_pre_save
                || book_path_modified_pre_save.is_none()
            {
                if tokio::fs::try_exists(&book_path).await? {
                    tokio::fs::remove_file(&book_path).await?;
                }
                tokio::fs::rename(&book_path_temp, &book_path).await?;

                book.last_modified = tokio::fs::metadata(&book_path).await?.modified().ok();
                tla_trace::emit_book_event(
                    &book.path,
                    "SaveBookFinish",
                    None,
                    "idle",
                    "idle",
                    "idle",
                )
                .await?;
                break;
            }
            // Attempt to merge and save again otherwise
        }

        let all_book_translations = LibraryBookMetadata::load(&book.path).await?;
        let mut loaded_translations = HashSet::new();
        for t in &merged_translations {
            loaded_translations.insert(t.lock().await.translation.id);
        }

        for translation_metadata in all_book_translations.translations_metadata {
            if !loaded_translations.contains(&translation_metadata.id) {
                merged_translations.push(Arc::new(TracedMutex::new(
                    LibraryTranslation::load_from_metadata(translation_metadata).await?,
                )));
            }
        }

        book.translations = merged_translations;

        Ok(())
    }
}

async fn remove_dir_recursive(path: &Path) -> anyhow::Result<()> {
    if !tokio::fs::try_exists(path).await? {
        return Ok(());
    }

    if tokio::fs::metadata(path).await?.is_file() {
        tokio::fs::remove_file(path).await?;
        return Ok(());
    }

    tokio::fs::remove_dir_all(path).await?;
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn try_move_to_trash(physical_path: &Path) -> anyhow::Result<bool> {
    if std::fs::metadata(physical_path).is_err() {
        return Ok(false);
    }

    match trash::delete(physical_path) {
        Ok(_) => Ok(true),
        Err(err) => {
            warn!("Failed to move {:?} to recycle bin: {}", physical_path, err);
            Ok(false)
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
#[allow(dead_code)]
fn try_move_to_trash(_path: &std::path::Path) -> anyhow::Result<bool> {
    Ok(false)
}

impl Library {
    pub async fn create_book(
        &self,
        title: &str,
        language: &Language,
    ) -> anyhow::Result<Arc<TracedMutex<LibraryBook>>> {
        let books = self.list_books().await?;
        if books.iter().any(|b| b.title == title) {
            Err(LibraryError::DuplicateTitle(title.to_owned()))?
        }

        let guid = Uuid::new_v4();
        let book_root = self.library_root.join(guid.to_string());

        let book = Arc::new(TracedMutex::new(LibraryBook {
            path: book_root,
            last_modified: None,
            book: Book::create(guid, title, language),
            translations: vec![],
            user_state: BookUserState::default(),
        }));

        let book = self.books_cache.insert(guid, book).await;

        Ok(book)
    }

    pub async fn delete_book(&self, uuid: &Uuid) -> anyhow::Result<()> {
        self.books_cache.remove(uuid).await;
        let book_path = self.library_root.join(uuid.to_string());

        if !tokio::fs::try_exists(&book_path).await? {
            return Ok(());
        }

        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            info!("Attempting to move {:?} to trash", book_path);
            if try_move_to_trash(&book_path)? {
                info!("Book at {:?} moved to system recycle bin", book_path);
                return Ok(());
            }
        }

        remove_dir_recursive(&book_path).await?;
        info!("Book at {:?} removed completely", book_path);
        Ok(())
    }
}
