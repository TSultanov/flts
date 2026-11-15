#!/bin/bash
# Runs the native UI tests on a fresh copy of a seeded library each time
# (the tour moves, imports and deletes books). Seeds once into build/.
# QUICK=1 runs only the core library and reader flow. Needs an unlocked, awake screen.
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

# The tests launch the app by bundle id, which would close a copy in use.
if pgrep -lf "/FLTS.app/Contents/MacOS/" | grep -v "$NATIVE/build/" >/dev/null; then
  echo "FLTS is running; quit it before the UI tests (they relaunch the app)." >&2
  exit 1
fi

cd "$NATIVE"
xcodegen generate >/dev/null
TEST_RUNNER_FLTS_UITEST_CONFIG_DIR="$RUN/library" \
TEST_RUNNER_FLTS_UITEST_EMPTY_DIR="$RUN/empty" \
TEST_RUNNER_FLTS_UITEST_EPUB="$NATIVE/scripts/fixtures/import.epub" \
TEST_RUNNER_FLTS_UITEST_QUICK="${QUICK:-}" \
  "$NATIVE/scripts/keep-prefs.sh" xcodebuild -project FLTS.xcodeproj -scheme FLTS -configuration Debug \
    -derivedDataPath build/DerivedData -resultBundlePath "$RUN/result.xcresult" \
    test -only-testing:FLTSUITests
