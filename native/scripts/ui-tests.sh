#!/bin/bash
# Runs the native UI tests on a fresh copy of a seeded library each time
# (the tour moves, imports and deletes books). Seeds once into build/.
# Needs an unlocked, awake screen.
set -euo pipefail
NATIVE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$(dirname "$NATIVE")"
FIXTURE="$NATIVE/build/ui-fixture"
RUN="$NATIVE/build/ui-run"

if [ ! -f "$FIXTURE/config.json" ]; then
  (cd "$ROOT" && cargo build -p app -p e2e-sims --features app/e2e-bridge)
  node "$NATIVE/scripts/fixtures/seed-library.mjs" "$FIXTURE" 40 "$NATIVE/scripts/fixtures/sample.epub"
fi
rm -rf "$RUN"
mkdir -p "$RUN/empty"
cp -R "$FIXTURE" "$RUN/library"

# AppKit saves window frames under the app's real preferences; put them back.
PREFS="$RUN/prefs.plist"
defaults export com.TS.FLTS "$PREFS" 2>/dev/null || true
restore_prefs() {
  if [ -s "$PREFS" ]; then defaults import com.TS.FLTS "$PREFS"; fi
}
trap restore_prefs EXIT

cd "$NATIVE"
xcodegen generate >/dev/null
TEST_RUNNER_FLTS_UITEST_CONFIG_DIR="$RUN/library" \
TEST_RUNNER_FLTS_UITEST_EMPTY_DIR="$RUN/empty" \
TEST_RUNNER_FLTS_UITEST_EPUB="$NATIVE/scripts/fixtures/import.epub" \
  xcodebuild -project FLTS.xcodeproj -scheme FLTS -configuration Debug \
    -derivedDataPath build/DerivedData -resultBundlePath "$RUN/result.xcresult" \
    test -only-testing:FLTSUITests
