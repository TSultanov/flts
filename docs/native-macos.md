# Native macOS app

A SwiftUI app (`native/`) over the same Rust core as the Tauri app. It opens the
same library, config, cache and sync identity (bundle id `com.TS.FLTS`; all paths
are resolved in Rust). The Tauri app remains the shell on iOS and Android.

## Layout

| Crate / dir      | Role                                                                                                                                                                                                                                                       |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `app-core`       | The app, independent of the shell: state, queues, sync, Anki, lyrics, view models, and `dispatch` — the `{cmd, args}` → JSON command table.                                                                                                                |
| `site/src-tauri` | Tauri adapter: `TauriHost` (events, main thread, Android dirs) and one invoke handler that forwards every command to `app_core::dispatch`.                                                                                                                 |
| `flts-ffi`       | UniFFI facade for Swift: `FltsCore` with one typed async method per command the app uses, records for its data, a `CoreEvent` enum for events, and `EpubPreview` (a parsed EPUB kept in Rust until imported). Ids reach Swift as `UUID`, numbers as `Int`. |
| `native/`        | xcodegen project. `scripts/build-core.sh` builds `libflts_ffi.a` and regenerates `Generated/` (bindings).                                                                                                                                                  |

Tauri and the E2E bridge use the JSON command table (`dispatch`); Swift uses the
typed `FltsCore`, which calls the same app-core functions. The records mirror
the core's serde names, so most methods convert a core value into its record
through serde inside Rust (no JSON crosses into Swift); the reader's paragraph
texts convert field by field. Events arrive from the core as (name, payload)
and are decoded once into `CoreEvent`; a test checks every core event has a
case. The reader's text comes as plain text plus UTF-16 word ranges, mark runs
and the book's language, so Swift never parses HTML.

## Layout

As in the web app: the window opens on a library page (folders and books;
double-click or Return opens, the context menu and Delete act on the whole
selection). An open book is the reader alone; its chapter list slides over the
page from the left (the Chapters button, C, or ⌃⌘C), takes the keyboard (arrows,
Return, Escape) and closes on a pick, so the
text never reflows. The word bar runs along the bottom: word, dictionary form,
transcription, translations, and Anki; W (or ⌃⌘W) replaces it with the word's
details, which grow upward over the page. Back to the library is the toolbar's
Library button (⌥⌘L). Sync status, Anki sync and Lyrics sit in the library's
toolbar only; Lyrics is also in the menu (⇧⌘L).

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
| Full layout lands (background)      | ~480 ms, view unchanged        | —                 |
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
| From the chapter's start, text shown | 45 ms  | 58–70 ms     |
| Restoring ¾ in, text shown           | 29 ms  | 84–99 ms     |

The books differ in paragraph lengths, and headless WebKit is not the app's own
webview (that run needs a visible window), so this is indicative only. About
20–28 ms of the native time is fetching all 320 paragraphs from the core.

### Reading position

The synced position is (paragraph, pages into it), which cannot name a line.
The native app also keeps the exact top line per book locally and uses it when
the synced position is still the one it saved; otherwise (another device read
on) it lands on the right page of that paragraph.

## Verification

- Swift unit tests (`make test-macos-native`): stores against a fake core
  (`Tests/FakeCore.swift`, generated from `FltsCoreProtocol`), the
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
- UI tests (`make test-macos-native-ui`, about 50 s plus the build; it seeds a
  library once and runs on a fresh copy each time;
  `make test-macos-native-ui-quick` stops after the library and reader checks,
  about 20 s): one tour that asserts the arrow
  keys still page after tapping words, using the word panel and picking a
  chapter, moves a book into a new folder, imports an EPUB and deletes it,
  opens Lyrics and Settings, quits and relaunches in dark mode, drives the
  smallest window from the keyboard, and on an empty config dir finishes
  onboarding and imports a pasted text. Its screenshots are
  attachments in the result bundle. It needs an unlocked session, and waits by
  polling, since XCTest's own waits re-check only once a second. The app runs
  with `FLTS_NO_ANIMATIONS`, since the runner waits out every animation. The
  tour also asserts that showing the chapters does not resize the reader, and
  `FLTS_UITEST_STRESS_DIR` enables a 60-launch stress test, and with
  `FLTS_UITEST_LONG_EPUB` (a book from `scripts/fixtures/gen-long-epub.py`) a
  check that the chapter list is as quick on a 200-page chapter as on a short
  one and never re-pages it.
- `make test-macos-native-ui-translate` (about 15 s plus the build) starts the
  e2e sims and, on screen, imports a text, translates its chapter, reads a
  word, translates the paragraph again with a model picked from the list and
  adds an Anki card. No real service is called. It also turns sync on with
  the real engine kept to this Mac (`FLTS_SYNC_HERMETIC`) on a throwaway
  library: online status, pairing code and device ID, renaming the device,
  the library's sync status button, turning sync off, and quitting with the
  engine running.
- A copy of a real library can be checked without touching the original:
  copy `config.json` and `library/` somewhere, turn `syncEnabled` and
  `ankiSyncEnabled` off in the copy, and set `FLTS_GEMINI_BASE_URL` to a dead
  address so nothing is sent or billed. The `RealLibraryCheck` unit test
  (`FLTS_TEST_REAL_DUMP=<out.json>` with `FLTS_CONFIG_DIR=<copy>`) dumps what
  the app's bindings report, for comparison with the Tauri core's view of the
  same data, and `testRealLibraryCopy` (`FLTS_UITEST_REAL_DIR=<copy>`) opens
  the copy on screen. The window
  size comes from `FLTS_WINDOW_SIZE`, which the app applies itself: a frame
  pinned by a launch argument made it die at launch in AppKit's toolbar layout.
- Reading position holds across a resize round trip
  (a sticky anchor; full layouts wait for relayouts to settle).
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

## Parity with the web app

The views were checked control by control against their Svelte templates.
Native follows the web app's behaviour, including:

- summary gating ("Waiting for chapter summaries…");
- ⌘-click to translate without the cache;
- the OpenRouter model-family picker;
- the book-deleted notice;
- EPUB chapter selection with previews, and returning to the library after import;
- the Spotify setup hint with the redirect URI.

These differences are kept on purpose, as macOS conventions:

- one API key field for the current provider, saved per field rather than with a Save button;
- multi-select with a selection bar instead of checkboxes;
- a folder picker for moving books;
- word details shown flat rather than in collapsible sections;
- a Settings window with tabs.

## Fixtures

`native/scripts/fixtures/seed-library.mjs <dir> [paragraphs]` seeds a config
dir with a translated Spanish book (through the headless Tauri app's E2E bridge
and the LLM sim); `sims-up.mjs` starts the sims with scripted translations for
the translate smoke test. Both need `cargo build -p app -p e2e-sims --features
app/e2e-bridge`. `seed-library.mjs <dir> 40 scripts/fixtures/sample.epub` adds a
multi-chapter book for the UI tour; `import.epub` is what the tour imports
(`FLTS_UITEST_EPUB`). `scripts/gen-fake-core.py` regenerates `Tests/FakeCore.swift`
after the core API changes.

## Not in v1

- QR scanning (macOS shows the pairing code; scanning comes with iPad).
- An iPad target: the reader canvas and `Platform` are AppKit. `Sources/Core`
  and `Sources/Stores` type-check against the iOS 18 SDK under Swift 6.
- Dark mode and the smallest window (720×480 content) are checked on screen by
  the UI tour: dark through the app's own Appearance setting (Reading), the
  smallest size through a preset window frame. `StateGallery` renders states
  the tour cannot reach (OpenRouter family picker, lyrics with glosses, the
  deleted-book notice, the shared-identity choice, translate waiting for
  summaries) in light and dark to `FLTS_SNAPSHOT_DIR`.
- Universal (x86_64) build; sandbox and App Store.

## Cutover notes

The new code signature makes macOS ask again for keychain access to the Spotify
token and for Automation access to Spotify. The `.flts-lock` lease keeps the
Tauri and native apps from running on one library at once.
