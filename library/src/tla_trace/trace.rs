use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use serde::Serialize;
use serde_json::json;

use crate::book::{serialization::Serializable, translation::Translation};

#[derive(Default)]
struct TraceSink {
    writer: Option<BufWriter<File>>,
}

static TRACE_SINK: OnceLock<Mutex<TraceSink>> = OnceLock::new();

fn sink() -> &'static Mutex<TraceSink> {
    TRACE_SINK.get_or_init(|| Mutex::new(TraceSink::default()))
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TraceArg {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reading: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

#[derive(Debug, Serialize)]
struct TraceEnvelope<'a> {
    tag: &'static str,
    ts: String,
    event: TraceEvent<'a>,
}

#[derive(Debug, Serialize)]
struct TraceEvent<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    arg: Option<TraceArg>,
    state: TraceState,
}

#[derive(Debug, Serialize)]
struct TraceState {
    #[serde(rename = "bookMainMTime")]
    book_main_m_time: u64,
    #[serde(rename = "bookConflictCount")]
    book_conflict_count: usize,
    #[serde(rename = "stateMainReading")]
    state_main_reading: String,
    #[serde(rename = "stateMainFolder")]
    state_main_folder: String,
    #[serde(rename = "stateConflictCount")]
    state_conflict_count: usize,
    #[serde(rename = "translationMainMTime")]
    translation_main_m_time: u64,
    #[serde(rename = "translationVersionCount")]
    translation_version_count: usize,
    #[serde(rename = "translationConflictCount")]
    translation_conflict_count: usize,
    #[serde(rename = "bookSaveStage")]
    book_save_stage: &'static str,
    #[serde(rename = "translationSaveStage")]
    translation_save_stage: &'static str,
    #[serde(rename = "stateOpKind")]
    state_op_kind: &'static str,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
struct TraceBookUserState {
    #[serde(default, rename = "readingState")]
    reading_state: Option<TraceReadingState>,
    #[serde(default, rename = "folderPath")]
    folder_path: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct TraceReadingState {
    #[serde(alias = "chapterId")]
    chapter_id: usize,
    #[serde(alias = "paragraphId")]
    paragraph_id: usize,
}

pub fn set_trace_file(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating trace dir for {}", path.display()))?;
    }

    let file =
        File::create(path).with_context(|| format!("creating trace file {}", path.display()))?;
    let mut guard = sink().lock().unwrap();
    guard.writer = Some(BufWriter::new(file));
    Ok(())
}

pub fn clear_trace_file() -> anyhow::Result<()> {
    let mut guard = sink().lock().unwrap();
    if let Some(writer) = guard.writer.as_mut() {
        writer.flush().context("flushing trace file")?;
    }
    guard.writer = None;
    Ok(())
}

pub async fn emit_book_event(
    book_dir: &Path,
    name: &str,
    arg: Option<TraceArg>,
    book_save_stage: &'static str,
    translation_save_stage: &'static str,
    state_op_kind: &'static str,
) -> anyhow::Result<()> {
    let library_root = book_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| book_dir.to_path_buf());
    let state = capture_state(
        Some(book_dir),
        &library_root,
        None,
        book_save_stage,
        translation_save_stage,
        state_op_kind,
    )
    .await?;
    emit(name, arg, state)
}

pub async fn emit_translation_event(
    book_dir: &Path,
    translation_path: &Path,
    name: &str,
    arg: Option<TraceArg>,
    book_save_stage: &'static str,
    translation_save_stage: &'static str,
    state_op_kind: &'static str,
) -> anyhow::Result<()> {
    let library_root = book_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| book_dir.to_path_buf());
    let state = capture_state(
        Some(book_dir),
        &library_root,
        Some(translation_path),
        book_save_stage,
        translation_save_stage,
        state_op_kind,
    )
    .await?;
    emit(name, arg, state)
}

/// Emits one roster-mesh trace event for the `spec/roster/` spec (Trace.tla).
///
/// Envelope: `{tag:"trace", ts:<nanos>, event:{name, node, target?, src?, ts,
/// roster:{active:{id->addedAtMs}, tomb:{id->removedAtMs}}, engine:[peer ids]}}`.
/// `ts` is the operation's own millisecond stamp (addedAtMs/removedAtMs, or 0 for
/// sync/reconcile); `active`/`tomb`/`engine` are this node's POST-state.
pub fn emit_roster_event(
    name: &str,
    node: &str,
    target: Option<&str>,
    src: Option<&str>,
    ts: u64,
    active: &BTreeMap<String, u64>,
    tomb: &BTreeMap<String, u64>,
    engine: &[String],
) -> anyhow::Result<()> {
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string();

    let mut event = serde_json::Map::new();
    event.insert("name".into(), json!(name));
    event.insert("node".into(), json!(node));
    if let Some(t) = target {
        event.insert("target".into(), json!(t));
    }
    if let Some(s) = src {
        event.insert("src".into(), json!(s));
    }
    event.insert("ts".into(), json!(ts));
    event.insert("roster".into(), json!({ "active": active, "tomb": tomb }));
    event.insert("engine".into(), json!(engine));

    let envelope = json!({
        "tag": "trace",
        "ts": now_ns,
        "event": serde_json::Value::Object(event),
    });

    let mut guard = sink().lock().unwrap();
    let Some(writer) = guard.writer.as_mut() else {
        return Ok(());
    };
    serde_json::to_writer(&mut *writer, &envelope).context("serializing roster trace event")?;
    writer.write_all(b"\n").context("writing trace newline")?;
    writer.flush().context("flushing trace event")?;
    Ok(())
}

fn emit(name: &str, arg: Option<TraceArg>, state: TraceState) -> anyhow::Result<()> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string();

    let envelope = TraceEnvelope {
        tag: "trace",
        ts,
        event: TraceEvent { name, arg, state },
    };

    let mut guard = sink().lock().unwrap();
    let Some(writer) = guard.writer.as_mut() else {
        return Ok(());
    };

    serde_json::to_writer(&mut *writer, &envelope).context("serializing trace event")?;
    writer.write_all(b"\n").context("writing trace newline")?;
    writer.flush().context("flushing trace event")?;
    Ok(())
}

async fn capture_state(
    book_dir: Option<&Path>,
    _library_root: &Path,
    translation_path_hint: Option<&Path>,
    book_save_stage: &'static str,
    translation_save_stage: &'static str,
    state_op_kind: &'static str,
) -> anyhow::Result<TraceState> {
    let (
        book_main_m_time,
        book_conflict_count,
        state_main_reading,
        state_main_folder,
        state_conflict_count,
    ) = if let Some(book_dir) = book_dir {
        let book_main_m_time = file_mtime_millis(&book_dir.join("book.dat")).await?;
        let book_conflict_count = count_matching_files(book_dir, |name| {
            name.starts_with("book") && name.ends_with(".dat") && name != "book.dat"
        })
        .await?;
        let (state_main_reading, state_main_folder) = read_canonical_state(book_dir).await?;
        let state_conflict_count = count_matching_files(book_dir, |name| {
            name.starts_with("state") && name.ends_with(".json") && name != "state.json"
        })
        .await?;
        (
            book_main_m_time,
            book_conflict_count,
            state_main_reading,
            state_main_folder,
            state_conflict_count,
        )
    } else {
        (0, 0, "nil".to_string(), "nil".to_string(), 0)
    };

    let (translation_main_m_time, translation_version_count, translation_conflict_count) =
        if let Some(book_dir) = book_dir {
            let translation_path = match translation_path_hint {
                Some(path) if path.parent() == Some(book_dir) => Some(path.to_path_buf()),
                _ => first_canonical_translation_path(book_dir).await?,
            };

            if let Some(translation_path) = translation_path {
                let translation_main_m_time = file_mtime_millis(&translation_path).await?;
                let translation_conflict_count = count_matching_files(book_dir, |name| {
                    name.starts_with("translation_")
                        && name.ends_with(".dat")
                        && !is_canonical_translation_filename(name)
                })
                .await?;
                let translation_version_count = if tokio::fs::try_exists(&translation_path).await? {
                    load_translation(&translation_path).await?.version_count()
                } else {
                    0
                };
                (
                    translation_main_m_time,
                    translation_version_count,
                    translation_conflict_count,
                )
            } else {
                (0, 0, 0)
            }
        } else {
            (0, 0, 0)
        };

    Ok(TraceState {
        book_main_m_time,
        book_conflict_count,
        state_main_reading,
        state_main_folder,
        state_conflict_count,
        translation_main_m_time,
        translation_version_count,
        translation_conflict_count,
        book_save_stage,
        translation_save_stage,
        state_op_kind,
    })
}

async fn file_mtime_millis(path: &Path) -> anyhow::Result<u64> {
    if !tokio::fs::try_exists(path).await? {
        return Ok(0);
    }
    let modified = tokio::fs::metadata(path)
        .await?
        .modified()
        .unwrap_or(UNIX_EPOCH);
    Ok(modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64)
}

async fn count_matching_files<F>(dir: &Path, mut predicate: F) -> anyhow::Result<usize>
where
    F: FnMut(&str) -> bool,
{
    if !tokio::fs::try_exists(dir).await? {
        return Ok(0);
    }
    let mut count = 0usize;
    let mut read_dir = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = read_dir.next_entry().await? {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && predicate(name)
        {
            count += 1;
        }
    }
    Ok(count)
}

async fn first_canonical_translation_path(book_dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    let mut read_dir = match tokio::fs::read_dir(book_dir).await {
        Ok(read_dir) => read_dir,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };

    let mut candidates = Vec::new();
    while let Some(entry) = read_dir.next_entry().await? {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && is_canonical_translation_filename(name)
        {
            candidates.push(path);
        }
    }
    candidates.sort();
    Ok(candidates.into_iter().next())
}

fn is_canonical_translation_filename(name: &str) -> bool {
    if !(name.starts_with("translation_") && name.ends_with(".dat")) {
        return false;
    }
    let stem = &name[..name.len() - 4];
    !stem.contains('.') && stem.matches('_').count() == 2
}

async fn read_canonical_state(book_dir: &Path) -> anyhow::Result<(String, String)> {
    let state_path = book_dir.join("state.json");
    if !tokio::fs::try_exists(&state_path).await? {
        return Ok(("nil".to_string(), "nil".to_string()));
    }

    let content = tokio::fs::read_to_string(&state_path).await?;
    if content.trim().is_empty() {
        return Ok(("nil".to_string(), "nil".to_string()));
    }

    let value: serde_json::Value = serde_json::from_str(&content)?;
    let state = if value.get("readingState").is_some() || value.get("folderPath").is_some() {
        serde_json::from_value::<TraceBookUserState>(value)?
    } else {
        let legacy = serde_json::from_value::<TraceReadingState>(value)?;
        TraceBookUserState {
            reading_state: Some(legacy),
            ..TraceBookUserState::default()
        }
    };

    let reading = state
        .reading_state
        .map(|reading| format!("{}:{}", reading.chapter_id, reading.paragraph_id))
        .unwrap_or_else(|| "nil".to_string());
    let folder = if state.folder_path.is_empty() {
        "nil".to_string()
    } else {
        state.folder_path.join("/")
    };

    Ok((reading, folder))
}

async fn load_translation(path: &Path) -> anyhow::Result<Translation> {
    let content = tokio::fs::read(path).await?;
    let mut cursor = std::io::Cursor::new(content);
    Ok(Translation::deserialize(&mut cursor)?)
}
