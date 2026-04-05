use std::{
    collections::HashMap,
    io::Write,
    ops::{Deref, DerefMut},
    path::Path,
    sync::{Mutex as StdMutex, OnceLock},
    time::Instant,
};

use serde::Serialize;

// ---------------------------------------------------------------------------
// Task-local context (set by test scenarios, absent in production)
// ---------------------------------------------------------------------------

tokio::task_local! {
    pub static TASK_CTX: TaskCtx;
}

#[derive(Clone, Debug)]
pub struct TaskCtx {
    pub task_id: String,
    pub role: String,
}

// ---------------------------------------------------------------------------
// TracedLock — implement on types to supply a descriptive lock name
// ---------------------------------------------------------------------------

/// Implement on types wrapped by `TracedMutex` to auto-derive a lock name.
///
/// The name should be a stable, human-readable identifier like
/// `"book:abc-123"` or `"trans:eng_fra"`. It appears in trace events
/// and the TLA+ spec maps it to a constant.
pub trait TracedLock {
    fn lock_name(&self) -> String;
}

// ---------------------------------------------------------------------------
// TracedMutex — drop-in replacement for tokio::sync::Mutex with tracing
// ---------------------------------------------------------------------------

pub struct TracedMutex<T> {
    inner: tokio::sync::Mutex<T>,
    name: StdMutex<String>,
}

impl<T: TracedLock> TracedMutex<T> {
    /// Create a new TracedMutex, deriving the lock name from the inner value.
    pub fn new(value: T) -> Self {
        let name = value.lock_name();
        Self {
            inner: tokio::sync::Mutex::new(value),
            name: StdMutex::new(name),
        }
    }
}

impl<T> TracedMutex<T> {
    /// Create a TracedMutex with an explicit name (no TracedLock required).
    pub fn named(value: T, name: impl Into<String>) -> Self {
        Self {
            inner: tokio::sync::Mutex::new(value),
            name: StdMutex::new(name.into()),
        }
    }

    /// Override the lock name after construction.
    pub fn set_name(&self, name: impl Into<String>) {
        *self.name.lock().unwrap() = name.into();
    }

    /// Read the current lock name.
    pub fn name(&self) -> String {
        self.name.lock().unwrap().clone()
    }

    pub async fn lock(&self) -> TracedMutexGuard<'_, T> {
        let start = now_ns();
        let guard = self.inner.lock().await;
        let end = now_ns();
        let name = self.name();

        emit_lock_event("Acq", &name, start, end);
        set_holder(&name);

        TracedMutexGuard {
            inner: guard,
            name,
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for TracedMutex<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TracedMutex")
            .field("name", &*self.name.lock().unwrap())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// TracedMutexGuard — auto-emits release event on Drop
// ---------------------------------------------------------------------------

pub struct TracedMutexGuard<'a, T> {
    inner: tokio::sync::MutexGuard<'a, T>,
    name: String,
}

impl<T> Deref for TracedMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> DerefMut for TracedMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T> Drop for TracedMutexGuard<'_, T> {
    fn drop(&mut self) {
        clear_holder(&self.name);
        emit_lock_event_instant("Rel", &self.name);
    }
}

// ---------------------------------------------------------------------------
// Global collector (initialized once per test run)
// ---------------------------------------------------------------------------

static COLLECTOR: OnceLock<TraceCollector> = OnceLock::new();

struct TraceCollector {
    start: Instant,
    events: StdMutex<HashMap<String, Vec<TraceEvent>>>,
    /// lock name → task_id that currently holds it
    holders: StdMutex<HashMap<String, String>>,
}

#[derive(Serialize, Clone, Debug)]
struct TraceEvent {
    tag: &'static str,
    event: String,
    lock: String,
    start: u64,
    end: u64,
    /// Snapshot of all held locks at event time: { "lock_name": "task_id", ... }
    state: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the trace collector. Call once at the start of each test run.
pub fn init() {
    COLLECTOR.get_or_init(|| TraceCollector {
        start: Instant::now(),
        events: StdMutex::new(HashMap::new()),
        holders: StdMutex::new(HashMap::new()),
    });
}

/// Reset the collector (for running multiple test scenarios).
pub fn reset() {
    if let Some(c) = COLLECTOR.get() {
        c.events.lock().unwrap().clear();
        c.holders.lock().unwrap().clear();
    }
}

/// Current nanosecond timestamp relative to collector start.
pub fn now_ns() -> u64 {
    COLLECTOR
        .get()
        .map(|c| (Instant::now() - c.start).as_nanos() as u64)
        .unwrap_or(0)
}

/// Write per-task NDJSON trace files to the given directory.
pub fn write_per_task_traces(dir: &Path) -> std::io::Result<()> {
    let Some(c) = COLLECTOR.get() else {
        return Ok(());
    };
    std::fs::create_dir_all(dir)?;

    let events = c.events.lock().unwrap();
    for (task_id, task_events) in events.iter() {
        let path = dir.join(format!("trace-task-{}.ndjson", task_id));
        let mut file = std::fs::File::create(&path)?;
        for event in task_events {
            serde_json::to_writer(&mut file, event).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::Other, e)
            })?;
            file.write_all(b"\n")?;
        }
        file.flush()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn emit_lock_event(event: &str, lock_name: &str, start: u64, end: u64) {
    let Some(c) = COLLECTOR.get() else { return };
    let Ok(ctx) = TASK_CTX.try_with(|c| c.clone()) else {
        return;
    };

    let state = c.holders.lock().unwrap().clone();

    let trace_event = TraceEvent {
        tag: "trace",
        event: event.to_string(),
        lock: lock_name.to_string(),
        start,
        end,
        state,
    };

    c.events
        .lock()
        .unwrap()
        .entry(ctx.task_id.clone())
        .or_default()
        .push(trace_event);
}

fn emit_lock_event_instant(event: &str, lock_name: &str) {
    let ns = now_ns();
    emit_lock_event(event, lock_name, ns, ns);
}

fn set_holder(lock_name: &str) {
    let Some(c) = COLLECTOR.get() else { return };
    let Ok(ctx) = TASK_CTX.try_with(|c| c.clone()) else {
        return;
    };
    c.holders
        .lock()
        .unwrap()
        .insert(lock_name.to_string(), ctx.task_id.clone());
}

fn clear_holder(lock_name: &str) {
    let Some(c) = COLLECTOR.get() else { return };
    if TASK_CTX.try_with(|_| ()).is_err() {
        return;
    };
    c.holders.lock().unwrap().remove(lock_name);
}
