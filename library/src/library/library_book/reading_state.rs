use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    time::SystemTime,
};

use tokio::io::AsyncWriteExt;

use crate::{book::serialization::create_random_string, tla_trace};

use super::{BookReadingState, BookUserState};

async fn reading_state_files(path: &Path) -> anyhow::Result<Vec<(PathBuf, SystemTime)>> {
    let mut files = Vec::new();
    let mut read_dir = tokio::fs::read_dir(path).await?;
    while let Some(entry) = read_dir.next_entry().await? {
        let path = entry.path();
        if path.is_file()
            && let Some(filename) = path.file_name().and_then(|n| n.to_str())
            && filename.starts_with("state")
            && filename.ends_with(".json")
        {
            // An unlocked concurrent resolver may have removed the entry since
            // the scan; skipping beats failing the whole load.
            let modified = match entry.metadata().await {
                Ok(metadata) => metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                Err(_) => continue,
            };
            files.push((path, modified));
        }
    }
    Ok(files)
}

/// Parses a `state*.json`: empty file, `{readingState, folderPath}` object, or
/// a bare [`BookReadingState`]. Unparseable content yields `None` so one
/// corrupt conflict sibling can't poison the merge or brick the book.
fn parse_user_state(contents: &str) -> Option<BookUserState> {
    if contents.trim().is_empty() {
        return Some(BookUserState::default());
    }

    let value: serde_json::Value = serde_json::from_str(contents).ok()?;
    if value.get("readingState").is_some() || value.get("folderPath").is_some() {
        return serde_json::from_value(value).ok();
    }

    let legacy: BookReadingState = serde_json::from_value(value).ok()?;
    Some(BookUserState {
        reading_state: Some(legacy),
        ..BookUserState::default()
    })
}

/// Field-wise merge of every `state*.json` candidate in a book directory.
///
/// `reading_state` and `folder_path` share a file but change independently
/// (position auto-saves, folder moves are one-shot), so keeping only the
/// newest-mtime file would silently revert a concurrent folder move. Fields are
/// unioned instead, mtime breaking ties within one field; `candidates` must be
/// oldest-first so the last file carrying a field wins it.
///
/// Without per-field stamps, a deliberate clear is indistinguishable from
/// "never set", so a stale non-empty sibling can override it.
fn merge_user_states(candidates: &[(BookUserState, SystemTime)]) -> BookUserState {
    let mut merged = BookUserState::default();
    for (state, _) in candidates {
        if state.reading_state.is_some() {
            merged.reading_state = state.reading_state.clone();
        }
        if !state.folder_path.is_empty() {
            merged.folder_path = state.folder_path.clone();
        }
    }
    merged
}

/// Writes `state` via temp file + `rename(2)`. The rename replaces the
/// canonical file without unlinking it, so a concurrent reader never observes a
/// missing `state.json`.
async fn write_state_file(state_path: &Path, state: &BookUserState) -> anyhow::Result<()> {
    let dir = state_path.parent().unwrap_or_else(|| Path::new("."));
    let temp_path = dir.join(format!("state.json~{}", create_random_string(8)));

    {
        let mut file = tokio::fs::File::create(&temp_path).await?;
        let content = serde_json::to_vec_pretty(state)?;
        file.write_all(&content).await?;
    }

    tokio::fs::rename(&temp_path, state_path).await?;
    Ok(())
}

/// Merges conflict siblings field-wise back into a single `state.json`.
///
/// Runs with **no book lock**, concurrently with the book-locked writers, so
/// every filesystem step tolerates a peer removing or replacing a file:
/// vanished siblings are skipped, the write is an atomic replace, deletion is
/// best-effort. None of these races may fail the load.
async fn resolve_reading_state_file(path: &Path) -> anyhow::Result<BookUserState> {
    let mut files = reading_state_files(path).await?;
    if files.is_empty() {
        return Ok(BookUserState::default());
    }
    // Oldest-first: the newest file carrying a field wins that field.
    files.sort_by(|a, b| a.1.cmp(&b.1));

    let canonical_path = path.join("state.json");
    let canonical_name = canonical_path
        .file_name()
        .expect("state.json has a file name")
        .to_owned();

    // Tolerate a concurrently deleted sibling, and skip unparseable ones so
    // one corrupt file cannot poison the rest.
    let mut parsed: Vec<(BookUserState, SystemTime)> = Vec::with_capacity(files.len());
    for (candidate_path, modified) in &files {
        match tokio::fs::read_to_string(candidate_path).await {
            Ok(contents) => {
                if let Some(state) = parse_user_state(&contents) {
                    parsed.push((state, *modified));
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }

    // Leave unreadable files untouched; they may still be recoverable.
    if parsed.is_empty() {
        return Ok(BookUserState::default());
    }

    let merged = merge_user_states(&parsed);

    let only_canonical =
        files.len() == 1 && files[0].0.file_name() == Some(canonical_name.as_os_str());
    if only_canonical {
        return Ok(merged);
    }

    // Persist before dropping the siblings, so a crash or racing reader never
    // sees the merged fields disappear.
    write_state_file(&canonical_path, &merged).await?;

    for (candidate_path, _) in &files {
        if candidate_path.file_name() != Some(canonical_name.as_os_str())
            && tokio::fs::try_exists(candidate_path).await?
        {
            let _ = tokio::fs::remove_file(candidate_path).await;
        }
    }

    tla_trace::emit_book_event(
        path,
        "ResolveReadingStateFile",
        None,
        "idle",
        "idle",
        "idle",
    )
    .await?;

    Ok(merged)
}

pub(super) async fn load_user_state_from_dir(path: &Path) -> anyhow::Result<BookUserState> {
    resolve_reading_state_file(path).await
}

pub(super) async fn persist_user_state(path: &Path, state: &BookUserState) -> anyhow::Result<()> {
    if !tokio::fs::try_exists(path).await? {
        tokio::fs::create_dir_all(path).await?;
    }

    // Atomic replace only: remove-then-rename opens a window where an unlocked
    // concurrent resolver sees no state.json.
    write_state_file(&path.join("state.json"), state).await
}

pub async fn load_book_user_state(path: &Path) -> anyhow::Result<BookUserState> {
    load_user_state_from_dir(path).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TempDir;

    fn write_json(path: &Path, json: &str) {
        std::fs::write(path, json).unwrap();
    }

    fn book_dir(name: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(name);
        let book = dir.path.join("book");
        std::fs::create_dir_all(&book).unwrap();
        (dir, book)
    }

    // Reading position (newer mtime) and a folder move (older) land in
    // different siblings; the merge must keep both and consolidate.
    #[tokio::test]
    async fn merge_keeps_folder_and_reading_from_different_siblings() {
        let (_guard, book) = book_dir("flts_rs_merge");

        write_json(
            &book.join("state.json"),
            r#"{"readingState":null,"folderPath":["Shelf","Favorites"]}"#,
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_json(
            &book.join("state (conflict copy).json"),
            r#"{"readingState":{"chapter_id":7,"paragraph_id":3,"page_offset":1},"folderPath":[]}"#,
        );

        let merged = load_user_state_from_dir(&book).await.unwrap();

        assert_eq!(
            merged.folder_path,
            vec!["Shelf".to_string(), "Favorites".to_string()],
            "folder move from the older sibling must survive the newer auto-save"
        );
        let rs = merged
            .reading_state
            .clone()
            .expect("reading state from the newer sibling must survive");
        assert_eq!(rs.chapter_id, 7);
        assert_eq!(rs.paragraph_id, 3);
        assert_eq!(rs.page_offset, 1);

        assert!(book.join("state.json").exists());
        assert!(!book.join("state (conflict copy).json").exists());
        let reloaded = load_user_state_from_dir(&book).await.unwrap();
        assert_eq!(reloaded.folder_path, merged.folder_path);
        assert_eq!(reloaded.reading_state, merged.reading_state);
    }

    // With no state.json, a lone sibling must be promoted, not error.
    #[tokio::test]
    async fn resolve_promotes_lone_sibling_without_canonical() {
        let (_guard, book) = book_dir("flts_rs_promote");
        write_json(
            &book.join("state (conflict copy).json"),
            r#"{"readingState":{"chapter_id":2,"paragraph_id":4,"page_offset":0},"folderPath":["A"]}"#,
        );

        let state = load_user_state_from_dir(&book).await.unwrap();
        assert_eq!(state.folder_path, vec!["A".to_string()]);
        assert_eq!(state.reading_state.map(|s| s.chapter_id), Some(2));
        assert!(book.join("state.json").exists());
        assert!(!book.join("state (conflict copy).json").exists());
    }

    // An empty directory yields the default, never an error.
    #[tokio::test]
    async fn resolve_tolerates_missing_directory_contents() {
        let (_guard, book) = book_dir("flts_rs_empty");
        assert_eq!(
            load_user_state_from_dir(&book).await.unwrap(),
            BookUserState::default()
        );
    }

    // Two unlocked resolvers racing to promote and delete the same siblings
    // must both succeed.
    #[tokio::test]
    async fn resolve_tolerates_concurrent_removal_of_siblings() {
        let (_guard, book) = book_dir("flts_rs_race");
        let canonical =
            r#"{"readingState":{"chapter_id":1,"paragraph_id":1,"page_offset":0},"folderPath":[]}"#;
        let sibling = r#"{"readingState":{"chapter_id":5,"paragraph_id":9,"page_offset":0},"folderPath":["Shelf"]}"#;
        write_json(&book.join("state.json"), canonical);
        write_json(&book.join("state (conflict copy).json"), sibling);

        for _ in 0..25 {
            // Keep a sibling present for the resolvers to race over.
            let _ = std::fs::write(book.join("state (conflict copy).json"), sibling);
            let a = load_user_state_from_dir(&book);
            let b = load_user_state_from_dir(&book);
            let (ra, rb) = tokio::join!(a, b);
            ra.expect("resolver A must tolerate a concurrently removed sibling");
            rb.expect("resolver B must tolerate a concurrently removed sibling");
        }

        let final_state = load_user_state_from_dir(&book).await.unwrap();
        assert_eq!(final_state.folder_path, vec!["Shelf".to_string()]);
    }

    // Bare-BookReadingState files load; an empty state.json gives the default.
    #[tokio::test]
    async fn parse_handles_legacy_and_empty() {
        assert_eq!(parse_user_state(""), Some(BookUserState::default()));
        assert_eq!(parse_user_state("   \n"), Some(BookUserState::default()));

        let legacy = parse_user_state(r#"{"chapterId":3,"paragraphId":9}"#)
            .expect("legacy reading state parses");
        assert_eq!(legacy.reading_state.map(|s| s.chapter_id), Some(3));
        assert!(legacy.folder_path.is_empty());

        assert_eq!(parse_user_state("{not json"), None);
    }
}
