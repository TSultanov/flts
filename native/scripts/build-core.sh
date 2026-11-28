#!/bin/bash
# Builds the Rust core as a static library and regenerates its Swift bindings.
# Usage: build-core.sh [debug|release|release-ship] [macos|ios|ios-sim]
# The bindings are the same on every platform and come from the macOS build.
set -euo pipefail

# Xcode exports hundreds of build settings (linker, strip, SDK, deployment
# targets); cargo and its build scripts must not see them.
if [ -z "${FLTS_CLEAN_ENV:-}" ]; then
    exec /usr/bin/env -i FLTS_CLEAN_ENV=1 HOME="$HOME" USER="${USER:-}" TMPDIR="${TMPDIR:-/tmp}" \
        FLTS_CORE_PROFILE="${FLTS_CORE_PROFILE:-}" FLTS_MACOS_MIN="${FLTS_MACOS_MIN:-}" \
        FLTS_IOS_MIN="${FLTS_IOS_MIN:-}" FLTS_SKIP_CORE="${FLTS_SKIP_CORE:-}" \
        /bin/bash "$0" "$@"
fi

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PROFILE="${1:-${FLTS_CORE_PROFILE:-debug}}"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export MACOSX_DEPLOYMENT_TARGET="${FLTS_MACOS_MIN:-15.0}"
[ -n "${FLTS_CORE_PROFILE}" ] || unset FLTS_CORE_PROFILE

PLATFORM="${2:-macos}"
case "$PLATFORM" in
    macos) TARGET_ARGS=() TARGET_DIR="target" LIB_OUT="$ROOT/native/build/core" ;;
    ios) TARGET_ARGS=(--target aarch64-apple-ios) TARGET_DIR="target/aarch64-apple-ios" LIB_OUT="$ROOT/native/build/core-ios" ;;
    ios-sim) TARGET_ARGS=(--target aarch64-apple-ios-sim) TARGET_DIR="target/aarch64-apple-ios-sim" LIB_OUT="$ROOT/native/build/core-ios-sim" ;;
    *)
        echo "unknown platform: $PLATFORM" >&2
        exit 2
        ;;
esac
export IPHONEOS_DEPLOYMENT_TARGET="${FLTS_IOS_MIN:-18.0}"

if [ "${FLTS_SKIP_CORE:-}" = "1" ] && [ -f "$LIB_OUT/libflts_ffi.a" ]; then
    exit 0
fi

cd "$ROOT"
if [ "$PROFILE" = "debug" ]; then
  cargo build -p flts-ffi --lib ${TARGET_ARGS[@]+"${TARGET_ARGS[@]}"}
  DIR="$TARGET_DIR/debug"
else
  cargo build -p flts-ffi --lib --profile "$PROFILE" ${TARGET_ARGS[@]+"${TARGET_ARGS[@]}"}
  DIR="$TARGET_DIR/$PROFILE"
fi

GEN_OUT="$ROOT/native/Generated"
mkdir -p "$LIB_OUT" "$GEN_OUT"
# Only replace when changed so Xcode does not relink needlessly.
if ! cmp -s "$DIR/libflts_ffi.a" "$LIB_OUT/libflts_ffi.a"; then
  cp "$DIR/libflts_ffi.a" "$LIB_OUT/libflts_ffi.a"
fi
[ "$PLATFORM" = "macos" ] || exit 0

cargo build -p flts-ffi --features bindgen --bin uniffi-bindgen-swift
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
target/debug/uniffi-bindgen-swift --swift-sources --headers --modulemap \
  --module-name flts_ffiFFI --modulemap-filename module.modulemap \
  "$DIR/libflts_ffi.a" "$TMP"
for f in "$TMP"/*; do
  name="$(basename "$f")"
  if ! cmp -s "$f" "$GEN_OUT/$name"; then
    cp "$f" "$GEN_OUT/$name"
  fi
done
