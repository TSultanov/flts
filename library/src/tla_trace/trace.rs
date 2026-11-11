use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use serde_json::json;

#[derive(Default)]
struct TraceSink {
    writer: Option<BufWriter<File>>,
}

static TRACE_SINK: OnceLock<Mutex<TraceSink>> = OnceLock::new();

fn sink() -> &'static Mutex<TraceSink> {
    TRACE_SINK.get_or_init(|| Mutex::new(TraceSink::default()))
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

/// Emits one roster-mesh trace event for the `spec/roster/` spec (Trace.tla).
///
/// Envelope: `{tag:"trace", ts:<nanos>, event:{name, node, target?, src?,
/// roster:{<id>:{add:<vclock>, rem:<vclock>}}, engine:[peer ids]}}`, where
/// `roster` is this node's post-state and `engine` its peer set.
pub fn emit_roster_event(
    name: &str,
    node: &str,
    target: Option<&str>,
    src: Option<&str>,
    roster: serde_json::Value,
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
    event.insert("roster".into(), roster);
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
