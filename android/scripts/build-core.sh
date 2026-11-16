#!/bin/bash
# Builds the Rust core for Android into app/src/main/jniLibs and regenerates
# its Kotlin bindings.
# Usage: build-core.sh [debug|release|release-ship] [abi ...]   (default: arm64-v8a x86_64)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PROFILE="${1:-${FLTS_CORE_PROFILE:-debug}}"
shift || true
ABIS=("$@")
[ ${#ABIS[@]} -gt 0 ] || ABIS=(arm64-v8a x86_64)

export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
export ANDROID_HOME="${ANDROID_HOME:-/opt/homebrew/share/android-commandlinetools}"
export NDK_HOME="${NDK_HOME:-${ANDROID_NDK_HOME:-$(ls -d "$ANDROID_HOME"/ndk/27.* | sort | tail -1)}}"
# The app's minSdk.
export FLTS_ANDROID_API="${FLTS_ANDROID_API:-29}"
JNILIBS="$ROOT/android/app/src/main/jniLibs"
export FLTS_ANDROID_JNILIBS_DIR="$JNILIBS"

HOST_TAG="$(ls "$NDK_HOME/toolchains/llvm/prebuilt" | grep -v '^\.' | head -1)"
BIN="$NDK_HOME/toolchains/llvm/prebuilt/$HOST_TAG/bin"

cd "$ROOT"
for ABI in "${ABIS[@]}"; do
  case "$ABI" in
    arm64-v8a) TRIPLE=aarch64-linux-android ;;
    x86_64) TRIPLE=x86_64-linux-android ;;
    armeabi-v7a) TRIPLE=armv7-linux-androideabi ;;
    *) echo "unknown abi: $ABI" >&2; exit 2 ;;
  esac
  CLANG_TRIPLE="$TRIPLE"
  [ "$TRIPLE" = armv7-linux-androideabi ] && CLANG_TRIPLE=armv7a-linux-androideabi
  CC="$BIN/${CLANG_TRIPLE}${FLTS_ANDROID_API}-clang"
  UNDER="$(echo "$TRIPLE" | tr '-' '_')"
  UPPER="$(echo "$UNDER" | tr '[:lower:]' '[:upper:]')"
  export "CC_$UNDER=$CC" "CXX_$UNDER=$CC++" "AR_$UNDER=$BIN/llvm-ar"
  export "CARGO_TARGET_${UPPER}_LINKER=$CC"

  if [ "$PROFILE" = "debug" ]; then
    cargo build -p flts-ffi --lib --target "$TRIPLE"
  else
    cargo build -p flts-ffi --lib --profile "$PROFILE" --target "$TRIPLE"
  fi
  LIB="target/$TRIPLE/$PROFILE/libflts_ffi.so"
  mkdir -p "$JNILIBS/$ABI"
  # A debug core carries 400 MB of symbols; Gradle strips only with an NDK it knows of.
  STRIPPED="$(mktemp)"
  "$BIN/llvm-strip" --strip-unneeded -o "$STRIPPED" "$LIB"
  # Only replace when changed so Gradle does not repackage needlessly.
  cmp -s "$STRIPPED" "$JNILIBS/$ABI/libflts_ffi.so" || cp "$STRIPPED" "$JNILIBS/$ABI/libflts_ffi.so"
  rm -f "$STRIPPED"
  "$BIN/llvm-strip" --strip-unneeded "$JNILIBS/$ABI/libsyncthing_core.so"
done

cargo build -p flts-ffi --lib
cargo build -p flts-ffi --features bindgen --bin uniffi-bindgen
GEN_OUT="$ROOT/android/app/src/main/java"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
# From the host library: a ship-profile one is stripped of the metadata this reads.
target/debug/uniffi-bindgen generate --library target/debug/libflts_ffi.dylib --language kotlin --no-format --out-dir "$TMP"
(cd "$TMP" && find . -type f) | while read -r f; do
  mkdir -p "$GEN_OUT/$(dirname "$f")"
  cmp -s "$TMP/$f" "$GEN_OUT/$f" || cp "$TMP/$f" "$GEN_OUT/$f"
done
