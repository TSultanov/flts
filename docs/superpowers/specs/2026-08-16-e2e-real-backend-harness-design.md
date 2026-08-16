# E2E Real-Backend Harness & Service Simulators — Design

**Date:** 2026-08-16
**Status:** Approved design, pre-implementation

## Goal

Truly end-to-end tests that exercise the real Rust backend and real HTTP
clients against full stateful simulators of the three external services FLTS
depends on — the LLM providers, the lyrics provider (LRClib), and Anki
(AnkiConnect) — with per-test failure injection (404s, 5xx, stalled
connections, dropped connections, incomplete or malformed payloads, latency).
Existing Playwright specs are reused where feasible; new failure-injection
specs are added.

## Key decisions (agreed during brainstorming)

- **Simulators in Rust**, one new workspace crate `e2e-sims`, single binary
  hosting all three sims.
- **Per-test HTTP control API** (`/_sim/*`) — tests program failures and seed
  state at runtime, including mid-test behavior changes (fail → recover).
- **Protocol-faithful cores + scripted content** — real wire protocols and
  stateful mechanics; response content is seeded/canned by tests.
- **App driving: invoke-bridge** — Playwright cannot attach to WKWebView on
  macOS and `tauri-driver` is Linux/Windows only. The real Tauri binary runs
  headless with a WebSocket bridge (feature-gated, in-binary — "Approach A");
  Playwright runs the real Svelte frontend in Chromium with a transport shim.
  A Linux true-binary smoke tier (tauri-driver in Docker) is designed-for but
  deferred.
- **Run targets:** local macOS now; harness kept free of Mac-only assumptions
  (ephemeral ports, no fixed paths) so CI wiring later is straightforward.
  CI setup itself is out of scope.
- **Test scope:** translation/LLM pipeline, lyrics import, Anki export, and
  cross-cutting resilience (no data loss, error surfacing, recovery after
  outage).
- **Out of scope:** Spotify simulation (hardcoded URLs + keychain + PKCE;
  the existing TS-mock tier keeps covering its UI), sync (has its own Docker
  harness; real-mode runs set `FLTS_DISABLE_SYNC=1`), CI pipeline setup.

## Current state (exploration findings)

- The existing Playwright suite (`site/tests/e2e/`, 19 specs) runs against the
  Vite dev server with a 1329-line TypeScript fake backend
  (`site/tests/mocks/tauri-api.ts`) aliased in at the `@tauri-apps/api`
  boundary when `PLAYWRIGHT=1` (`site/vite.config.ts:11-19`). No Rust, no
  network. It remains the fast inner-loop tier, untouched.
- Endpoint overridability today:
  - AnkiConnect: fully configurable via `ankiEndpoint` in `config.json`
    (`site/src-tauri/src/app/config.rs:199`).
  - OpenAI (plain): honors `OPENAI_BASE_URL` via async-openai defaults.
  - Gemini (`library/src/translator/gemini.rs:67`), DeepSeek / Z.AI
    (`library/src/translator/openai.rs:43-44`), LRClib
    (`library/src/lyrics/lrclib.rs:12`): **hardcoded**, need overrides.
- `FLTS_CONFIG_DIR` (`site/src-tauri/src/app.rs:105`) isolates config +
  library; API keys live in plaintext `config.json`, so seeding is a file
  write.
- Precedent for runtime fakes exists (`FLTS_MOCK_ANKICONNECT` →
  `MockAnkiConnect` at `library/src/anki/connect.rs:591`, `FLTS_MOCK_SYNC`);
  no LLM or lyrics doubles exist in Rust.

## Architecture

### Topology (per test worker)

```
Playwright (Chromium, real Svelte frontend + transport shim)
      │ WebSocket (invoke + events)
Tauri app binary  — `e2e-bridge` feature, headless, FLTS_E2E_BRIDGE_PORT
      │ HTTP (real reqwest / async-openai / gemini-rust clients)
flts-e2e-sims     — LLM sim, LRClib sim, AnkiConnect sim (3 ports)
      ▲ HTTP control (/_sim/*) from Playwright tests
```

Each Playwright worker owns its own app process, sims process, and temp
`FLTS_CONFIG_DIR`. All ports are ephemeral (bind port 0, report actual port)
so runs parallelize and never collide.

### Backend changes (small, at confirmed seams)

1. `FLTS_GEMINI_BASE_URL` — `gemini.rs:67` switches to
   `Gemini::with_model_and_base_url` when set (crate already exposes it).
2. `FLTS_DEEPSEEK_BASE_URL` / `FLTS_ZAI_BASE_URL` — override the consts at
   `openai.rs:43-44`. Plain OpenAI keeps honoring `OPENAI_BASE_URL`.
3. `FLTS_LRCLIB_BASE_URL` — override the const at `lrclib.rs:12`.
4. AnkiConnect: no change (config field exists).
5. New cargo feature `e2e-bridge` on `site/src-tauri`: compiled out of
   `release` / `release-ship`. With the feature built and
   `FLTS_E2E_BRIDGE_PORT` set, the app skips window creation and starts the
   bridge; everything else in `setup()` (config load, queues, anki sync loop)
   runs exactly as production wires it.

## The `e2e-sims` crate

Workspace member producing one binary `flts-e2e-sims`. Started with ephemeral
ports; prints bound ports as a JSON line on stdout for the harness.

### Shared fault-injection layer

A tower middleware, identical across sims. Behavior is an ordered rule list
programmed via the control API. Rule = matcher (method/path glob, optional
body predicate, optional nth-call) + action:

- `status(code, body?)` — 404/429/500/503 with optional custom payload
- `delay(ms)` — latency, then proceed
- `stall` — accept, optionally send headers, never complete; socket held
  until teardown or `/_sim/reset`
- `drop` — abrupt TCP close; optional `after_bytes(n)` for mid-body cuts
- `truncate(fraction)` — valid prefix of the real response, then cut
  (incomplete JSON / SSE)
- `corrupt(mode)` — malformed JSON, wrong content-type, garbage bytes
- `passthrough` (default) — hand to the stateful core

Rule lifetimes: `once`, `times(n)`, `always` — "fail twice then succeed"
retry tests are a single setup call.

### Control API (uniform, under `/_sim/`)

- `POST /_sim/rules` (append), `DELETE /_sim/rules` (clear)
- `POST /_sim/reset` — clear rules, tear down held sockets, reset core state
- `POST /_sim/seed` — install state (decks/notes, lyrics catalog, LLM
  scripts)
- `GET /_sim/requests` — request log (method, path, body, timestamp) for
  assertions like "backend retried 3×" or "no duplicate note posted"

### Stateful cores

- **AnkiConnect sim:** port of the semantics proven in `MockAnkiConnect`
  (decks, notes, cards, `multi` batching, version handshake,
  `{result, error}` envelope) behind real HTTP. State persists across calls
  within a run.
- **LRClib sim:** `GET /api/get` with real query params and response schema;
  seeded catalog keyed by artist/title/album/duration; unknown → real 404
  shape.
- **LLM sim:** speaks both wire protocols the app uses — Gemini `v1beta`
  (`generateContent`, streaming, and the cached-content endpoints used by
  `gemini_cache.rs`) and OpenAI-compatible chat completions (OpenAI /
  DeepSeek / Z.AI, incl. SSE streaming). Responses come from seeded scripts
  matched on request features; a default echo-style fallback returns
  schema-valid output so unscripted calls never wedge the app. Token-usage
  fields filled plausibly.

## Bridge & frontend transport shim

### Backend side (`site/src-tauri`, behind `e2e-bridge`)

Small axum WebSocket server started from `setup()` when
`FLTS_E2E_BRIDGE_PORT` is set (port 0 allowed; actual port printed to
stdout). One WS connection per Playwright page. JSON frames:

- `{id, cmd, args}` → dispatch to the real command handlers; reply
  `{id, ok, payload}` or `{id, err}`. The dispatch match is generated from
  the same command list `lib.rs:197-248` registers (macro or small codegen)
  so it cannot drift from the IPC surface.
- Events: a global listener forwards every backend `emit` as
  `{event, payload}` frames; the shim dispatches locally to `listen()`
  subscribers.

### Frontend side (`site/tests/`)

New vite mode `PLAYWRIGHT_REAL=1` aliases `@tauri-apps/api/core` and
`@tauri-apps/api/event` to a ~100-line transport shim (`invoke()` → WS
request/reply; `listen()` → local event dispatch). Bridge port injected via
`addInitScript`. Dialog/os plugin mocks stay as-is. Everything else is the
real production frontend served by Vite.

**Conformance risk & mitigation:** `invoke` arg casing (Tauri camelCases
args) and error shapes (serialized `Err`) must match Tauri IPC exactly. The
shim mirrors both, and a dedicated conformance spec calls every command once
against the real backend to pin the behavior.

## Seeding & spec reuse

Reuse works by **keeping the helper API and swapping its implementation** —
not porting specs one-by-one.

- `tests/e2e/helpers/` gains a backend-mode switch. In real mode,
  `seedAndOpen(book)` seeds through the real pipeline: real commands over the
  bridge (`import_epub` with a fixture EPUB from the existing
  `epub-generator.ts`, or a plain-text import path). Translation seeds become
  LLM-sim scripts (`/_sim/seed`) instead of `__mockTranslationCache`;
  lyrics/Anki seeds go to their sims. Mock-internal assertions (e.g.
  `getTranslateCalls`) are re-pointed at `/_sim/requests` behind the same
  helper.
- Playwright projects: existing `chromium` (mock tier, untouched, fast) plus
  a new `real` project running the same spec files where feasible;
  `testIgnore` for inherently mock-bound specs. Expectation: most specs run
  in both tiers via the helper swap; a minority need small edits or a
  real-mode variant. The mock tier remains the inner loop; the real tier is
  the integration truth.
- **New failure-injection specs** in `tests/e2e/real/`, real project only:
  - Translation: stall, truncation, malformed JSON, mid-stream cutoff,
    429/5xx retry behavior; a translations-never-vanish regression suite
    asserting persisted book state after error paths.
  - Lyrics: 404, empty result, malformed payload, slow response, UI error
    surfacing.
  - Anki: connection refused (Anki not running), mid-batch failure, stale
    deck state, duplicate-note prevention.
  - Cross-cutting: recovery after a service comes back, no data loss on
    save/retry paths.

### Isolation

Worker-scoped fixture spawns app + sims + temp `FLTS_CONFIG_DIR` once per
worker; between tests within a worker: `/_sim/reset` + fresh config/library
state. Process startup cost is paid per worker, not per test.

## Orchestration & observability

**Lifecycle** (worker-scoped Playwright fixture, TS):

1. Spawn `flts-e2e-sims`; read bound ports from stdout.
2. `mkdtemp` a `FLTS_CONFIG_DIR`; write `config.json` (fake API keys,
   `ankiEndpoint` → Anki sim).
3. Spawn the bridge-enabled app binary with `FLTS_E2E_BRIDGE_PORT=0`,
   `FLTS_*_BASE_URL` → sims, `FLTS_DISABLE_SYNC=1`; read bridge port.
4. Health-check bridge + sims; run tests.
5. Teardown: kill both processes, remove temp dir (kept on failure).

**Scripts:** `pnpm test:e2e:real` (+ `:ui` / `:debug`). Builds
`cargo build -p app --features e2e-bridge -p e2e-sims` by default, with an
opt-out env var for fast re-runs; fails fast with a clear message if binaries
are missing/stale.

**Failure diagnosis:** on test failure, Playwright attaches app stderr, each
sim's request log, and the active rule list. `/_sim/reset` tears down held
(stalled) sockets so a failing test can't hang the worker; the fixture also
enforces a hard per-test app-process timeout.

## Testing the test infrastructure

- `e2e-sims` Rust tests: protocol conformance of each core (AnkiConnect
  envelope, LRClib schema, Gemini/OpenAI response + SSE framing) and
  fault-layer semantics (once/times/always ordering, stall teardown).
- Bridge conformance spec (Section above): every command invoked once
  against the real backend.

## Deferred (designed-for, not built)

- **Linux true-binary smoke tier:** the actual Tauri app under
  `tauri-driver` (WebDriverIO) in Docker, reusing the same sims and env-var
  seams unchanged — only the driver layer differs.
- **CI pipeline:** harness is CI-ready (ephemeral ports, no Mac-only
  assumptions); wiring a workflow is a follow-up.
