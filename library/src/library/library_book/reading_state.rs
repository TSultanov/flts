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
            // The entry can vanish between the directory scan and this stat if a
            // concurrent, unlocked resolver already promoted/deleted it. Skip it
            // rather than letting a NotFound `?`-fail the whole load.
            let modified = match entry.metadata().await {
                Ok(metadata) => metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                Err(_) => continue,
            };
            files.push((path, modified));
        }
    }
    Ok(files)
}

/// Parse the raw contents of a `state*.json` file into a [`BookUserState`],
/// tolerating three encodings: an empty file, the current
/// `{readingState, folderPath}` object, and the legacy bare [`BookReadingState`]
/// (pre-`folderPath`). Returns `None` when the file cannot be parsed, so that a
/// single corrupt Syncthing conflict sibling can never poison the field-wise
/// merge of its healthy peers (nor brick the book by `?`-propagating).
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

/// Field-wise merge of every `state*.json` candidate found in a book directory.
///
/// `reading_state` and `folder_path` share one file but change independently:
/// reading position auto-saves continuously while a folder move is a one-shot
/// action. A Syncthing conflict that keeps only the newest-mtime file therefore
/// almost always lets the auto-save file win and silently reverts a concurrent
/// folder move (BUG B). Instead we union the fields across all candidates —
/// taking the freshest non-null `reading_state` and the freshest non-empty
/// `folder_path` — using mtime only to break ties within a single field.
///
/// `candidates` must be ordered oldest-first, so the last candidate carrying a
/// given field wins that field.
///
/// Limitation: without per-field update stamps we cannot distinguish "this file
/// never set folder_path" from "folder_path was deliberately cleared to empty",
/// so a genuine clear can be overridden by a stale non-empty sibling. Closing
/// that gap needs per-field timestamps stamped by the writers — see the module
/// note and the report accompanying this change.
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

/// Write `state` to `state_path` durably and atomically: serialize into a
/// uniquely named temp file, then `rename(2)` it into place. rename atomically
/// replaces any existing canonical file (POSIX rename / Win32
/// `MOVEFILE_REPLACE_EXISTING`) without ever unlinking it first, so a concurrent
/// reader never observes a missing `state.json`. This is the primitive that
/// fixes the remove-then-rename race (BUG A).
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

/// Load the book's user state from `path`, merging any Syncthing conflict
/// siblings field-wise and consolidating them back into a single canonical
/// `state.json`.
///
/// Runs from `load_book_user_state` with **no book lock**, concurrently with the
/// book-locked writers (`update_reading_state` / `persist_user_state`). Every
/// filesystem step is therefore tolerant of a peer having removed or replaced a
/// file out from under us: reads that hit NotFound skip the vanished sibling,
/// the merged result is written with an atomic replace, and sibling deletion is
/// best-effort. None of these races may `?`-fail the load (BUG A).
async fn resolve_reading_state_file(path: &Path) -> anyhow::Result<BookUserState> {
    let mut files = reading_state_files(path).await?;
    if files.is_empty() {
        return Ok(BookUserState::default());
    }
    // Oldest-first, so `merge_user_states` lets the newest file carrying each
    // field win that field.
    files.sort_by(|a, b| a.1.cmp(&b.1));

    let canonical_path = path.join("state.json");
    let canonical_name = canonical_path
        .file_name()
        .expect("state.json has a file name")
        .to_owned();

    // Read + parse every candidate, tolerating a sibling that a concurrent
    // resolver deleted between the directory scan and this read (NotFound), and
    // skipping any that fail to parse so one corrupt file cannot poison the rest.
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

    // Nothing readable/parseable: leave the files untouched (they may be
    // recoverable) and report an empty state rather than destroying data.
    if parsed.is_empty() {
        return Ok(BookUserState::default());
    }

    let merged = merge_user_states(&parsed);

    // Fast path: a single canonical file with no conflict siblings needs no
    // rewrite or cleanup.
    let only_canonical =
        files.len() == 1 && files[0].0.file_name() == Some(canonical_name.as_os_str());
    if only_canonical {
        return Ok(merged);
    }

    // Persist the merged result to the canonical file FIRST (atomic replace),
    // and only then drop the now-consolidated siblings, so a crash or a racing
    // reader never sees the merged fields disappear.
    write_state_file(&canonical_path, &merged).await?;

    for (candidate_path, _) in &files {
        if candidate_path.file_name() != Some(canonical_name.as_os_str())
            && tokio::fs::try_exists(candidate_path).await?
        {
            // Best-effort: a concurrent resolver may have removed it already.
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

    // Atomic replace only — never remove-then-rename. The pre-remove opened a
    // window during which a concurrent, unlocked resolver's `try_exists` on
    // state.json returned false (BUG A); rename's atomic replace closes it.
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

    // BUG B: reading position (auto-saved, so newer mtime) and a folder move
    // (one-shot, so older mtime) end up in two different Syncthing conflict
    // siblings. A "newest-mtime wins" resolve would drop whichever field the
    // loser owned. The field-wise merge must keep BOTH — and consolidate.
    #[tokio::test]
    async fn merge_keeps_folder_and_reading_from_different_siblings() {
        let (_guard, book) = book_dir("flts_rs_merge");

        // Older canonical carries only the folder move.
        write_json(
            &book.join("state.json"),
            r#"{"readingState":null,"folderPath":["Shelf","Favorites"]}"#,
        );
        // Distinct, newer mtime for the reading-position sibling.
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

        // Siblings are consolidated into a single canonical file holding the
        // merged result.
        assert!(book.join("state.json").exists());
        assert!(!book.join("state (conflict copy).json").exists());
        let reloaded = load_user_state_from_dir(&book).await.unwrap();
        assert_eq!(reloaded.folder_path, merged.folder_path);
        assert_eq!(reloaded.reading_state, merged.reading_state);
    }

    // BUG A tolerance: only a conflict sibling exists (no state.json). Resolve
    // must promote it via atomic rename without erroring on the absent canonical.
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

    // BUG A: an empty book directory and a missing canonical are tolerated
    // (return default), never an error.
    #[tokio::test]
    async fn resolve_tolerates_missing_directory_contents() {
        let (_guard, book) = book_dir("flts_rs_empty");
        assert_eq!(
            load_user_state_from_dir(&book).await.unwrap(),
            BookUserState::default()
        );
    }

    // BUG A core: load runs with no book lock, concurrently with book-locked
    // writers, over the same conflict siblings. Two resolvers racing to promote
    // and delete the same files must both succeed — no fatal NotFound `?`.
    #[tokio::test]
    async fn resolve_tolerates_concurrent_removal_of_siblings() {
        let (_guard, book) = book_dir("flts_rs_race");
        let canonical = r#"{"readingState":{"chapter_id":1,"paragraph_id":1,"page_offset":0},"folderPath":[]}"#;
        let sibling = r#"{"readingState":{"chapter_id":5,"paragraph_id":9,"page_offset":0},"folderPath":["Shelf"]}"#;
        write_json(&book.join("state.json"), canonical);
        write_json(&book.join("state (conflict copy).json"), sibling);

        for _ in 0..25 {
            // Re-create a sibling each round so there is always a file for the
            // two resolvers to race over (promote + delete).
            let _ = std::fs::write(book.join("state (conflict copy).json"), sibling);
            let a = load_user_state_from_dir(&book);
            let b = load_user_state_from_dir(&book);
            let (ra, rb) = tokio::join!(a, b);
            ra.expect("resolver A must tolerate a concurrently removed sibling");
            rb.expect("resolver B must tolerate a concurrently removed sibling");
        }

        // Whatever the interleaving, the folder move is never lost.
        let final_state = load_user_state_from_dir(&book).await.unwrap();
        assert_eq!(final_state.folder_path, vec!["Shelf".to_string()]);
    }

    // Legacy bare-BookReadingState files (pre-folderPath) still load, and an
    // empty state.json yields the default without error.
    #[tokio::test]
    async fn parse_handles_legacy_and_empty() {
        assert_eq!(parse_user_state(""), Some(BookUserState::default()));
        assert_eq!(
            parse_user_state("   \n"),
            Some(BookUserState::default())
        );

        let legacy = parse_user_state(r#"{"chapterId":3,"paragraphId":9}"#)
            .expect("legacy reading state parses");
        assert_eq!(legacy.reading_state.map(|s| s.chapter_id), Some(3));
        assert!(legacy.folder_path.is_empty());

        // Corrupt content parses to None (skipped in the merge, never `?`-fatal).
        assert_eq!(parse_user_state("{not json"), None);
    }
}
