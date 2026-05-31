#!/usr/bin/env bash
# Build the FLTS Android app and emit an installable, signed APK.
#
#   ./build-android.sh            # build + sign -> prints the signed APK path
#   ./build-android.sh --install  # also `adb install -r` to a connected device
#   ./build-android.sh --run      # also install AND launch the app
#
# Release signing: by default the APK is signed with a local debug keystore
# (installable, but NOT distributable). To sign with a real keystore, set:
#   FLTS_KEYSTORE  FLTS_KEY_ALIAS  FLTS_STORE_PASS  FLTS_KEY_PASS
#
# Prerequisites: cargo + cargo-tauri, pnpm/node (Tauri runs `pnpm build`),
# Android SDK + NDK r27, and a Gradle JDK <= 24 (pinned in ~/.gradle/gradle.properties).
set -euo pipefail
cd "$(dirname "$0")" # site/ (Tauri project root)

# --- Android SDK / NDK / build-tools (fall back to the Homebrew layout) ---
export ANDROID_HOME="${ANDROID_HOME:-/opt/homebrew/share/android-commandlinetools}"
[[ -n "${NDK_HOME:-}" ]] || export NDK_HOME="$(ls -d "$ANDROID_HOME"/ndk/*/ 2>/dev/null | sort -V | tail -1 | sed 's:/$::')"
[[ -n "${ANDROID_NDK_HOME:-}" ]] || export ANDROID_NDK_HOME="$NDK_HOME"
BT="$(ls -d "$ANDROID_HOME"/build-tools/*/ 2>/dev/null | sort -V | tail -1 | sed 's:/$::')"

# --- Signing key (debug by default; override via env for a release key) ---
KEYSTORE="${FLTS_KEYSTORE:-$HOME/.android/debug.keystore}"
KEY_ALIAS="${FLTS_KEY_ALIAS:-androiddebugkey}"
STORE_PASS="${FLTS_STORE_PASS:-android}"
KEY_PASS="${FLTS_KEY_PASS:-android}"
if [[ "$KEYSTORE" == "$HOME/.android/debug.keystore" && ! -f "$KEYSTORE" ]]; then
	echo ">> creating debug keystore at $KEYSTORE"
	mkdir -p "$(dirname "$KEYSTORE")"
	keytool -genkeypair -v -keystore "$KEYSTORE" -alias "$KEY_ALIAS" \
		-keyalg RSA -keysize 2048 -validity 10000 \
		-storepass "$STORE_PASS" -keypass "$KEY_PASS" -dname "CN=Android Debug,O=Android,C=US"
fi

# --- Build ---
echo ">> building (SDK=$ANDROID_HOME NDK=$NDK_HOME)"
cargo tauri android build

# --- Sign the universal release APK ---
OUT="src-tauri/gen/android/app/build/outputs/apk/universal/release"
ALIGNED="$OUT/app-universal-release-aligned.apk"
SIGNED="$OUT/app-universal-release-signed.apk"
echo ">> signing with build-tools $(basename "$BT")"
rm -f "$ALIGNED" "$SIGNED"
"$BT/zipalign" -p -f 4 "$OUT/app-universal-release-unsigned.apk" "$ALIGNED"
"$BT/apksigner" sign --ks "$KEYSTORE" --ks-pass "pass:$STORE_PASS" \
	--ks-key-alias "$KEY_ALIAS" --key-pass "pass:$KEY_PASS" --out "$SIGNED" "$ALIGNED"
"$BT/apksigner" verify "$SIGNED" >/dev/null 2>&1 || {
	echo "!! signature verify failed"
	exit 1
}
rm -f "$ALIGNED"
echo ">> signed APK: $(pwd)/$SIGNED"

# --- Optional install / run ---
case "${1:-}" in
--install | -i) adb install -r "$SIGNED" ;;
--run | -r)
	adb install -r "$SIGNED"
	PKG="$(sed -n 's/.*"identifier":"\([^"]*\)".*/\1/p' src-tauri/tauri.conf.json | head -1)"
	adb shell am start -n "${PKG:-com.TS.FLTS}/.MainActivity"
	;;
esac
