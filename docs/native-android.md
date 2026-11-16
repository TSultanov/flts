# Native Android app

A Kotlin/Compose app (`android/`) over the same Rust core as the SwiftUI apps;
see `docs/native-macos.md` for the core, the bindings and the reader's design,
and `docs/native-ios.md` for the touch behaviour it follows. It is meant to
replace the Tauri Android app: same application id (`com.TS.FLTS`), same
signing key, the same directories (`Context.dataDir` for config and library,
`Context.cacheDir` for caches, which is where Tauri's path API put them), and
version code 1100 against the Tauri build's 1000. It needs Android 10 (API 29)
and ships arm64 only; the Tauri app ran on Android 7 and on 32-bit devices.

The Tauri app (`make build-android`) is unchanged and remains the rollback.
Android refuses a lower version code and an uninstall deletes the data, so
going back means building the Tauri app with a version code above 1100
(`bundle.android.versionCode` in `tauri.conf.json`), signed with the same key.

Debug builds are `com.TS.FLTS.dev`: they install beside the real app and never
see its library.

## Layout

| Path                                | Role                                                                                                                                                                                                                                                                                                    |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `flts-ffi`                          | The UniFFI facade, also for Kotlin. For Android it adds `FltsCore.startWithDirs` (there is no HOME to resolve paths from), a logcat logger, and `Native.init`, which hands the JVM to the Rust TLS verifier (`rustls-platform-verifier`, used by the OpenAI-compatible providers) and to `ndk-context`. |
| `android/scripts/build-core.sh`     | Builds `libflts_ffi.so` with the NDK (and through it the Go `libsyncthing_core.so`) into `app/src/main/jniLibs`, and regenerates the Kotlin bindings. Gradle runs it before compiling; `FLTS_SKIP_CORE=1` skips it.                                                                                     |
| `app/src/main/java/uniffi/flts_ffi` | The generated bindings, committed. They call the core through JNA.                                                                                                                                                                                                                                      |
| `…/core`, `…/stores`                | The shell (events and main-thread work hop to the main looper) and one store per domain, as in `native/Sources/Stores`. Stores hold Compose state.                                                                                                                                                      |
| `…/reader`                          | `PageMap` (pagination, plain Kotlin), `ChapterTextLayout`, `ReaderView` (a custom `View`), and the Compose bridge.                                                                                                                                                                                      |
| `…/ui`, `…/platform`                | The screens, the EPUB picker, the pairing code and its camera scanner (CameraX and ZXing; no Play Services).                                                                                                                                                                                            |

The app model lives in the `Application`, so the core outlives the activity;
the activity handles rotation itself and the reader re-lays out in place.
Coming back to the foreground wakes sync and drops stale translation
progress. Android delivers no exit, so the core is never shut down: the
reading position is saved as pages turn and when the reader goes away.

## Reader

One `StaticLayout` per paragraph, stacked into a tall column; pages are bands
of it cut at line boundaries, as in the Swift reader, and a changed paragraph
costs only its own layout. Justified and hyphenated in the book's language,
with every line exactly the line height. Opening shows a provisional layout
from the reading position onward; the full one runs off the main thread and
is swapped in with a forced break at the line on screen. Two pages side by
side when the view is wide enough. Input is the iOS reader's: tap a word (a
near miss counts), drag the spread, tap the right margin, long press for a
menu, the paragraph's translate button in the left margin, and a hardware
keyboard's arrows, Space, Home, End, W, C and Escape. System Back closes the
chapter list, then the word's details, then the book.

Not on Android, as under Tauri: Lyrics and Spotify, the system dictionary,
and sync while the app is in the background.

## Build

```sh
make test-android-native      # unit tests on the JVM (Robolectric), no device
make build-android-native     # release APK, core in release-ship
make install-android-native   # adb install over the Tauri app, keeping its data
```

`cd android && ./gradlew :app:assembleDebug` builds the `.dev` variant with a
debug core. The build needs NDK r27 (`NDK_HOME`), Go, and JDK 21 for Gradle;
`FLTS_ANDROID_ABIS=arm64-v8a,x86_64` adds ABIs. Release signing reads
`FLTS_KEYSTORE`, `FLTS_KEY_ALIAS`, `FLTS_STORE_PASS` and `FLTS_KEY_PASS`, as
`site/build-android.sh` does, and falls back to the debug keystore.

Debug builds take the core's test switches as launch extras, e.g.
`adb shell am start -n com.TS.FLTS.dev/com.TS.FLTS.MainActivity -e FLTS_CONFIG_DIR <dir>`;
with one set, UI preferences go to their own file.

## Tests

`make test-android-native` runs, with real text measurement under Robolectric:
the stores against a fake core (`FakeCore.kt`, regenerated by
`android/scripts/gen-fake-core.py` after the core API changes), pagination,
the layout, a hit-test gate (240 random taps on provisional and full layouts,
no misses), reopening on the same line from any page, position held across
resizes and rotations, and a stream of updates during the provisional layout.
Robolectric has no hyphenation patterns, so words split across lines are not
covered there.

`FLTS_SMOKE_CONFIG_DIR=<copy of a seeded config dir> make test-android-native`
also runs `CoreSmokeTest`: the generated bindings against the core built for
this machine (`cargo build -p flts-ffi --lib` first), reading the seeded
library and receiving an event. This is what checks the bindings themselves;
the fake core bypasses them.

`make test-android-native-ui` installs the `.dev` build on a connected device
or emulator, copies the seeded library (`native/build/ui-fixture`) into the
app's own storage and runs `TourTest`: library, reader, settings, importing a
pasted text.

With the same variable set, `AppTourTest` runs the whole app on the JVM: the
real screens and reader over that core, on the seeded library. It opens a
book, waits for the full layout, turns a page, taps a translated word and
opens its details, changes the text size, opens every Settings page, imports
a pasted text, moves it into a new folder, deletes it, and imports an EPUB
(`FLTS_UITEST_EPUB` names the file, since no test can drive the system
picker). `OnboardingTest` covers the first run against the fake core.

## Verified on an emulator

On a Pixel 7 image (Android 15, arm64), with the `.dev` build unless noted:

- the core starts in the app's data directory and opens a seeded library;
  the on-device tour passes (`make test-android-native-ui`);
- an OpenAI-compatible provider is reached over TLS (an invalid key comes
  back as 401, shown in the reader), which is the path through the Rust
  verifier;
- the reader draws justified, hyphenated pages, a tapped word shows its
  translation and the word bar, and a swipe turns the page;
- sync comes online with its device ID and pairing code;
- the release build (R8) starts the core;
- the release build installs over the Tauri build, opens the same
  directories with the existing library files in place, and Android then
  refuses the Tauri build as a downgrade.

Two things the first run on Android turned up, both fixed in the core:
`std::fs::File::lock` and `try_lock` are not implemented for Android, so the
library lease and the store's file locks failed and no library opened (under
the Tauri shell as well); `dbsync::file_lock` calls `flock` there. And the
core's runtime threads are now attached to the JVM for life: a thread the
bindings attach for one callback is detached when that callback returns, even
when it is nested in another on the same thread, which aborted the process
when a coroutine resumed inline on a core thread.

## Not verified yet

These need a real device:

- the camera scanner reading a code shown by another device, and two devices
  pairing and syncing;
- an EPUB picked through the system picker;
- the reader's timings against the gate in `docs/native-macos.md`;
- the upgrade over the Tauri app on real data, signed with the real key, and
  a device with 16 KB pages.
