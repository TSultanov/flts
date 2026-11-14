# Native macOS app

A SwiftUI app (`native/`) over the same Rust core as the Tauri app. It opens the
same library, config, cache and sync identity (bundle id `com.TS.FLTS`; all paths
are resolved in Rust). The Tauri app remains the shell on iOS and Android.

## Layout

| Crate / dir      | Role                                                                                                                                                                                            |
| ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `app-core`       | The app, independent of the shell: state, queues, sync, Anki, lyrics, view models, and `dispatch` — the `{cmd, args}` → JSON command table.                                                     |
| `site/src-tauri` | Tauri adapter: `TauriHost` (events, main thread, Android dirs) and one invoke handler that forwards every command to `app_core::dispatch`.                                                      |
| `flts-ffi`       | UniFFI facade for Swift: `FltsCore.start(shell:)`, `invoke(cmd:args:)` (JSON), `shutdown()`; events reach Swift through the `Shell` callback with the same names and payloads the webview gets. |
| `native/`        | xcodegen project. `scripts/build-core.sh` builds `libflts_ffi.a` and regenerates `Generated/` (bindings).                                                                                       |

Commands are JSON over FFI on purpose: one table serves Tauri, Swift and the E2E
bridge, and `bridge-conformance.spec.ts` checks it. (The plan had typed UniFFI
records; JSON with Codable models on the Swift side kept one contract instead
of two.) The reader's hot path has its own command, `get_paragraph_texts_batch`
(text plus UTF-16 word ranges, mark runs and the book's language), so Swift
never parses HTML.

## Build

```sh
make build-macos-native     # Release app, core in release-ship
make test-macos-native      # Swift unit tests
make install-macos-native   # replaces /Applications/FLTS.app (quit FLTS first)
```

The Debug scheme runs on `FLTS_CONFIG_DIR=/tmp/flts-dev` with sync off; the
"FLTS (Real Data)" scheme opens the real library. The Tauri targets
(`build-macos`, `install-macos`) are unchanged and remain the rollback.

## Reader

TextKit 2, one layout per chapter at column width; pages are bands cut at line
boundaries, drawn two per spread when the window is wide enough. Word taps, the
selection highlight and the translation over a word are custom drawing on
attribute ranges.

A full layout of a heavy chapter costs ~300 ms, so opening shows a provisional
layout of just the paragraphs from the reading position onward, and the full
layout runs off the main thread. When it lands it is paginated with a forced
break at the line on screen, so nothing moves. Only a chapter's first page can
be short as a result.

### Gate (Release, M-series, 320 paragraphs / ~11.3k words)

| Scenario                            | Result                         | Target            |
| ----------------------------------- | ------------------------------ | ----------------- |
| Open at start, first paint          | 25 ms                          | < 150 ms          |
| Restore ¾ deep, first paint         | 24 ms                          | < 150 ms          |
| Full layout lands (background)      | ~320 ms, view unchanged        | —                 |
| Flip, worst / median                | 15.6 / 3.6 ms                  | < 33 / < 17 ms    |
| Tap a word                          | 3.7 ms                         | < 33 ms           |
| Live resize step                    | 6.8 ms                         | < 33 ms           |
| 20 paragraphs retranslated in place | 4 turns of ≤ 6 ms, anchor kept | < 33 ms per frame |

Go. Bursts of changed paragraphs are applied five per run-loop turn, so no
frame waits on the whole burst (36 ms if done at once). Reproduce with
`TEST_RUNNER_FLTS_PERF=1 TEST_RUNNER_FLTS_SNAPSHOT_DIR=<dir> xcodebuild … test
-only-testing:FLTSTests/ReaderPerf -configuration Release ENABLE_HARDENED_RUNTIME=NO FLTS_NO_EXPORTS=`.

### Against the web reader

Chapter open including the fetch from the core, 320 paragraphs, median of 5
(native: `OpenTiming` test, Release, real core; web: `chapter-open-perf.spec.ts`
"a long translated chapter" on headless WebKit through the E2E bridge):

|                                      | Native | Web (WebKit) |
| ------------------------------------ | ------ | ------------ |
| From the chapter's start, text shown | 59 ms  | 58–70 ms     |
| Restoring ¾ in, text shown           | 42 ms  | 84–99 ms     |

The books differ in paragraph lengths, and headless WebKit is not the app's own
webview (that run needs a visible window), so this is indicative only. About
30 ms of the native time is the JSON fetch of all 320 paragraphs.

### Reading position

The synced position is (paragraph, pages into it), which cannot name a line.
The native app also keeps the exact top line per book locally and uses it when
the synced position is still the one it saved; otherwise (another device read
on) it lands on the right page of that paragraph.

## Verification

- Swift unit tests (`make test-macos-native`): stores against a fake core, the
  layout (pagination, in-place edits), exact reopen and cross-device position
  fallback, and a hit-test gate (284 random taps on provisional and full
  layouts, 88 on hyphen-split words, no misses); clean under Thread Sanitizer.
- Smoke tests against the real core, opt-in by environment:
  `FLTS_SMOKE_CONFIG_DIR` (seeded library: reader, word panel, dictionary via
  the main-thread hop, Anki card, import/move/delete), `FLTS_SMOKE_EMPTY_DIR`
  (first launch and config written through the core), `FLTS_SMOKE_SYNC_DIR`
  (+ `FLTS_SMOKE_PEER_ID`: embedded Syncthing online, pair and unpair, quit),
  `FLTS_SMOKE_TRANSLATE_DIR` with `FLTS_SMOKE_LLM_URL`, `FLTS_SMOKE_TEXT`,
  `FLTS_SMOKE_EPUB` (EPUB parse and import; a chapter translated against the
  LLM sim, the reader store updated by events alone: 24 paragraphs, 12 text
  fetches).
- Reader selection: drag selects (Copy, ⌘C, Look Up); a click without a drag
  taps the word. Tested with synthesized mouse events.
- UI tests (`make test-macos-native-ui FLTS_UITEST_CONFIG_DIR=<seeded dir>`):
  open a book and flip, open Settings. They need an unlocked session; on a
  locked screen XCTest refuses to start.
- Tauri on all three platforms still builds on the refactored core: macOS
  release, a signed iOS `.ipa`, a signed Android APK.
- Runs on another config dir and tests keep UI state in the
  `com.TS.FLTS.isolated` defaults suite, never the real app's.
- The Tauri production IPC path (one invoke handler over `dispatch`) was driven
  in the real webview through `e2e_eval`; the real E2E tier and Rust tests pass.

## Size

Native 32.0 MB binary vs Tauri 31.9 MB: both are dominated by the shared Rust
core and the Go Syncthing archive (`__TEXT` 26.7 vs 27.2 MB); WebKit is a system
framework, so dropping it saves little. Release strips symbols, builds Swift
with `-Osize` and exports nothing (`-exported_symbols_list /dev/null`), which
took the export trie from 1.6 MB to 0.3 MB.

## Not in v1

- QR scanning (macOS shows the pairing code; scanning comes with iPad).
- An iPad target: the reader canvas and `Platform` are AppKit. `Sources/Core`
  and `Sources/Stores` type-check against the iOS 18 SDK under Swift 6.
- Lyrics against a live Spotify was not driven by automation (it would prompt
  for Automation access); the store is unit-tested and the core path is covered
  by the Tauri E2E tier.
- Universal (x86_64) build; sandbox and App Store.

## Cutover notes

The new code signature makes macOS ask again for keychain access to the Spotify
token and for Automation access to Spotify. The `.flts-lock` lease keeps the
Tauri and native apps from running on one library at once.
