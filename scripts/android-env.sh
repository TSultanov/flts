# Sourced by the Android build scripts: finds the SDK and the NDK on macOS and Linux.
# ANDROID_HOME and NDK_HOME win when set.
if [ -z "${ANDROID_HOME:-}" ]; then
  for sdk in "${ANDROID_SDK_ROOT:-}" \
    /opt/homebrew/share/android-commandlinetools \
    /usr/local/share/android-commandlinetools \
    "$HOME/Library/Android/sdk" \
    "$HOME/Android/Sdk"; do
    if [ -n "$sdk" ] && [ -d "$sdk" ]; then
      ANDROID_HOME="$sdk"
      break
    fi
  done
fi
if [ -z "${ANDROID_HOME:-}" ]; then
  echo "Android SDK not found: set ANDROID_HOME" >&2
  exit 1
fi
export ANDROID_HOME

if [ -z "${NDK_HOME:-}" ]; then
  NDK_HOME="${ANDROID_NDK_HOME:-$(ls -d "$ANDROID_HOME"/ndk/27.*/ 2>/dev/null | sort -V | tail -1 | sed 's:/$::')}"
fi
if [ -z "$NDK_HOME" ] || [ ! -d "$NDK_HOME" ]; then
  echo "NDK r27 not found under $ANDROID_HOME/ndk: set NDK_HOME" >&2
  exit 1
fi
export NDK_HOME
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$NDK_HOME}"
