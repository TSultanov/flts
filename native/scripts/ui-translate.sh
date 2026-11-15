#!/bin/bash
# Drives translation, retranslation and Anki on screen against the e2e sims
# (no real service is called), and the Sync settings with the real engine kept
# to this Mac. Needs the seeded fixture (see ui-tests.sh) and an unlocked,
# awake screen.
set -euo pipefail
NATIVE="$(cd "$(dirname "$0")/.." && pwd)"
FIXTURE="$NATIVE/build/ui-fixture"
RUN="$NATIVE/build/ui-translate"

if pgrep -lf "/FLTS.app/Contents/MacOS/" | grep -v "$NATIVE/build/" >/dev/null; then
  echo "FLTS is running; quit it before the UI tests (they relaunch the app)." >&2
  exit 1
fi
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
  "$NATIVE/scripts/keep-prefs.sh" xcodebuild -project FLTS.xcodeproj -scheme FLTS -configuration Debug \
    -derivedDataPath build/DerivedData -resultBundlePath "$RUN/result.xcresult" \
    test -only-testing:FLTSUITests/FLTSScreenshotTour/testTranslateWithSims \
    -only-testing:FLTSUITests/FLTSScreenshotTour/testSyncSettingsLocalEngine
