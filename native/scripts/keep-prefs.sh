#!/bin/bash
# Runs a command, then puts the app's real preferences back: AppKit saves
# window frames there whenever the app runs, tests included. Keys the run
# added are removed (an import alone only overwrites).
set -uo pipefail
DOMAIN=com.TS.FLTS
TMP="$(mktemp -d)"
defaults export "$DOMAIN" "$TMP/before.plist" 2>/dev/null || true

"$@"
status=$?

if [ -s "$TMP/before.plist" ] && defaults export "$DOMAIN" "$TMP/after.plist" 2>/dev/null; then
  python3 - "$TMP/before.plist" "$TMP/after.plist" >"$TMP/added" <<'PY'
import plistlib, sys
before, after = (plistlib.load(open(p, "rb")) for p in sys.argv[1:3])
for key in after:
    if key not in before:
        print(key)
PY
  while IFS= read -r key; do
    if [ -n "$key" ]; then defaults delete "$DOMAIN" "$key"; fi
  done <"$TMP/added"
  defaults import "$DOMAIN" "$TMP/before.plist"
fi
rm -rf "$TMP"
exit "$status"
