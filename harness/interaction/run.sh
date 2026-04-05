#!/usr/bin/env bash
# Run the interaction trace harness end-to-end:
#   1. Build & run Rust tests to produce NDJSON traces
#   2. Preprocess each trace into Trace.tla-compatible JSON
#   3. Verify event coverage (all 19 types present)
#
# Usage:
#   ./harness/interaction/run.sh              # defaults to ./traces/interaction/
#   TRACE_DIR=/my/dir ./harness/interaction/run.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TRACE_DIR="${TRACE_DIR:-${REPO_ROOT}/traces/interaction}"
SPEC_DIR="${REPO_ROOT}/spec/interaction"
PREPROCESS="${REPO_ROOT}/harness/interaction/preprocess.py"

# All 19 event types from the spec
EXPECTED_EVENTS=(
    AppClose BeginTauri BeginWatcher BeginWorker ConfigChange
    DeliverEvent MarkWordVisible
    TauriComputeSnapshot TauriEmit TauriModify
    WatcherComputeSnapshot WatcherEmit WatcherReload
    WorkerCallAPI WorkerComputeSnapshot WorkerEmit
    WorkerReadParagraph WorkerSave WorkerStoreResult
)

echo "=== Interaction Trace Harness ==="
echo "Trace output: ${TRACE_DIR}"
echo ""

# 1. Run tests
echo "--- Step 1: Run trace tests ---"
mkdir -p "${TRACE_DIR}"
FLTS_INTERACTION_TRACE_DIR="${TRACE_DIR}" \
    cargo test -q -p library trace_interaction_ -- --test-threads=1
echo ""

# 2. Preprocess
echo "--- Step 2: Preprocess traces ---"
for ndjson in "${TRACE_DIR}"/*.ndjson; do
    name="$(basename "${ndjson}" .ndjson)"
    out="${TRACE_DIR}/${name}.json"
    echo "  ${name}.ndjson → ${name}.json"
    python3 "${PREPROCESS}" "${ndjson}" "${out}"
done
echo ""

# 3. Verify event coverage
echo "--- Step 3: Verify event coverage ---"
MISSING=()
ALL_EVENTS=$(cat "${TRACE_DIR}"/*.ndjson | python3 -c "
import sys, json
events = set()
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    obj = json.loads(line)
    if obj.get('tag') == 'trace':
        events.add(obj['event'])
for e in sorted(events):
    print(e)
")

for evt in "${EXPECTED_EVENTS[@]}"; do
    if ! echo "${ALL_EVENTS}" | grep -qx "${evt}"; then
        MISSING+=("${evt}")
    fi
done

FOUND_COUNT=$(echo "${ALL_EVENTS}" | wc -l | tr -d ' ')
echo "  Found ${FOUND_COUNT}/19 unique event types"

if [ ${#MISSING[@]} -eq 0 ]; then
    echo "  ✓ All 19 event types covered"
else
    echo "  ✗ Missing events: ${MISSING[*]}"
    exit 1
fi
echo ""

# 4. Summary
echo "--- Summary ---"
for ndjson in "${TRACE_DIR}"/*.ndjson; do
    name="$(basename "${ndjson}")"
    lines=$(wc -l < "${ndjson}" | tr -d ' ')
    echo "  ${name}: ${lines} events"
done
echo ""
echo "Traces ready for Trace.tla validation."
echo "  JSON files: ${TRACE_DIR}/*.json"
echo "  Spec:       ${SPEC_DIR}/Trace.tla"
echo "  Config:     ${SPEC_DIR}/Trace.cfg"
