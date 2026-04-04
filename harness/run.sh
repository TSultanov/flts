#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
trace_dir="$repo_root/traces"

bash "$repo_root/harness/apply.sh"
rm -rf "$trace_dir"
mkdir -p "$trace_dir"

export FLTS_TRACE_DIR="$trace_dir"

cd "$repo_root"
cargo test -q -p library trace_ -- --test-threads=1

python3 - <<'PY'
import json
import pathlib
import sys

root = pathlib.Path("traces")
expected = {
    "LoadBookFromMetadata",
    "SaveBookBegin",
    "SaveBookFinish",
    "UpdateReadingStateReload",
    "UpdateReadingStatePersist",
    "UpdateFolderPathReload",
    "UpdateFolderPathPersist",
    "ResolveReadingStateFile",
    "LoadTranslationFromMetadata",
    "SaveTranslationBegin",
    "SaveTranslationFinish",
    "LoadDictionaryFromMetadata",
}

seen = set()
for path in sorted(root.glob("*.ndjson")):
    lines = path.read_text().splitlines()
    if not lines:
        raise SystemExit(f"{path} is empty")
    for idx, line in enumerate(lines, start=1):
        try:
            data = json.loads(line)
        except json.JSONDecodeError as exc:
            raise SystemExit(f"{path}:{idx}: invalid JSON: {exc}") from exc
        if data.get("tag") != "trace":
            raise SystemExit(f"{path}:{idx}: missing trace tag")
        event = data.get("event", {})
        name = event.get("name")
        if not name:
            raise SystemExit(f"{path}:{idx}: missing event name")
        seen.add(name)
    print(f"{path.name}: {len(lines)} trace event(s)")

missing = sorted(expected - seen)
if missing:
    raise SystemExit("missing expected event types: " + ", ".join(missing))

print("event coverage: ok")
PY
