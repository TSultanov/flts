#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"

required=(
  "$repo_root/library/src/tla_trace.rs"
  "$repo_root/library/tests/trace_harness.rs"
  "$repo_root/spec/instrumentation-spec.md"
)

for path in "${required[@]}"; do
  if [[ ! -f "$path" ]]; then
    echo "missing required instrumented file: $path" >&2
    exit 1
  fi
done

echo "FLTS trace instrumentation is checked into the working tree; no patch application needed."
