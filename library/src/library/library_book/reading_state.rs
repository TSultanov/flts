use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
            let metadata = entry.metadata().await?;
            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            files.push((path, modified));
        }
    }
    Ok(files)
}

async fn resolve_reading_state_file(path: &Path) -> anyhow::Result<Option<(PathBuf, SystemTime)>> {
    let mut candidates = reading_state_files(path).await?;
    if candidates.is_empty() {
        return Ok(None);
    }

    candidates.sort_by(|a, b| a.1.cmp(&b.1));
    let (latest_path, latest_modified) = candidates
        .last()
        .cloned()
        .unwrap_or_else(|| unreachable!("candidates is not empty"));

    let canonical_path = path.join("state.json");
    let canonical_name = canonical_path.file_name().unwrap();
    let mut effective_modified = latest_modified;

    if latest_path.file_name().unwrap() != canonical_name {
        if tokio::fs::try_exists(&canonical_path).await? {
            tokio::fs::remove_file(&canonical_path).await?;
        }
        tokio::fs::rename(&latest_path, &canonical_path).await?;
        effective_modified = tokio::fs::metadata(&canonical_path)
            .await?
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);
    }

    let had_multiple_candidates = candidates.len() > 1;
    for (candidate_path, _) in &candidates {
        if candidate_path.file_name().unwrap() != canonical_name
            && tokio::fs::try_exists(&candidate_path).await?
        {
            let _ = tokio::fs::remove_file(candidate_path).await;
        }
    }

    if latest_path.file_name().unwrap() != canonical_name || had_multiple_candidates {
        tla_trace::emit_book_event(
            path,
            "ResolveReadingStateFile",
            None,
            "idle",
            "idle",
            "idle",
        )
        .await?;
    }

    Ok(Some((canonical_path, effective_modified)))
}

pub(super) async fn load_user_state_from_dir(path: &Path) -> anyhow::Result<BookUserState> {
    if let Some((state_path, _)) = resolve_reading_state_file(path).await? {
        let mut file = tokio::fs::File::open(&state_path).await?;
        let mut contents = String::new();
        file.read_to_string(&mut contents).await?;

        if contents.trim().is_empty() {
            return Ok(BookUserState::default());
        }

        let value: serde_json::Value = serde_json::from_str(&contents)?;
        if value.get("readingState").is_some() || value.get("folderPath").is_some() {
            return Ok(serde_json::from_value(value)?);
        }

        let legacy: BookReadingState = serde_json::from_value(value)?;
        return Ok(BookUserState {
            reading_state: Some(legacy),
            ..BookUserState::default()
        });
    }

    Ok(BookUserState::default())
}

pub(super) async fn persist_user_state(path: &Path, state: &BookUserState) -> anyhow::Result<()> {
    if !tokio::fs::try_exists(path).await? {
        tokio::fs::create_dir_all(path).await?;
    }

    let state_path = path.join("state.json");
    let temp_path = path.join(format!("state.json~{}", create_random_string(8)));

    {
        let mut file = tokio::fs::File::create(&temp_path).await?;
        let content = serde_json::to_vec_pretty(state)?;
        file.write_all(&content).await?;
    }

    if tokio::fs::try_exists(&state_path).await? {
        tokio::fs::remove_file(&state_path).await?;
    }
    tokio::fs::rename(&temp_path, &state_path).await?;

    Ok(())
}

pub async fn load_book_user_state(path: &Path) -> anyhow::Result<BookUserState> {
    load_user_state_from_dir(path).await
}
