#!/usr/bin/env python3
"""Preprocess per-task NDJSON trace files into a single JSON for TLC trace validation.

Reads trace-task-*.ndjson files from a directory and produces:
  1. A merged JSON file with lock names mapped to TLA+ constants.
  2. A suggested Trace.cfg snippet with the constant sets.

Lock name mapping:
  "book:<uuid>"    -> b1, b2, ...  (BookLock)
  "trans:eng_fra"  -> tr1, tr2, ... (TransLock)
  "dict:eng_fra"   -> d1, d2, ...   (DictLock)

Output JSON format:
{
    "t1": [ {"event": "Acq", "lock": "b1", "start": N, "end": N}, ... ],
    "t2": [ ... ],
    ...
}

Usage:
    python3 preprocess.py <trace_dir> [--output <path>]

Example:
    python3 preprocess.py /tmp/flts_mutex_traces/concurrent_save_list
    python3 preprocess.py /tmp/flts_mutex_traces/double_lock --output ../../traces/mutex_trace.json
"""

import json
import sys
import os
import glob
import argparse
from collections import OrderedDict


def load_ndjson(path: str) -> list[dict]:
    """Load an NDJSON file, returning a list of event dicts."""
    events = []
    with open(path, "r") as f:
        for line_num, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            try:
                obj = json.loads(line)
                events.append(obj)
            except json.JSONDecodeError as e:
                print(f"  WARNING: {path}:{line_num}: {e}", file=sys.stderr)
    return events


def discover_locks(merged: dict[str, list[dict]]) -> dict[str, str]:
    """Discover all unique lock names and assign TLA+ constants.

    Returns a mapping from raw lock name to TLA+ constant, e.g.:
      "book:abc-123"  -> "b1"
      "trans:eng_fra"  -> "tr1"
      "dict:eng_fra"   -> "d1"
    """
    books: list[str] = []
    trans: list[str] = []
    dicts: list[str] = []

    for events in merged.values():
        for e in events:
            lock = e.get("lock", "")
            if lock.startswith("book:") and lock not in books:
                books.append(lock)
            elif lock.startswith("trans:") and lock not in trans:
                trans.append(lock)
            elif lock.startswith("dict:") and lock not in dicts:
                dicts.append(lock)

    mapping = {}
    for i, b in enumerate(books, 1):
        mapping[b] = f"b{i}"
    for i, t in enumerate(trans, 1):
        mapping[t] = f"tr{i}"
    for i, d in enumerate(dicts, 1):
        mapping[d] = f"d{i}"

    return mapping


def apply_lock_mapping(merged: dict[str, list[dict]], mapping: dict[str, str]) -> dict[str, list[dict]]:
    """Replace raw lock names with TLA+ constants in all events."""
    for events in merged.values():
        for e in events:
            raw = e.get("lock", "")
            if raw in mapping:
                e["lock"] = mapping[raw]
    return merged


def strip_fields(events: list[dict], fields: list[str]) -> list[dict]:
    """Remove specified fields from events (tag, state — not needed by TLC)."""
    for e in events:
        for f in fields:
            e.pop(f, None)
    return events


def compress_timestamps(merged: dict[str, list[dict]]) -> dict[str, list[dict]]:
    """Compress nanosecond timestamps to smaller values.

    Finds the global minimum timestamp and subtracts it from all start/end
    values. This keeps intervals comparable while avoiding TLC integer overflow.
    """
    global_min = float("inf")
    for events in merged.values():
        for e in events:
            if "start" in e:
                global_min = min(global_min, e["start"])
            if "end" in e:
                global_min = min(global_min, e["end"])

    if global_min == float("inf"):
        return merged

    for events in merged.values():
        for e in events:
            if "start" in e:
                e["start"] = e["start"] - global_min
            if "end" in e:
                e["end"] = e["end"] - global_min

    return merged


def validate_events(merged: dict[str, list[dict]]) -> list[str]:
    """Basic validation of trace events. Returns list of warnings."""
    warnings = []
    for task_id, events in merged.items():
        for i, e in enumerate(events):
            if "event" not in e:
                warnings.append(f"{task_id}[{i}]: missing 'event' field")
            if "lock" not in e:
                warnings.append(f"{task_id}[{i}]: missing 'lock' field")
            if e.get("event") not in ("Acq", "Rel"):
                warnings.append(f"{task_id}[{i}]: unknown event '{e.get('event')}'")
            if "start" not in e or "end" not in e:
                warnings.append(f"{task_id}[{i}]: missing start/end timestamp")
            elif e["start"] > e["end"]:
                warnings.append(
                    f"{task_id}[{i}]: start ({e['start']}) > end ({e['end']})"
                )

        # Check ordering within a task
        for i in range(1, len(events)):
            if events[i]["start"] < events[i - 1]["start"]:
                warnings.append(
                    f"{task_id}[{i}]: out-of-order (start {events[i]['start']} < prev start {events[i-1]['start']})"
                )

    return warnings


def generate_cfg_snippet(task_ids: list[str], mapping: dict[str, str]) -> str:
    """Generate a Trace.cfg CONSTANTS snippet from the lock mapping."""
    books = sorted(v for k, v in mapping.items() if k.startswith("book:"))
    trans = sorted(v for k, v in mapping.items() if k.startswith("trans:"))
    dicts = sorted(v for k, v in mapping.items() if k.startswith("dict:"))

    lines = ["CONSTANTS"]
    lines.append(f"    Task = {{{', '.join(sorted(task_ids))}}}")
    lines.append(f"    BookLock = {{{', '.join(books)}}}")
    lines.append(f"    TransLock = {{{', '.join(trans)}}}")
    lines.append(f"    DictLock = {{{', '.join(dicts)}}}")

    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Preprocess mutex trace NDJSON files")
    parser.add_argument("trace_dir", help="Directory containing trace-task-*.ndjson files")
    parser.add_argument(
        "--output",
        "-o",
        default=None,
        help="Output JSON path (default: ../../traces/mutex_trace.json relative to this script)",
    )
    args = parser.parse_args()

    trace_dir = args.trace_dir
    if not os.path.isdir(trace_dir):
        print(f"ERROR: {trace_dir} is not a directory", file=sys.stderr)
        sys.exit(1)

    # Find trace files
    pattern = os.path.join(trace_dir, "trace-task-*.ndjson")
    files = sorted(glob.glob(pattern))
    if not files:
        print(f"ERROR: no trace-task-*.ndjson files found in {trace_dir}", file=sys.stderr)
        sys.exit(1)

    print(f"Found {len(files)} trace file(s):")

    # Load and merge
    merged: dict[str, list[dict]] = {}
    total_events = 0
    for f in files:
        basename = os.path.basename(f)
        task_id = basename.replace("trace-task-", "").replace(".ndjson", "")

        events = load_ndjson(f)
        events = strip_fields(events, ["tag", "state"])
        merged[task_id] = events
        total_events += len(events)
        print(f"  {basename}: {len(events)} events")

    # Discover and apply lock name mapping
    lock_mapping = discover_locks(merged)
    print(f"\nLock mapping ({len(lock_mapping)} locks):")
    for raw, tla in sorted(lock_mapping.items(), key=lambda x: x[1]):
        print(f"  {raw:40s} -> {tla}")

    merged = apply_lock_mapping(merged, lock_mapping)

    # Compress timestamps
    merged = compress_timestamps(merged)

    # Validate
    warnings = validate_events(merged)
    if warnings:
        print(f"\nValidation warnings ({len(warnings)}):")
        for w in warnings:
            print(f"  {w}")
    else:
        print(f"\nValidation: OK ({total_events} events, {len(merged)} tasks)")

    # Print cfg snippet
    task_ids = sorted(merged.keys())
    cfg_snippet = generate_cfg_snippet(task_ids, lock_mapping)
    print(f"\nTrace.cfg snippet:\n{cfg_snippet}")

    # Output path
    if args.output:
        output_path = args.output
    else:
        script_dir = os.path.dirname(os.path.abspath(__file__))
        output_path = os.path.join(script_dir, "..", "..", "traces", "mutex_trace.json")

    os.makedirs(os.path.dirname(os.path.abspath(output_path)), exist_ok=True)
    with open(output_path, "w") as f:
        json.dump(merged, f, indent=2)

    abs_output = os.path.abspath(output_path)
    print(f"\nWrote {abs_output} ({os.path.getsize(abs_output)} bytes)")
    print(f"Tasks: {task_ids}")


if __name__ == "__main__":
    main()
