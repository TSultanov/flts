//! TLA+ trace emission for the **interaction** spec (`spec/interaction/`).
//!
//! Records NDJSON events with `[start, end]` timestamps around each operation
//! for partial-order (ViablePIDs) replay in TLC.
//!
//! # Usage (in test harnesses)
//!
//! ```ignore
//! tla_trace_interaction::init(&path)?;
//!
//! let span = TraceSpan::begin("t1", "BeginWorker")
//!     .field("task", "t1")
//!     .field("book", "b1")
//!     .field("lib", 1);
//! // ... real operation ...
//! span.end();
//!
//! tla_trace_interaction::close()?;
//! ```

use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
    sync::{Mutex, OnceLock},
    time::Instant,
};

use anyhow::Context;
use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// Global writer
// ---------------------------------------------------------------------------

struct TraceSink {
    writer: Option<BufWriter<File>>,
    epoch: Instant,
}

impl Default for TraceSink {
    fn default() -> Self {
        Self {
            writer: None,
            epoch: Instant::now(),
        }
    }
}

static SINK: OnceLock<Mutex<TraceSink>> = OnceLock::new();

fn sink() -> &'static Mutex<TraceSink> {
    SINK.get_or_init(|| Mutex::new(TraceSink::default()))
}

/// Open (or replace) the trace output file.
pub fn init(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating trace dir for {}", path.display()))?;
    }
    let file =
        File::create(path).with_context(|| format!("creating trace file {}", path.display()))?;
    let mut guard = sink().lock().unwrap();
    guard.writer = Some(BufWriter::new(file));
    guard.epoch = Instant::now();
    Ok(())
}

/// Flush and close the current trace file.
pub fn close() -> anyhow::Result<()> {
    let mut guard = sink().lock().unwrap();
    if let Some(writer) = guard.writer.as_mut() {
        writer.flush().context("flushing interaction trace")?;
    }
    guard.writer = None;
    Ok(())
}

/// Returns elapsed nanoseconds since trace init (monotonic).
fn now_ns() -> u64 {
    sink()
        .lock()
        .unwrap()
        .epoch
        .elapsed()
        .as_nanos() as u64
}

/// Write one NDJSON line to the trace file.
fn emit_line(obj: &Value) {
    let mut guard = sink().lock().unwrap();
    let Some(writer) = guard.writer.as_mut() else {
        return;
    };
    serde_json::to_writer(&mut *writer, obj).ok();
    let _ = writer.write_all(b"\n");
    let _ = writer.flush();
}

// ---------------------------------------------------------------------------
// TraceSpan — RAII guard for timed events
// ---------------------------------------------------------------------------

/// Records `[start, end]` around an operation and emits one NDJSON line on
/// [`TraceSpan::end`].
///
/// If dropped without calling `end()`, the event is silently discarded.
pub struct TraceSpan {
    actor: String,
    event: String,
    start: u64,
    fields: Map<String, Value>,
    finished: bool,
}

impl TraceSpan {
    /// Mark the beginning of an operation. Call [`end`] after the real code runs.
    pub fn begin(actor: &str, event: &str) -> Self {
        Self {
            actor: actor.into(),
            event: event.into(),
            start: now_ns(),
            fields: Map::new(),
            finished: false,
        }
    }

    /// Attach a field to the event envelope.
    pub fn field(mut self, key: &str, val: impl Into<Value>) -> Self {
        self.fields.insert(key.into(), val.into());
        self
    }

    /// Finish the span: record `end` timestamp and emit the NDJSON line.
    pub fn end(mut self) {
        self.finished = true;
        let end = now_ns();

        let mut obj = Map::new();
        obj.insert("tag".into(), Value::String("trace".into()));
        obj.insert("actor".into(), Value::String(self.actor.clone()));
        obj.insert("event".into(), Value::String(self.event.clone()));
        obj.insert("start".into(), Value::Number(self.start.into()));
        obj.insert("end".into(), Value::Number(end.into()));
        for (k, v) in &self.fields {
            obj.insert(k.clone(), v.clone());
        }

        emit_line(&Value::Object(obj));
    }
}

// ---------------------------------------------------------------------------
// Test guard (RAII)
// ---------------------------------------------------------------------------

/// RAII guard that opens a trace file on creation and closes it on drop.
pub struct InteractionTraceGuard;

impl InteractionTraceGuard {
    /// Open a trace file for the given scenario name.
    ///
    /// Respects `FLTS_INTERACTION_TRACE_DIR` env var; falls back to a temp dir.
    pub fn start(filename: &str) -> Self {
        let root = std::env::var_os("FLTS_INTERACTION_TRACE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!(
                    "flts_interaction_trace_{}",
                    uuid::Uuid::new_v4()
                ))
            });
        std::fs::create_dir_all(&root).unwrap();
        init(&root.join(filename)).unwrap();
        Self
    }
}

impl Drop for InteractionTraceGuard {
    fn drop(&mut self) {
        close().unwrap();
    }
}
