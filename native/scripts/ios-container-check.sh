#!/bin/bash
# The path a real device takes, in a Simulator: no test folder, so the app
# uses its own container. A fresh install must go through onboarding and
# write its config where the Tauri app keeps it; then, with a library placed
# there (as after installing over the Tauri app), it must find the books.
# Usage: ios-container-check.sh [simulator-id]
set -euo pipefail
NATIVE="$(cd "$(dirname "$0")/.." && pwd)"
FIXTURE="$NATIVE/build/ui-fixture"
SIM="${1:-4A8E245A-F9AD-41F8-AD90-554AC8B87AF9}"
RUN="$NATIVE/build/ios-container-check"
[ -f "$FIXTURE/config.json" ] || { echo "run scripts/ui-tests.sh once to seed the fixture" >&2; exit 1; }
mkdir -p "$RUN"

cd "$NATIVE"
xcodegen generate >/dev/null
xcrun simctl boot "$SIM" 2>/dev/null || true
xcrun simctl bootstatus "$SIM" >/dev/null
xcrun simctl uninstall "$SIM" com.TS.FLTS || true

stage() {
  TEST_RUNNER_FLTS_UITEST_CONTAINER_STAGE="$1" \
    xcodebuild -project FLTS.xcodeproj -scheme FLTS-iOS -configuration Debug \
      -destination "id=$SIM" -derivedDataPath build/DerivedData-iOS \
      -resultBundlePath "$RUN/stage$1.xcresult" -collect-test-diagnostics never \
      test -only-testing:FLTSiOSUITests/FLTSiOSTour/testOwnContainer
}

rm -rf "$RUN/stage1.xcresult" "$RUN/stage2.xcresult"
stage 1
DATA="$(xcrun simctl get_app_container "$SIM" com.TS.FLTS data)/Library/Application Support/com.TS.FLTS"
[ -f "$DATA/config.json" ] || { echo "no config.json in the app's container at $DATA" >&2; exit 1; }
echo "CONTAINER-CONFIG $DATA/config.json"

# Existing data, where the Tauri app keeps it.
xcrun simctl terminate "$SIM" com.TS.FLTS 2>/dev/null || true
cp "$FIXTURE/config.json" "$DATA/config.json"
mkdir -p "$DATA/library"
find "$DATA/library" -mindepth 1 -delete
cp -R "$FIXTURE/library/." "$DATA/library/"
stage 2
echo "CONTAINER-CHECK passed"
