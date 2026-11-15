#!/bin/bash
# In a Simulator, headless: translation, retranslation and Anki against the
# e2e sims (no real service is called), library actions, and sync with the
# real engine kept to this machine. Needs the seeded fixture (see ui-tests.sh).
# Usage: ios-ui-services.sh [simulator-id] [name]
set -euo pipefail
NATIVE="$(cd "$(dirname "$0")/.." && pwd)"
FIXTURE="$NATIVE/build/ui-fixture"
SIM="${1:-3008D068-E4B1-4984-B43C-8988BDAF19CE}"
NAME="${2:-ipad}"
RUN="$NATIVE/build/ios-ui-services/$NAME"

[ -f "$FIXTURE/config.json" ] || { echo "run scripts/ui-tests.sh once to seed the fixture" >&2; exit 1; }
mkdir -p "$RUN"
find "$RUN" -mindepth 1 -delete
cp -R "$FIXTURE" "$RUN/library"
cp -R "$FIXTURE" "$RUN/sync-library"

# The sims exit when their stdin closes.
tail -f /dev/null | node "$NATIVE/scripts/fixtures/sims-up.mjs" "$RUN/text.txt" "$RUN/ports.json" &
trap 'pkill -f "flts-e2e-sims" || true; pkill -f "sims-up.mjs" || true' EXIT
for _ in $(seq 1 100); do [ -s "$RUN/ports.json" ] && break; sleep 0.1; done
[ -s "$RUN/ports.json" ] || { echo "the sims did not start" >&2; exit 1; }

LLM="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['llm'])" "$RUN/ports.json")"
python3 - "$RUN/library/config.json" "$RUN/ports.json" <<'PY'
import json, sys
config, ports = json.load(open(sys.argv[1])), json.load(open(sys.argv[2]))
config["ankiEndpoint"] = f"http://127.0.0.1:{ports['anki']}"
config["ankiSyncEnabled"] = True
json.dump(config, open(sys.argv[1], "w"), indent=2)
PY

cd "$NATIVE"
xcodegen generate >/dev/null
TEST_RUNNER_FLTS_UITEST_SIM_DIR="$RUN/library" \
TEST_RUNNER_FLTS_UITEST_SIM_TEXT="$RUN/text.txt" \
TEST_RUNNER_FLTS_UITEST_SIM_LLM="http://127.0.0.1:$LLM/v1beta/" \
TEST_RUNNER_FLTS_UITEST_SYNC_DIR="$RUN/sync-library" \
  xcodebuild -project FLTS.xcodeproj -scheme FLTS-iOS -configuration Debug \
    -destination "id=$SIM" -derivedDataPath build/DerivedData-iOS \
    -resultBundlePath "$RUN/result.xcresult" -collect-test-diagnostics never \
    test -only-testing:FLTSiOSUITests/FLTSiOSTour/testTranslateWithSims \
    -only-testing:FLTSiOSUITests/FLTSiOSTour/testSyncLocalEngine && status=0 || status=$?

rm -rf "$RUN/shots" && mkdir -p "$RUN/shots"
xcrun xcresulttool export attachments --path "$RUN/result.xcresult" --output-path "$RUN/shots" >/dev/null
python3 - "$RUN/shots" <<'PY'
import json, os, sys
d = sys.argv[1]
for test in json.load(open(os.path.join(d, "manifest.json"))):
    for a in test.get("attachments", []):
        name = a.get("suggestedHumanReadableName", "")
        if name[:2] in ("t-", "s-"):
            os.rename(os.path.join(d, a["exportedFileName"]), os.path.join(d, name.split("_0_")[0] + ".png"))
PY
exit "$status"
