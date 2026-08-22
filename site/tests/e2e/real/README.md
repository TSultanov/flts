# Real-backend E2E tier

Specs here drive the **real Rust backend** (the Tauri `app` binary, real HTTP
clients, real library/translation/Anki code) against stateful simulators of the
three external services FLTS depends on. The mock tier
(`site/tests/e2e/*.spec.ts` under `site/playwright.config.ts`) stays the fast inner
loop; this tier is the integration truth.

## Architecture

Four processes per Playwright worker:

```
Playwright + Chromium            Vite dev server :5181  (PLAYWRIGHT_REAL=true)
  │  real Svelte frontend    ────┤  aliases @tauri-apps/api/core|event →
  │                              │  site/tests/real/tauri-shim-core.ts|-event.ts
  │  window.__FLTS_BRIDGE_PORT   │
  ▼
WebSocket  ws://127.0.0.1:<port>/bridge
  ▼
app binary (headless, --features e2e-bridge)   FLTS_E2E_BRIDGE_PORT=0
  │  real invoke handlers, real reqwest clients
  ▼  HTTP
flts-e2e-sims (one binary, three sims on three ephemeral ports)
     LLM (Gemini/OpenAI wire) · LRClib · AnkiConnect   + /_sim/* control API
```

- The shim turns every `invoke()` into a `{id, cmd, args}` frame on the bridge
  socket; the app answers `{id, ok}` / `{id, err}` and pushes `event` frames
  that `site/tests/real/tauri-shim-event.ts` fans out to `listen()` subscribers.
- `site/tests/real/fixtures.ts` owns the whole tree: it spawns the sims, reads their
  port JSON from stdout, writes a `config.json`, spawns the app, waits for its
  `FLTS_E2E_BRIDGE_LISTENING {...}` line, and hands specs a `harness`.
- `site/tests/real/global-setup.ts` builds both binaries before the run.

### Env-var seams

The app is redirected at the process boundary — no code branches on "test mode"
beyond the `e2e-bridge` feature.

| Var                                           | Effect                                                                                   |
| --------------------------------------------- | ---------------------------------------------------------------------------------------- |
| `FLTS_GEMINI_BASE_URL`                        | Gemini client base (`…/v1beta/`)                                                         |
| `OPENAI_BASE_URL`                             | plain-OpenAI client base                                                                 |
| `FLTS_DEEPSEEK_BASE_URL`, `FLTS_ZAI_BASE_URL` | OpenAI-compatible providers                                                              |
| `FLTS_LRCLIB_BASE_URL`                        | lyrics provider                                                                          |
| `ankiEndpoint` in `config.json`               | AnkiConnect (config, not env)                                                            |
| `FLTS_CONFIG_DIR`                             | per-worker temp dir: config, library **and** `<dir>/cache` (translation + lyrics caches) |
| `FLTS_KEYRING_SERVICE`                        | per-worker keychain service — never the developer's real `FLTS-Spotify` entry            |
| `FLTS_DISABLE_SYNC=1`                         | sync has its own Docker harness; off here                                                |
| `FLTS_ANKI_SYNC_INTERVAL_SECS`                | pinned high so only explicit `sync_anki_now` passes run                                  |
| `FLTS_E2E_BRIDGE_PORT=0`                      | ephemeral bridge port, printed on stdout                                                 |

## Running

```sh
pnpm test:e2e:real                      # builds binaries, then runs (~1.3m)
FLTS_E2E_SKIP_BUILD=1 pnpm test:e2e:real  # fast re-run, reuses target/debug
pnpm test:e2e:real:ui                   # Playwright UI mode
pnpm test:e2e:real:debug                # inspector
```

Build command behind the auto-build:

```sh
cargo build -p app -p e2e-sims --features app/e2e-bridge
```

**Gotcha:** a plain `cargo build -p app` (or `cargo tauri dev`) overwrites
`target/debug/app` with a _bridge-less_ binary. The next
`FLTS_E2E_SKIP_BUILD=1` run then hangs until the "app bridge line" timeout.
Rebuild with the feature, or drop `FLTS_E2E_SKIP_BUILD`.

On failure the worker keeps its config dir (path is logged and attached), and
each failing test attaches `app-stderr`, the three sims' rules + request logs,
and the config dir path.

## Adding a failure rule

Rules are pushed at runtime to a sim's `/_sim/rules` via
`harness.llm | .lrclib | .anki`. Shape (`SimRule` in `site/tests/real/sim-client.ts`,
`Rule` in `e2e-sims/src/rules.rs`, camelCase on the wire):

```ts
{
  matcher?: {           // omitted = matches everything
    method?: string;    // case-insensitive
    pathGlob?: string;  // '*' spans '/' too
    bodyContains?: string;
    nthCall?: number;   // 1-based, against the sim's total request count
  },
  action: { type: 'status' | 'delay' | 'stall' | 'drop' | 'truncate' | 'corrupt' | 'passthrough',
            code?, body?, ms?, afterBytes?, fraction?,
            // corrupt only; its values are snake_case, the wire's one exception:
            mode?: 'malformed_json' | 'wrong_content_type' | 'garbage' },
  times?: number,       // omitted = forever; n = fires n times then expires
}
```

First matching non-expired rule wins, in insertion order.

Example — transient 5xx, then recovery (the retry path):

```ts
await harness.llm.addRule({
  matcher: { pathGlob: "*streamGenerateContent*" },
  action: { type: "status", code: 503, body: { error: "sim overloaded" } },
  times: 2,
});
await btn.click();
await expectTranslated(p); // third attempt passes through
```

Example — a hung connection, released by a reset:

```ts
await harness.llm.addRule({
  matcher: { pathGlob: "*streamGenerateContent*" },
  action: { type: "stall" },
});
await btn.click();
await expect(btn).toBeDisabled(); // UI holds in-progress, does not fall over

await harness.llm.reset(); // the only stall release — also wipes scripts
await harness.llm.seed({
  scripts: [{ matchSubstring: text, translation: json }],
});
```

Seed shapes (`POST /_sim/seed`):

- LLM — `{ scripts: [{ matchSubstring, translation, stream?, chunks? }], fallback?: "minimal" }`
- LRClib — `[{ artist, title, album?, syncedLyrics?, plainLyrics? }]`
- Anki — `{ decks: [name], notes: [{ deck, model, fields, tags }] }`

The LLM seed **replaces** the script list; the LRClib and Anki seeds **merge**
into existing state (catalog `extend`, decks/notes appended). Use `reset()` to
clear.

Other control endpoints: `DELETE /_sim/rules`, `POST /_sim/reset`,
`GET /_sim/requests` (the request log used for traffic assertions). Control
routes are never faulted and never logged.

## Two tiers

- **Mock tier** (`site/playwright.config.ts`): the frontend against
  `site/tests/mocks/tauri-api.ts`, a TypeScript fake backend. No Rust, no network,
  seconds to run, `window.__test` hooks for arbitrary state. Inner loop.
- **Real tier** (`site/playwright.real.config.ts`): everything above. Catches what a
  fake backend cannot — wire formats, retry/timeout behavior, persistence,
  partial failure, restart recovery.

Legacy specs run in both tiers where the shared helper contract suffices
(`app`, `text-import`, `epub-import`, `chapters-panel`,
`chapter-translate-all`, `chapter-translation-ratio`); `site/tests/e2e/helpers/test.ts` picks the right `test` and
`site/tests/e2e/helpers/real-seed.ts` re-implements seeding by importing a real book and
scripting the LLM sim. The rest are listed in `site/playwright.real.config.ts`'s
`testIgnore`, each with the reason it cannot run here (mock-only `window.__test`
surfaces, seed fields the real pipeline cannot forge, segment text that
deliberately diverges from the original).

## Constraints for spec authors

- **Use per-test nonce text.** Translations are cached on disk by source text,
  and the cache dir outlives the per-test reset. Any spec asserting LLM traffic
  or re-translation needs textually unique paragraphs (and unique track
  name/artist for lyrics).
- **Chapters >0 work.** They used to hang on a stale summary-ready watch
  (fixed: `BookSummaryState::publish_ready` uses `send_replace`, and
  `wait_ready` quick-checks the sidecar). Seeding a paragraph in chapter K
  waits for summaries 0..K-1 to be generated by the LLM sim first, so give
  such specs a chapter count you're willing to pay summary calls for.
- **No failure UI for translations.** The contract is a `console.warn`
  (`` `Translation failed for paragraph ${paragraphId}:` `` — the paragraph id
  with a trailing colon, not an index) plus the paragraph reverting to
  untranslated. Assert on those, not on a toast.
- **The Node-side `harness.invoke` does not observe events.** `BridgeClient`
  only correlates command replies. Anything event-driven must be asserted in
  the page, or polled through a state-reading command.
- **`sim.reset()` clears everything**: seeds, rules, the request log and the
  `nthCall` counter, and it releases stalls. So re-seed after any reset, and
  don't expect pre-reset traffic to still be in `requests()` or to count toward
  `nthCall`. The per-test `autoReset` fixture already resets all three sims and
  deletes every book before each test.
- **Lyrics run through `e2e_resolve_track`.** There is no Spotify sim, and the
  lyrics UI needs a `spotify_state` event — so lyrics specs drive the bridge-only
  `e2e_resolve_track` command and assert on `get_track_lyrics_state` plus LRClib
  traffic.
- **A relaunched app answers only once it is configured.** `eval_config`
  (library open + anki sync task) is still spawned _after_ the bridge starts
  listening, but everything it installs now lives behind a readiness gate
  (`site/src-tauri/src/app/gated_state.rs`): commands that touch it await the
  startup outcome, up to 30s, and then answer for real — or return startup's own
  error. So the _first_ call after `restartApp()` is authoritative; don't poll
  for state to appear (see `restart-under-load.spec.ts`'s `expectLibraryHas`).
  What the gate does _not_ cover is the pass the relaunched app fires on its
  own: `sync_anki_now` can still lose the race with it ("anki sync already in
  progress" — one pass at a time, by design), which is the single transient
  `spec-helpers`' `syncNow` retries. `restartApp` takes `{ signal: 'SIGKILL' }`
  to skip the graceful shutdown entirely.
- **Anki has no UI here** either; `sync_anki_now` / `get_anki_sync_status` over
  the bridge, cards on disk under `<configDir>/library/cards/<pair>/`.

## Deferred

A Linux true-binary tier (the actual Tauri app under `tauri-driver` in Docker,
reusing these sims and env seams) is designed-for but not built — Playwright
cannot attach to WKWebView on macOS, and `tauri-driver` is Linux/Windows only.
See `docs/superpowers/specs/2026-08-16-e2e-real-backend-harness-design.md`
("Deferred").
