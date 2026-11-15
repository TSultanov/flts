#!/bin/bash
# Runs the iOS UI tour in a Simulator on a fresh copy of the seeded library
# (see ui-tests.sh for the seeding). The Simulator runs headless; nothing
# appears on screen. Usage: ios-ui-tests.sh [simulator-id] [name]
set -euo pipefail
NATIVE="$(cd "$(dirname "$0")/.." && pwd)"
FIXTURE="$NATIVE/build/ui-fixture"
SIM="${1:-3008D068-E4B1-4984-B43C-8988BDAF19CE}"
NAME="${2:-ipad}"
RUN="$NATIVE/build/ios-ui-run/$NAME"

[ -f "$FIXTURE/config.json" ] || { echo "run scripts/ui-tests.sh once to seed the fixture" >&2; exit 1; }
mkdir -p "$RUN"
find "$RUN" -mindepth 1 -delete
cp -R "$FIXTURE" "$RUN/library"
mkdir -p "$RUN/empty"

cd "$NATIVE"
xcodegen generate >/dev/null
TEST_RUNNER_FLTS_UITEST_CONFIG_DIR="$RUN/library" \
TEST_RUNNER_FLTS_UITEST_EMPTY_DIR="$RUN/empty" \
  xcodebuild -project FLTS.xcodeproj -scheme FLTS-iOS -configuration Debug \
    -destination "id=$SIM" -derivedDataPath build/DerivedData-iOS \
    -resultBundlePath "$RUN/result.xcresult" -collect-test-diagnostics never \
    test && status=0 || status=$?

# The screenshots, by name.
rm -rf "$RUN/shots" && mkdir -p "$RUN/shots"
xcrun xcresulttool export attachments --path "$RUN/result.xcresult" --output-path "$RUN/shots" >/dev/null
python3 - "$RUN/shots" <<'PY'
import json, os, sys
d = sys.argv[1]
for test in json.load(open(os.path.join(d, "manifest.json"))):
    for a in test.get("attachments", []):
        name = a.get("suggestedHumanReadableName", "")
        if name[:1].isdigit():
            os.rename(os.path.join(d, a["exportedFileName"]), os.path.join(d, name.split("_0_")[0] + ".png"))
PY
exit "$status"
