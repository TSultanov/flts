# Interaction Trace — Instrumentation Guide

## Overview

The interaction trace harness exercises **real Library/LibraryBook operations**
while emitting NDJSON events for the interaction TLA+ spec (`spec/interaction/`).

Unlike the mutex harness (which instruments production code), this harness
operates at the **test level** — trace events are emitted by the test scenarios
in `library/tests/trace_interaction_harness.rs`, wrapping real Library API calls.

## Architecture

```
┌─────────────────────────────────────┐
│  trace_interaction_harness.rs       │   Test file
│  ├── trace_interaction_baseline     │   Sequential, all 19 events
│  └── trace_interaction_concurrent   │   3 tokio tasks, real overlap
└──────────┬──────────────────────────┘
           │ uses
┌──────────▼──────────────────────────┐
│  library/src/tla_trace_interaction.rs│  Trace emission module
│  ├── InteractionTraceGuard          │   RAII: open/close trace file
│  └── TraceSpan                      │   RAII: begin/end timing
└──────────┬──────────────────────────┘
           │ writes
     *.ndjson files
           │
┌──────────▼──────────────────────────┐
│  harness/interaction/preprocess.py  │   Groups by actor, compresses timestamps
└──────────┬──────────────────────────┘
           │
     *.json files  →  Trace.tla / TLC
```

## Files

| File | Purpose |
|------|---------|
| `library/src/tla_trace_interaction.rs` | Trace emission (global writer, TraceSpan) |
| `library/tests/trace_interaction_harness.rs` | Test scenarios |
| `harness/interaction/preprocess.py` | NDJSON → Trace.tla JSON |
| `harness/interaction/run.sh` | End-to-end runner |
| `spec/interaction/instrumentation-spec.md` | Event-to-source mapping |

## NDJSON Event Schema

Each line is a JSON object with these fields:

```json
{
  "tag": "trace",
  "actor": "t1",
  "event": "BeginWorker",
  "start": 1542125,
  "end": 1607833,
  "task": "t1",
  "book": "b1",
  "lib": 1
}
```

- `tag` — always `"trace"` (filter marker)
- `actor` — which per-actor trace array this event belongs to
- `event` — one of the 19 TLA+ action names
- `start`, `end` — monotonic nanoseconds from trace initialization
- Additional fields vary by event type (see `instrumentation-spec.md`)

## Running

```bash
# Full pipeline
./harness/interaction/run.sh

# Or manually:
FLTS_INTERACTION_TRACE_DIR=./traces/interaction \
    cargo test -q -p library trace_interaction_ -- --test-threads=1

python3 harness/interaction/preprocess.py \
    traces/interaction/interaction-baseline.ndjson \
    traces/interaction/interaction-baseline.json
```

## Adding a New Event Type

1. Add the `Trace*` action in `spec/interaction/Trace.tla`
2. Add a CASE branch in `TraceNextSub` for the new event name
3. In the test harness, wrap the corresponding Library operation with `TraceSpan`:
   ```rust
   let span = TraceSpan::begin("t1", "NewEventName")
       .field("key", "value");
   // ... real Library operation ...
   span.end();
   ```
4. Run `./harness/interaction/run.sh` to verify coverage

## Modifying Event Fields

Fields are passed through `TraceSpan::field()`. To add a field:

```rust
TraceSpan::begin("t1", "WorkerSave")
    .field("task", "t1")
    .field("lib", 1)
    .field("new_field", "new_value")  // added
```

The preprocessor passes all fields through — no changes needed there.
Update `Trace.tla`'s corresponding `Trace*` action to read the new field.

## Event Coverage (19/19)

All 19 TLA+ actions are covered by the baseline scenario:

| # | Event | Actor | Real Operation |
|---|-------|-------|----------------|
| 1 | ConfigChange | t1 | `Library::open` (new instance) |
| 2 | BeginWorker | t1 | `library.get_book(&id)` |
| 3 | WorkerReadParagraph | t1 | `get_or_create_translation` + `paragraph_view` |
| 4 | WorkerCallAPI | t1 | `tokio::time::sleep` (simulated) |
| 5 | WorkerStoreResult | t1 | `add_paragraph_translation` |
| 6 | WorkerSave | t1 | `book.save()` |
| 7 | WorkerComputeSnapshot | t1 | `library.list_books()` |
| 8 | WorkerEmit | t1 | protocol event only |
| 9 | BeginTauri | t2 | protocol routing |
| 10 | TauriModify | t2 | `create_book_plain` |
| 11 | TauriComputeSnapshot | t2 | `library.list_books()` |
| 12 | TauriEmit | t2 | protocol event only |
| 13 | BeginWatcher | t1 | `library.get_book(&id)` |
| 14 | WatcherReload | t1 | fs write + reload |
| 15 | WatcherComputeSnapshot | t1 | `library.list_books()` |
| 16 | WatcherEmit | t1 | protocol event only |
| 17 | DeliverEvent | ui | protocol event only |
| 18 | MarkWordVisible | t2 | `mark_word_visible` + `save` |
| 19 | AppClose | t1 | protocol event only |
