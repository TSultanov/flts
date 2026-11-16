#!/bin/bash
# Runs the Android UI tour on a connected device or emulator, on a fresh copy
# of the seeded library (native/scripts/ui-tests.sh seeds it once). The debug
# build is com.TS.FLTS.dev, so the real app's data is never touched.
set -euo pipefail
ANDROID="$(cd "$(dirname "$0")/.." && pwd)"
FIXTURE="$ANDROID/../native/build/ui-fixture"
PKG=com.TS.FLTS.dev
STAGE=/data/local/tmp/flts-ui-fixture

[ -f "$FIXTURE/config.json" ] || { echo "run native/scripts/ui-tests.sh once to seed the fixture" >&2; exit 1; }
adb get-state >/dev/null 2>&1 || { echo "no device or emulator connected" >&2; exit 1; }

cd "$ANDROID"
./gradlew --console=plain -q :app:installDebug :app:installDebugAndroidTest

# The app can only read its own storage: stage the copy, then take it in as the app.
adb shell rm -rf "$STAGE"
adb push "$FIXTURE" "$STAGE" >/dev/null
# Readable by the app whoever adb runs as.
adb shell chmod -R a+rX "$STAGE"
# One string: adb re-splits separate arguments on the device.
adb shell "run-as $PKG sh -c 'rm -rf files/uitest && cp -R $STAGE files/uitest'"
CONFIG_DIR="$(adb shell "run-as $PKG sh -c 'cd files/uitest && pwd'" | tr -d '\r')"

adb shell am instrument -w -e FLTS_CONFIG_DIR "$CONFIG_DIR" \
  "$PKG.test/androidx.test.runner.AndroidJUnitRunner" | tee /dev/stderr | grep -q "^OK ("
