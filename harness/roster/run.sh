#!/usr/bin/env bash
#
# Generate + sanity-check roster-mesh NDJSON traces for spec/roster/.
#
# Instrumentation is compiled into the real library behind the `tla_trace` Cargo
# feature (see ../../library/src/sync/engine.rs), so there is no patch/apply step
# — building with the feature IS the instrumentation. Scenarios live in
# ../../library/src/sync/trace_harness.rs and drive the real SyncEngine /
# RosterStore / reconcile over a MockSyncthing (no live Go engine).
#
# Run:  bash harness/roster/run.sh
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo_root"
mkdir -p traces

# `--lib` skips the tests/ integration binaries (some unrelated ones don't build
# under this feature set). Filtered + single-threaded because the trace sink is a
# process-global; only the roster harness may run, or other tests' tla_trace
# emits would pollute the files.
cargo test -q -p library --lib --features tla_trace,sync-engine \
    trace_roster_scenarios -- --test-threads=1

python3 - <<'PY'
import json, pathlib
# 6 spec actions; ApprovePending is operationally identical to PairOn (same
# add_device + add_peer effect) and is emitted as PairOn, so 5 distinct names
# cover all 6 actions.
expected = {"EnsureSelf", "PairOn", "RosterSync", "ReconcileNode", "UnpairOn"}
seen = set()
for path in sorted(pathlib.Path("traces").glob("roster_*.ndjson")):
    lines = path.read_text().splitlines()
    if not lines:
        raise SystemExit(f"{path} is empty")
    for i, line in enumerate(lines, 1):
        ev = json.loads(line)
        if ev.get("tag") != "trace":
            raise SystemExit(f"{path}:{i}: missing trace tag")
        name = ev["event"]["name"]
        for k in ("node", "roster", "engine"):
            if k not in ev["event"]:
                raise SystemExit(f"{path}:{i}: event missing {k}")
        seen.add(name)
    print(f"{path.name}: {len(lines)} trace event(s)")
missing = sorted(expected - seen)
if missing:
    raise SystemExit("missing expected event types: " + ", ".join(missing))
print("event coverage: ok (" + ", ".join(sorted(seen)) + ")")
PY

echo
echo "Validate against the spec with:"
echo "  cd spec/roster && JSON=../../traces/roster_mesh_forms.ndjson \\"
echo "    java -cp <tla2tools>:<CommunityModules> tlc2.TLC -config Trace.cfg Trace.tla"
