# Mutex Trace Harness — Instrumentation Guide

## Overview

This harness instruments the FLTS Rust codebase to emit per-task NDJSON trace events
that validate against the `spec/mutex/Trace.tla` TLA+ specification. It verifies that
real lock acquisition patterns respect mutual exclusion and the lock hierarchy.

## Architecture

```
 Rust tests (trace_mutex_harness.rs)
        │
        ├── tokio tasks with TASK_CTX.scope()
        │       │
        │       └── library code with TracedMutex<T>
        │               │
        │               ├── .lock().await → Acq event (automatic)
        │               └── guard drop   → Rel event (automatic)
        │
        └── write_per_task_traces()  → trace-task-t1.ndjson, ...
                    │
         preprocess.py  → mapped mutex_trace.json
                    │
               TLC Trace.tla + Trace.cfg  → validate
```

## Files

| File | Purpose |
|------|---------|
| `library/src/tla_trace_mutex.rs` | `TracedMutex<T>` wrapper — auto-traces on lock/unlock |
| `library/tests/trace_mutex_harness.rs` | Test scenarios exercising lock patterns |
| `harness/mutex/preprocess.py` | Maps lock names to TLA+ constants, merges per-task NDJSON |
| `harness/mutex/run.sh` | End-to-end: test → preprocess → TLC validate |
| `spec/mutex/Trace.tla` | TLA+ trace validation spec (standalone, named locks) |
| `spec/mutex/Trace.cfg` | TLC config (constants, invariants, properties) |

## Quick Start

```bash
# Run everything:
./harness/mutex/run.sh

# Run specific scenario:
./harness/mutex/run.sh concurrent_save_list

# Skip tests, just preprocess + validate:
./harness/mutex/run.sh --skip-tests
```

## TracedMutex Approach

`TracedMutex<T>` is a drop-in replacement for `tokio::sync::Mutex<T>` that automatically
emits trace events. Each lock instance has a **name** derived from the inner value:

| Type | Lock name | Example |
|------|-----------|---------|
| `LibraryBook` | `book:<uuid>` | `book:abc-123` |
| `LibraryTranslation` | `trans:<src>_<tgt>` | `trans:eng_fra` |
| `LibraryDictionary` | `dict:<src>_<tgt>` | `dict:eng_fra` |

The `TracedLock` trait provides `lock_name()`. For explicit naming, use
`TracedMutex::named(value, "my_lock")`.

Tracing is **zero-cost in production**: events are only emitted when the global
collector is initialized (`tla_trace_mutex::init()`) and a `TASK_CTX` is set on
the current tokio task.

## Scenarios

### 1. `concurrent_save_list` (3 tasks)
- **t1 (saver)**: acquires bookLock, iterates transLock + dictLock (nested), saves
- **t2, t3 (tauri_list)**: wait for bookLock, then exercise double-lock pattern

### 2. `watcher_and_mark` (2 tasks)
- **t1 (watcher)**: bookLock → transLock → dictLock (full hierarchy traversal)
- **t2 (tauri_mark)**: bookLock → transLock → save

### 3. `double_lock` (1 task)
- **t1 (tauri_list)**: isolated `get_or_create_translation` double-lock

## Trace Format

Each NDJSON line (raw from TracedMutex):
```json
{
  "tag": "trace",
  "event": "Acq",
  "lock": "book:abc-123",
  "start": 12345,
  "end": 12400,
  "state": { "book:abc-123": "t1" }
}
```

After preprocessing (lock names mapped, timestamps compressed):
```json
{
  "t1": [ { "event": "Acq", "lock": "b1", "start": 0, "end": 55 }, ... ],
  "t2": [ ... ]
}
```

The preprocessor maps lock names to TLA+ model values:
- `book:abc-123` → `b1` (BookLock)
- `trans:eng_fra` → `tr1` (TransLock)
- `dict:eng_fra` → `d1` (DictLock)

## TLA+ Validation

The Trace.tla spec is **standalone** (doesn't extend base.tla) and validates:

- **MutexExclusivity**: no two tasks hold the same lock simultaneously
- **Lock hierarchy**: book (0) < trans (1) < dict (2) — enforced by AcqLock precondition
- **TraceFullyConsumed**: all events consumed (fails if hierarchy or exclusion violated)

Uses **Category B timebox** with:
- **Per-task cursors**: each task replays its own event sequence
- **ViablePIDs**: partial-order constraint (completed-before-started ordering)
- **Weak fairness**: ensures eventual progress

## Adding New Scenarios

1. Add a `#[tokio::test]` in `trace_mutex_harness.rs`
2. Wrap tokio tasks with `TASK_CTX.scope(TaskCtx { task_id, role })`
3. Call library code — `TracedMutex` traces automatically
4. Call `write_per_task_traces()` at the end
5. Update `Trace.cfg` constants if adding new tasks or lock instances
