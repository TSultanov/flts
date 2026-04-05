#!/usr/bin/env python3
"""Preprocess raw NDJSON traces into the format expected by Trace.tla.

Input:  raw NDJSON file (one event per line, with 'actor', 'start', 'end')
Output: JSON object keyed by actor with arrays of events, timestamps
        compressed to dense integers.

Usage:
    python3 preprocess.py <input.ndjson> <output.json>
"""

import json
import sys
from collections import defaultdict


def main():
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <input.ndjson> <output.json>", file=sys.stderr)
        sys.exit(1)

    input_path, output_path = sys.argv[1], sys.argv[2]

    # Read and filter trace events
    events = []
    with open(input_path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            obj = json.loads(line)
            if obj.get("tag") != "trace":
                continue
            events.append(obj)

    if not events:
        print("WARNING: no trace events found", file=sys.stderr)
        json.dump({}, open(output_path, "w"), indent=2)
        return

    # Collect all timestamps for compression
    all_ts = set()
    for e in events:
        all_ts.add(e["start"])
        all_ts.add(e["end"])

    sorted_ts = sorted(all_ts)
    ts_map = {ts: idx + 1 for idx, ts in enumerate(sorted_ts)}

    # Group by actor, sort by start timestamp
    by_actor = defaultdict(list)
    for e in events:
        actor = e["actor"]
        compressed = dict(e)
        compressed["start"] = ts_map[e["start"]]
        compressed["end"] = ts_map[e["end"]]
        # Remove tag and actor from per-event data (redundant)
        compressed.pop("tag", None)
        compressed.pop("actor", None)
        by_actor[actor].append(compressed)

    for actor in by_actor:
        by_actor[actor].sort(key=lambda e: e["start"])

    # Write output
    result = dict(by_actor)
    with open(output_path, "w") as f:
        json.dump(result, f, indent=2, ensure_ascii=False)

    # Report
    total = sum(len(v) for v in result.values())
    actors = ", ".join(f"{a}({len(result[a])})" for a in sorted(result))
    print(f"Preprocessed {total} events across {len(result)} actors: {actors}")
    print(f"Timestamp range compressed to [1..{len(sorted_ts)}]")


if __name__ == "__main__":
    main()
