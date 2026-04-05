#!/usr/bin/env bash
# Run mutex trace harness: tests → preprocess → TLC trace validation
#
# Usage:
#   ./run.sh                          # run all three scenarios
#   ./run.sh concurrent_save_list     # run a specific scenario
#   ./run.sh --skip-tests             # preprocess existing traces + run TLC
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TRACE_DIR="${FLTS_MUTEX_TRACE_DIR:-/tmp/flts_mutex_traces}"
SPEC_DIR="$REPO_ROOT/spec/mutex"
TLC_JAR="${TLC_JAR:-/Volumes/sources/Specula/lib/tla2tools.jar}"
SCENARIO="${1:-}"
SKIP_TESTS=false

if [ "$SCENARIO" = "--skip-tests" ]; then
    SKIP_TESTS=true
    SCENARIO="${2:-}"
fi

echo "=== FLTS Mutex Trace Harness ==="
echo "Repo:     $REPO_ROOT"
echo "Traces:   $TRACE_DIR"
echo "Spec:     $SPEC_DIR"
echo ""

# -----------------------------------------------------------------------
# Step 1: Run Rust tests to generate traces
# -----------------------------------------------------------------------
if [ "$SKIP_TESTS" = false ]; then
    echo "--- Step 1: Running mutex trace tests ---"
    cd "$REPO_ROOT"
    if [ -n "$SCENARIO" ]; then
        FLTS_MUTEX_TRACE_DIR="$TRACE_DIR" cargo test -q -p library "trace_mutex_${SCENARIO}" -- --test-threads=1
    else
        FLTS_MUTEX_TRACE_DIR="$TRACE_DIR" cargo test -q -p library trace_mutex_ -- --test-threads=1
    fi
    echo "Tests passed."
    echo ""
fi

# -----------------------------------------------------------------------
# Step 2: Preprocess traces
# -----------------------------------------------------------------------
echo "--- Step 2: Preprocessing traces ---"

# Find scenario directories
if [ -n "$SCENARIO" ]; then
    SCENARIOS=("$TRACE_DIR/$SCENARIO")
else
    SCENARIOS=()
    for d in "$TRACE_DIR"/*/; do
        if ls "$d"/trace-task-*.ndjson >/dev/null 2>&1; then
            SCENARIOS+=("$d")
        fi
    done
fi

if [ ${#SCENARIOS[@]} -eq 0 ]; then
    echo "ERROR: No trace directories found in $TRACE_DIR"
    exit 1
fi

TRACE_FILES=()
for scenario_dir in "${SCENARIOS[@]}"; do
    scenario_name=$(basename "$scenario_dir")
    output="$REPO_ROOT/traces/mutex_${scenario_name}.json"
    echo "  Preprocessing: $scenario_name"
    python3 "$SCRIPT_DIR/preprocess.py" "$scenario_dir" --output "$output"
    TRACE_FILES+=("$output")
    echo ""
done

# -----------------------------------------------------------------------
# Step 3: Run TLC trace validation
# -----------------------------------------------------------------------
if [ ! -f "$TLC_JAR" ]; then
    echo "WARNING: TLC jar not found at $TLC_JAR"
    echo "Set TLC_JAR env var to the path of tla2tools.jar"
    echo "Skipping TLC validation."
    exit 0
fi

echo "--- Step 3: TLC Trace Validation ---"

cd "$SPEC_DIR"
PASS=0
FAIL=0
for trace_file in "${TRACE_FILES[@]}"; do
    trace_name=$(basename "$trace_file" .json)
    echo "  Validating: $trace_name"
    echo "    Trace: $trace_file"

    if JSON="$trace_file" java -cp "$TLC_JAR" tlc2.TLC Trace.tla \
        -config Trace.cfg \
        -deadlock \
        -workers 1 \
        -cleanup \
        2>&1 | tail -20; then
        echo "    ✅ PASS"
        PASS=$((PASS + 1))
    else
        echo "    ❌ FAIL"
        FAIL=$((FAIL + 1))
    fi
    echo ""
done

echo "=== Results: $PASS passed, $FAIL failed ==="
[ "$FAIL" -eq 0 ]
