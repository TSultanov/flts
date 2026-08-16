# E2E Real-Backend Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** E2E tests that run the real Rust backend against stateful Rust simulators of the LLM providers, LRClib, and AnkiConnect, with per-test failure injection, driven by Playwright through a WebSocket invoke-bridge.

**Architecture:** A new `e2e-sims` workspace crate hosts three axum-based simulators behind a shared fault-injection middleware with a `/_sim/*` control API. The real Tauri binary gains a feature-gated (`e2e-bridge`) headless mode exposing invoke dispatch + event forwarding over WebSocket. Playwright runs the real Svelte frontend in Chromium with a transport shim aliased in at the `@tauri-apps/api` boundary (same trick the existing mock tier uses).

**Tech Stack:** Rust (axum, tokio, tower, serde), TypeScript (Playwright 1.60, Vite 8, pnpm), Tauri 2.10.

**Spec:** `docs/superpowers/specs/2026-08-16-e2e-real-backend-harness-design.md`

## Global Constraints

- Package manager is **pnpm**, never npm. All JS commands run in `site/`.
- Rust edition 2024, workspace resolver 3. New crate joins `/Volumes/sources/flts/Cargo.toml` members.
- The `e2e-bridge` cargo feature MUST NOT be enabled by default and MUST NOT appear in `release-ship` builds. Verify with `cargo tree` / feature checks in Task 8.
- All listener ports are ephemeral (bind `127.0.0.1:0`, report the real port). Never hardcode a port.
- Every real-mode app process gets its own temp `FLTS_CONFIG_DIR` (mkdtemp). Never touch the user's real config (see memory: FLTS E2E must use a temp config dir).
- Comments: radically terse — whys/invariants only, never history or restated code.
- Commits end with:
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
- Skip `cargo mutants` runs; keep any `mutants::skip` annotations consistent with the codebase.
- The existing mock-tier Playwright suite (`pnpm test:e2e`) must stay green and untouched in behavior throughout.
- Env vars introduced by this plan (exact names): `FLTS_GEMINI_BASE_URL` (full base incl. `/v1beta/` and trailing slash), `FLTS_DEEPSEEK_BASE_URL`, `FLTS_ZAI_BASE_URL`, `FLTS_LRCLIB_BASE_URL` (origin only; code appends `/api/get`), `FLTS_E2E_BRIDGE_PORT`, `FLTS_E2E_SKIP_BUILD`. Plain OpenAI uses async-openai's existing `OPENAI_BASE_URL`.

---

## Phase 1 — Backend base-URL seams

### Task 1: Env-var base-URL overrides for Gemini, DeepSeek/Z.AI, LRClib

**Files:**
- Modify: `library/src/translator/gemini.rs:67-69` (`gemini_client`)
- Modify: `library/src/translator/openai.rs:61-79` (`openai_client`, `openai_compat_base_url`) and its call sites `openai.rs:92-95`, `library/src/lyrics/translation.rs:~174`
- Modify: `library/src/lyrics/lrclib.rs:12,95` (`LRCLIB_BASE`, `fetch_once`)
- Test: unit tests in the same files (`#[cfg(test)] mod` at bottom, matching file convention)

**Interfaces:**
- Consumes: nothing new.
- Produces: `gemini_client(api_key: String, model: Model) -> anyhow::Result<Gemini>` (unchanged signature, now env-aware); `openai_compat_base_url(provider: TranslationProvider) -> Option<String>` (**return type changes** from `Option<&'static str>`); `fn lrclib_get_url() -> String` in `lrclib.rs`.

Env-var reads are wrapped in pure, testable resolver functions; tests never mutate process env (Rust 2024 `set_var` is unsafe and env is process-global under parallel tests).

- [ ] **Step 1: Write failing unit tests**

In `gemini.rs` tests:

```rust
#[test]
fn base_url_override_parses() {
    assert!(gemini_base_url_override(None).is_none());
    assert!(gemini_base_url_override(Some(String::new())).is_none());
    let url = gemini_base_url_override(Some("http://127.0.0.1:4001/v1beta/".into())).unwrap();
    assert_eq!(url.as_str(), "http://127.0.0.1:4001/v1beta/");
}
```

In `openai.rs` tests:

```rust
#[test]
fn compat_base_url_env_resolution() {
    assert_eq!(resolve_compat_base(None, DEEPSEEK_BASE_URL), DEEPSEEK_BASE_URL);
    assert_eq!(
        resolve_compat_base(Some("http://127.0.0.1:4001".into()), DEEPSEEK_BASE_URL),
        "http://127.0.0.1:4001"
    );
    assert_eq!(resolve_compat_base(Some(String::new()), ZAI_BASE_URL), ZAI_BASE_URL);
}
```

In `lrclib.rs` tests:

```rust
#[test]
fn get_url_env_resolution() {
    assert_eq!(resolve_get_url(None), LRCLIB_BASE);
    assert_eq!(
        resolve_get_url(Some("http://127.0.0.1:4002/".into())),
        "http://127.0.0.1:4002/api/get"
    );
    assert_eq!(resolve_get_url(Some(String::new())), LRCLIB_BASE);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p library base_url` — expected: compile errors (functions not defined).

- [ ] **Step 3: Implement**

`gemini.rs` — check `gemini-rust` 1.7.1's exact constructor first (`~/.cargo/registry/src/*/gemini-rust-1.7.1/src/client.rs:1172-1183` — the crate exposes `with_model_and_base_url`; match its parameter types, the base URL is a `url::Url` or `String` depending on version):

```rust
fn gemini_base_url_override(raw: Option<String>) -> Option<url::Url> {
    raw.filter(|s| !s.is_empty()).and_then(|s| s.parse().ok())
}

pub(crate) fn gemini_client(api_key: String, model: Model) -> anyhow::Result<Gemini> {
    match gemini_base_url_override(std::env::var("FLTS_GEMINI_BASE_URL").ok()) {
        Some(url) => Ok(Gemini::with_model_and_base_url(api_key, model, url)?),
        None => Ok(Gemini::with_model(api_key, model)?),
    }
}
```

`openai.rs`:

```rust
fn resolve_compat_base(env_val: Option<String>, default: &str) -> String {
    env_val.filter(|s| !s.is_empty()).unwrap_or_else(|| default.to_string())
}

pub(crate) fn openai_compat_base_url(
    provider: crate::translator::TranslationProvider,
) -> Option<String> {
    use crate::translator::TranslationProvider::*;
    match provider {
        Deepseek => Some(resolve_compat_base(
            std::env::var("FLTS_DEEPSEEK_BASE_URL").ok(), DEEPSEEK_BASE_URL)),
        Zai => Some(resolve_compat_base(
            std::env::var("FLTS_ZAI_BASE_URL").ok(), ZAI_BASE_URL)),
        _ => None,
    }
}
```

Adapt call sites to the `Option<String>` return: `openai.rs:92-95` becomes `.and_then(openai_compat_base_url)` then `openai_client(api_key, base_url.as_deref())`; same pattern at `lyrics/translation.rs:~174`.

`lrclib.rs`:

```rust
fn resolve_get_url(env_origin: Option<String>) -> String {
    match env_origin.filter(|s| !s.is_empty()) {
        Some(origin) => format!("{}/api/get", origin.trim_end_matches('/')),
        None => LRCLIB_BASE.to_string(),
    }
}
```

and in `fetch_once` replace `client.get(LRCLIB_BASE)` with `client.get(resolve_get_url(std::env::var("FLTS_LRCLIB_BASE_URL").ok()))`.

- [ ] **Step 4: Run tests + full library suite**

Run: `cargo test -p library` — expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add library/src/translator/gemini.rs library/src/translator/openai.rs library/src/lyrics/lrclib.rs library/src/lyrics/translation.rs
git commit -m "feat: env-var base URL overrides for Gemini, DeepSeek, Z.AI, LRClib"
```

---

## Phase 2 — `e2e-sims` crate

### Task 2: Crate scaffold + fault rule engine (pure logic)

**Files:**
- Modify: `/Volumes/sources/flts/Cargo.toml` (add `"e2e-sims"` to members)
- Create: `e2e-sims/Cargo.toml`, `e2e-sims/src/lib.rs`, `e2e-sims/src/rules.rs`

**Interfaces:**
- Produces (used by Tasks 3-7):

```rust
// rules.rs — all types derive Debug, Clone, Serialize, Deserialize
pub struct Rule {
    pub matcher: Matcher,
    pub action: Action,
    /// None = always; Some(n) = fires n more times then expires.
    pub times: Option<u32>,
}
pub struct Matcher {
    pub method: Option<String>,       // "GET"/"POST", case-insensitive
    pub path_glob: Option<String>,    // '*' wildcard segments, e.g. "/v1beta/*"
    pub body_contains: Option<String>,
    pub nth_call: Option<u64>,        // 1-based, against this sim's total request count
}
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Status { code: u16, body: Option<serde_json::Value> },
    Delay { ms: u64 },                // then passthrough
    Stall,
    Drop { after_bytes: Option<usize> },
    Truncate { fraction: f32 },       // of the real response body
    Corrupt { mode: CorruptMode },
    Passthrough,
}
#[serde(rename_all = "snake_case")]
pub enum CorruptMode { MalformedJson, WrongContentType, Garbage }

pub struct RuleSet { /* Vec<Rule> + call counter */ }
impl RuleSet {
    pub fn push(&mut self, r: Rule);
    pub fn clear(&mut self);
    /// Increments call counter, returns first matching non-expired rule's
    /// Action (cloned), decrements its `times`, removes if expired.
    pub fn decide(&mut self, method: &str, path: &str, body: &[u8]) -> Action;
}
```

- [ ] **Step 1: Scaffold crate**

`e2e-sims/Cargo.toml` (reuse workspace deps where they exist — check `[workspace.dependencies]` in the root Cargo.toml first; add axum 0.8 / tower 0.5 / glob-free hand-rolled matching, no new glob dep):

```toml
[package]
name = "e2e-sims"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "flts-e2e-sims"
path = "src/main.rs"

[dependencies]
axum = "0.8"
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "net", "time", "io-util"] }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
anyhow = { workspace = true }
log = { workspace = true }
env_logger = "0.11"

[dev-dependencies]
reqwest = { workspace = true }
```

Add `"e2e-sims"` to workspace members. Create empty `src/main.rs` (`fn main() {}` placeholder until Task 7) and `src/lib.rs` with `pub mod rules;`.

- [ ] **Step 2: Write failing tests for the rule engine**

In `rules.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    fn status_rule(m: Matcher, times: Option<u32>) -> Rule {
        Rule { matcher: m, action: Action::Status { code: 503, body: None }, times }
    }
    #[test]
    fn empty_ruleset_passthrough() {
        let mut rs = RuleSet::default();
        assert!(matches!(rs.decide("GET", "/x", b""), Action::Passthrough));
    }
    #[test]
    fn glob_and_method_match() {
        let mut rs = RuleSet::default();
        rs.push(status_rule(Matcher { method: Some("post".into()),
            path_glob: Some("/v1beta/*".into()), body_contains: None, nth_call: None }, None));
        assert!(matches!(rs.decide("POST", "/v1beta/models/g:generateContent", b""),
            Action::Status { .. }));
        assert!(matches!(rs.decide("GET", "/v1beta/x", b""), Action::Passthrough));
        assert!(matches!(rs.decide("POST", "/api/get", b""), Action::Passthrough));
    }
    #[test]
    fn times_expires() {
        let mut rs = RuleSet::default();
        rs.push(status_rule(Matcher::default(), Some(2)));
        assert!(matches!(rs.decide("GET", "/", b""), Action::Status { .. }));
        assert!(matches!(rs.decide("GET", "/", b""), Action::Status { .. }));
        assert!(matches!(rs.decide("GET", "/", b""), Action::Passthrough)); // fail-twice-then-succeed
    }
    #[test]
    fn nth_call_and_body_contains() {
        let mut rs = RuleSet::default();
        rs.push(status_rule(Matcher { nth_call: Some(2), ..Default::default() }, None));
        assert!(matches!(rs.decide("GET", "/", b""), Action::Passthrough)); // call 1
        assert!(matches!(rs.decide("GET", "/", b""), Action::Status { .. })); // call 2
        let mut rs2 = RuleSet::default();
        rs2.push(status_rule(Matcher { body_contains: Some("needle".into()),
            ..Default::default() }, None));
        assert!(matches!(rs2.decide("POST", "/", b"hay needle stack"), Action::Status { .. }));
        assert!(matches!(rs2.decide("POST", "/", b"hay"), Action::Passthrough));
    }
    #[test]
    fn first_match_wins_in_insertion_order() {
        let mut rs = RuleSet::default();
        rs.push(Rule { matcher: Matcher::default(), action: Action::Delay { ms: 5 }, times: None });
        rs.push(status_rule(Matcher::default(), None));
        assert!(matches!(rs.decide("GET", "/", b""), Action::Delay { .. }));
    }
}
```

`Matcher` derives `Default`.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p e2e-sims` — expected: compile failure.

- [ ] **Step 4: Implement `rules.rs`**

Path glob: split pattern on `*`; match = starts_with(first piece) + contains subsequent pieces in order + ends_with(last piece) when pattern doesn't end in `*` (a ~15-line helper `glob_match(pattern, path) -> bool`, tested through the cases above). `decide` increments the sim-wide counter first, then scans rules in order.

- [ ] **Step 5: Run tests, verify pass, commit**

Run: `cargo test -p e2e-sims` — expected: PASS.

```bash
git add Cargo.toml Cargo.lock e2e-sims
git commit -m "feat: e2e-sims crate with fault-injection rule engine"
```

### Task 3: Fault middleware + control API + request log

**Files:**
- Create: `e2e-sims/src/fault.rs` (middleware applying `Action`s), `e2e-sims/src/control.rs` (`/_sim/*` router), `e2e-sims/src/server.rs` (compose: control + fault + inner router; bind ephemeral port)
- Modify: `e2e-sims/src/lib.rs` (`pub mod fault; pub mod control; pub mod server;`)
- Test: `e2e-sims/tests/fault_layer.rs`

**Interfaces:**
- Consumes: `RuleSet` from Task 2.
- Produces:

```rust
// server.rs
pub struct SimState {
    pub rules: Mutex<RuleSet>,
    pub log: Mutex<Vec<RequestRecord>>,     // RequestRecord { method, path, body: String, ts_ms: u128 }
    pub seed_reset: Box<dyn Fn() + Send + Sync>,   // resets the sim's stateful core
    pub stall_abort: tokio::sync::Notify,          // wakes stalled handlers on reset/teardown
}
/// Wraps `inner` (a sim's core Router) with the fault layer + /_sim routes,
/// binds 127.0.0.1:0, returns the bound port and the serve JoinHandle.
pub async fn serve(inner: axum::Router, state: Arc<SimState>) -> anyhow::Result<(u16, JoinHandle<()>)>;
```

Control endpoints (all JSON):
- `POST /_sim/rules` — body: single `Rule` or `[Rule]`; appends. 200 `{"count": <total>}`.
- `DELETE /_sim/rules` — clears rules. 200.
- `POST /_sim/reset` — clears rules + log, wakes all stalled handlers (they abort their connections), calls `seed_reset`. 200.
- `GET /_sim/requests` — 200 `[RequestRecord]` (excludes `/_sim/*` traffic itself).

Action semantics in `fault.rs` (axum `middleware::from_fn_with_state`, buffered request body so `body_contains`/log work):
- `Status` → respond immediately with code + body (default body `{}`).
- `Delay` → `tokio::time::sleep(ms)` then run inner.
- `Stall` → `state.stall_abort.notified().await` (never responds until reset/teardown; on notify, return a 599 that the closing connection makes moot).
- `Drop` → run inner, then stream `after_bytes` (default 0) of the real body and abruptly end the body stream with an io error (produces a client-side "connection closed before message completed").
- `Truncate` → run inner, send `floor(len * fraction)` bytes with the original headers/Content-Length of the FULL body, then end (client sees incomplete body / invalid JSON).
- `Corrupt` → run inner, replace body per mode: `MalformedJson` = valid body with last `}` removed and `{` prepended; `WrongContentType` = body intact, `Content-Type: text/html`; `Garbage` = 64 random-ish fixed bytes.
- `Passthrough` → run inner.

- [ ] **Step 1: Write failing integration tests**

`e2e-sims/tests/fault_layer.rs` — spin a trivial inner router (`GET /hello` → `{"msg":"hi"}`), then via `reqwest`:

```rust
#[tokio::test]
async fn passthrough_and_status_rule() { /* no rules → 200 {"msg":"hi"};
    POST /_sim/rules {"matcher":{},"action":{"type":"status","code":503},"times":1}
    → next GET /hello is 503, the one after is 200 */ }

#[tokio::test]
async fn request_log_records_calls() { /* GET /hello twice → GET /_sim/requests
    returns 2 records with method/path; /_sim/* absent */ }

#[tokio::test]
async fn truncate_yields_invalid_json() { /* truncate 0.5 rule → body parse fails */ }

#[tokio::test]
async fn stall_released_by_reset() { /* stall rule; spawn GET /hello; assert it
    does NOT complete within 300ms; POST /_sim/reset; the pending request now
    errors/completes quickly; subsequent GET /hello is 200 */ }

#[tokio::test]
async fn drop_closes_connection() { /* drop rule → reqwest error (incomplete message) */ }
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p e2e-sims --test fault_layer` fails to compile.

- [ ] **Step 3: Implement `fault.rs`, `control.rs`, `server.rs`**

Notes for the implementer: buffer the request with `axum::body::to_bytes` (cap 8 MB), rebuild the request for the inner service. For `Drop`/`Truncate`, buffer the inner response body too, then hand-construct the response with `Body::from_stream` over a stream that yields the prefix then `Err(io::Error...)` (Drop) or just ends early while Content-Length promises more (Truncate). For stall teardown use `tokio::sync::Notify` (`notify_waiters`). `ts_ms` from `SystemTime::now().duration_since(UNIX_EPOCH)`.

- [ ] **Step 4: Run tests, verify pass** — `cargo test -p e2e-sims`.

- [ ] **Step 5: Commit**

```bash
git add e2e-sims
git commit -m "feat: e2e-sims fault middleware, /_sim control API, request log"
```

### Task 4: AnkiConnect simulator core

**Files:**
- Create: `e2e-sims/src/anki.rs`
- Test: `e2e-sims/tests/anki_sim.rs`

**Interfaces:**
- Consumes: `server::serve`.
- Produces: `pub fn anki_router() -> (axum::Router, Arc<AnkiSimState>)` and `impl AnkiSimState { pub fn reset(&self) }` (wired as the sim's `seed_reset`). Seed shape accepted by `POST /_sim/seed`: `{"decks": ["FLTS"], "notes": [{"deck": "...", "model": "...", "fields": {..}, "tags": [..]}]}`.

**Reference material (read before implementing):**
- The wire protocol and semantics to reproduce: `library/src/anki/connect.rs` — the `AnkiConnect` trait at `:33` lists every action the app uses; `HttpAnkiConnect` (`:223`) shows the exact request envelope (`{"action", "version": 6, "params", "key"?}`) and response envelope (`{"result", "error"}`); `MockAnkiConnect` (`:559-1045`) is a proven in-process model of the semantics (decks, notes, cards, `multi` batching, per-sub-action error packaging) — port its state machine (`MockState`/`MockNote`/`MockCard`, `:561-588`) behind HTTP, dropping its test-instrumentation counters (`multi_call_count` etc. — the request log replaces them).
- What the sync engine actually calls: `library/src/anki/sync.rs`.

Single route: `POST /` dispatching on `action`. Support exactly the trait's action set (enumerate from `connect.rs:33` — includes `version`, `findNotes`, `notesInfo`, `cardsInfo`, `addNote`, `updateNoteFields`, `multi`, deck/model queries; implement each with the same success/error strings `MockAnkiConnect` produces, since `anki/sync.rs` pattern-matches some of them). Unknown action → `{"result": null, "error": "unsupported action: <name>"}`.

- [ ] **Step 1: Write failing tests** — `tests/anki_sim.rs`:

```rust
#[tokio::test]
async fn version_handshake() { /* {"action":"version","version":6} → {"result":6,"error":null} */ }

#[tokio::test]
async fn add_then_find_then_info_roundtrip() { /* seed a deck; addNote with tag
    "flts-test"; findNotes {"query":"tag:flts-test"} returns the id;
    notesInfo returns fields; cardsInfo returns a card referencing the note */ }

#[tokio::test]
async fn multi_batches_and_isolates_errors() { /* multi with [good addNote, addNote
    into missing deck] → result array len 2, second entry carries error, first
    succeeded and is findable */ }

#[tokio::test]
async fn state_survives_across_requests_and_resets() { /* addNote; /_sim/reset;
    findNotes → empty */ }
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p e2e-sims --test anki_sim`.
- [ ] **Step 3: Implement `anki.rs`** by porting `MockAnkiConnect`'s handlers verbatim in behavior (same id allocation, same error strings, same `multi` envelope — `connect.rs:680-1045`).
- [ ] **Step 4: Run tests, verify pass.**
- [ ] **Step 5: Commit** — `git commit -m "feat: AnkiConnect simulator core"`

### Task 5: LRClib simulator core

**Files:**
- Create: `e2e-sims/src/lrclib.rs`
- Test: `e2e-sims/tests/lrclib_sim.rs`

**Interfaces:**
- Produces: `pub fn lrclib_router() -> (axum::Router, Arc<LrclibSimState>)`. Seed shape: `[{"artist": "...", "title": "...", "album": null, "syncedLyrics": "[00:01.00] line", "plainLyrics": "line"}]` (either lyrics field nullable).

**Reference:** the client is `library/src/lyrics/lrclib.rs` — query params `artist_name`, `track_name`, optional `album_name`, `duration` (`:82-93`); the app deserializes only `syncedLyrics`/`plainLyrics` (`:43-49`); 404 = not found; non-2xx = error.

Route: `GET /api/get`. Lookup: exact match on (artist, title), ignoring album/duration (recorded in the log for assertions). Hit → 200 with the full real-shaped record (`{"id": 1, "trackName", "artistName", "albumName", "duration": 0, "instrumental": false, "plainLyrics", "syncedLyrics"}`); miss → 404 `{"statusCode": 404, "name": "TrackNotFound", "message": "Failed to find specified track"}` (LRClib's real 404 body).

- [ ] **Step 1: Failing tests** — seeded hit returns syncedLyrics; unseeded → 404 with that body; reset empties the catalog.
- [ ] **Step 2: Verify failure.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify pass.**
- [ ] **Step 5: Commit** — `git commit -m "feat: LRClib simulator core"`

### Task 6: LLM simulator core (Gemini + OpenAI-compatible)

**Files:**
- Create: `e2e-sims/src/llm.rs`
- Test: `e2e-sims/tests/llm_sim.rs`

**Interfaces:**
- Produces: `pub fn llm_router() -> (axum::Router, Arc<LlmSimState>)`. Seed shape:

```json
{"scripts": [{"matchSubstring": "<text found in the request body>",
              "translation": { /* verbatim paragraph-translation JSON the model would emit */ },
              "stream": true, "chunks": 5}],
 "fallback": "minimal"}
```

**Reference material:**
- Gemini protocol as consumed: `library/src/translator/gemini.rs` (streaming via `TryStreamExt`) and `library/src/translator/gemini_cache.rs` (cachedContents endpoints). gemini-rust 1.7.1 sources in `~/.cargo/registry/` show exact paths: `POST {base}models/{model}:generateContent`, `POST {base}models/{model}:streamGenerateContent?alt=sse`, and `POST/DELETE {base}cachedContents[/{name}]`.
- OpenAI protocol: async-openai 0.34 — `POST {api_base}/chat/completions`, SSE chunks `data: {json}\n\n`, terminated by `data: [DONE]\n\n`. The app requests `response_format: json_schema` and streams (`library/src/translator/openai.rs:110+`).
- The translation JSON the app parses: `paragraph_translation_schema()` in `library/src/translator.rs` and the importer `library/src/book/translation_import.rs` (field meanings: `s` = sentences, `wl` = word list, `o` = original, etc.).

Routes (register both with and without a `/v1` prefix for the OpenAI family, since `OPENAI_BASE_URL` conventionally includes `/v1` while DeepSeek's base does not):
- `POST /v1beta/models/{model}:generateContent` → `{"candidates":[{"content":{"parts":[{"text": "<translation JSON as string>"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":20,"totalTokenCount":30}}`
- `POST /v1beta/models/{model}:streamGenerateContent` → SSE: the same shape split into N chunks, each `data: {candidates:[{content:{parts:[{text: "<piece>"}]}}]}\n\n`, final chunk carries `finishReason` + usage.
- `POST /v1beta/cachedContents` → `{"name": "cachedContents/sim-1", ...}` (store name; DELETE removes; unknown name → 404 with Google error JSON so `is_cache_missing_error` paths are exercisable).
- `POST /chat/completions` and `/v1/chat/completions` → non-stream: standard completion object with `choices[0].message.content` = translation JSON string, `finish_reason: "stop"`; stream: SSE delta chunks then `[DONE]`.

Script matching: first script whose `matchSubstring` occurs in the raw request body; else fallback = a minimal valid translation object (single sentence, single word `{"o": "sim"}`-style, conforming to the schema the importer accepts — build it once by hand from `translation_import.rs` and assert it imports in Step 1's roundtrip test).

- [ ] **Step 1: Failing tests** — gemini non-stream returns scripted JSON in `parts[0].text`; gemini SSE streams N chunks reassembling to the script; openai stream terminates with `[DONE]`; cachedContents create→delete→use-after-delete-404; fallback response passes `serde_json::from_str::<serde_json::Value>` and contains the schema's required top-level keys.
- [ ] **Step 2: Verify failure.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify pass.**
- [ ] **Step 5: Commit** — `git commit -m "feat: LLM simulator core (Gemini + OpenAI wire protocols)"`

### Task 7: `flts-e2e-sims` binary

**Files:**
- Create: `e2e-sims/src/main.rs`
- Test: `e2e-sims/tests/binary_smoke.rs`

**Interfaces:**
- Produces: binary that starts all three sims on ephemeral ports and prints exactly one stdout line the harness parses:

```json
{"llm": 49321, "lrclib": 49322, "anki": 49323}
```

then serves until SIGTERM/stdin EOF (exit on stdin close so orphaned sims die with the test runner).

- [ ] **Step 1: Failing test** — `tests/binary_smoke.rs` spawns the binary via `std::process::Command` (`env!("CARGO_BIN_EXE_flts-e2e-sims")`), reads the JSON line, hits each port's `/_sim/requests` (200 `[]`), then closes stdin and asserts exit within 2s.
- [ ] **Step 2: Verify failure.**
- [ ] **Step 3: Implement `main.rs`** — tokio main, three `server::serve` calls, print line, `tokio::io::stdin().read(...)` until EOF.
- [ ] **Step 4: Verify pass** (`cargo test -p e2e-sims`), and run the full crate suite once.
- [ ] **Step 5: Commit** — `git commit -m "feat: flts-e2e-sims binary hosting all three simulators"`

---

## Phase 3 — Invoke bridge

### Task 8: `e2e-bridge` feature — headless mode + WS server + first dispatch slice

**Files:**
- Modify: `site/src-tauri/Cargo.toml` (feature + optional deps), `site/src-tauri/src/lib.rs` (context window-clear + bridge spawn in setup)
- Create: `site/src-tauri/src/bridge.rs`
- Test: `site/src-tauri/src/bridge.rs` unit tests + drift test

**Interfaces:**
- Produces: with feature `e2e-bridge` compiled and `FLTS_E2E_BRIDGE_PORT` set (use `0` for ephemeral), the app runs windowless and serves WS at `ws://127.0.0.1:<port>/bridge`, printing `FLTS_E2E_BRIDGE_LISTENING {"port": <port>}` to stdout. Frame protocol (all JSON text frames):
  - client→server: `{"id": 1, "cmd": "get_config", "args": {}}`
  - server→client: `{"id": 1, "ok": <payload>}` | `{"id": 1, "err": <serialized command error>}` | `{"event": "book_updated", "payload": <json>}`
  - Args arrive with the same camelCase keys the frontend passes to `invoke()`; the dispatcher deserializes them into per-command `#[serde(rename_all = "camelCase")]` structs mirroring Tauri's IPC convention.

- [ ] **Step 1: Cargo feature**

```toml
[features]
e2e-bridge = ["dep:axum", "dep:futures-util"]

[dependencies]
axum = { version = "0.8", features = ["ws"], optional = true }
futures-util = { version = "0.3", optional = true }
```

Verify isolation: `cargo tree -p app -e features | grep -c axum` is 0 without the feature; `cargo build -p app --profile release-ship` must not include it (feature is opt-in only — confirm no default wiring).

- [ ] **Step 2: Headless context + spawn**

In `lib.rs` `run()` where `tauri::generate_context!()` is passed to `.build(...)`, hoist it into a binding first:

```rust
let mut context = tauri::generate_context!();
#[cfg(feature = "e2e-bridge")]
if std::env::var("FLTS_E2E_BRIDGE_PORT").is_ok() {
    context.config_mut().app.windows.clear(); // headless: bridge replaces the webview
}
```

and at the end of the existing `.setup(...)` closure:

```rust
#[cfg(feature = "e2e-bridge")]
if let Ok(port) = std::env::var("FLTS_E2E_BRIDGE_PORT") {
    crate::bridge::spawn(app.handle().clone(), port.parse()?);
}
```

- [ ] **Step 3: Write the drift test (failing)**

In `bridge.rs`:

```rust
#[test]
fn bridge_covers_all_registered_commands() {
    let src = include_str!("lib.rs");
    let block = src.split("generate_handler![").nth(1).unwrap()
        .split(']').next().unwrap();
    let registered: Vec<&str> = block.lines()
        .filter_map(|l| l.trim().trim_end_matches(',').rsplit("::").next())
        .filter(|s| !s.is_empty()).collect();
    for cmd in registered {
        assert!(COMMANDS.contains(&cmd), "bridge missing command: {cmd}");
    }
}
```

`COMMANDS: &[&str]` is the bridge's dispatch list. Run `cargo test -p app --features e2e-bridge bridge_covers` — fails (module absent).

- [ ] **Step 4: Implement `bridge.rs` with a first slice**

Structure: `pub fn spawn(app: AppHandle, port: u16)` → tokio task, axum router `GET /bridge` WS upgrade. Per connection: read loop parses frames; each command runs in its own `tauri::async_runtime::spawn` so a slow command never blocks the socket; replies go through an `mpsc` writer task. Dispatch:

```rust
async fn dispatch(app: &AppHandle, cmd: &str, args: Value) -> Result<Value, Value> {
    let state = app.state::<Arc<crate::app::AppState>>();
    match cmd {
        "get_config" => wrap(crate::app::get_config(state).await),
        "update_config" => {
            #[derive(serde::Deserialize)] #[serde(rename_all = "camelCase")]
            struct A { config: crate::app::config::Config }
            let a: A = args_of(args)?;
            wrap(crate::app::update_config(app.clone(), state, a.config).await)
        }
        // ...one arm per command; signatures come from the command fns themselves —
        // open each module (app.rs, app/config.rs, app/library_view.rs, app/lyrics.rs,
        // app/sync.rs, app/spotify/web.rs) and mirror the exact parameter names/types.
        // Tauri IPC camelCases each Rust snake_case parameter; replicate per-arg.
        other => Err(json!(format!("unknown command: {other}"))),
    }
}
fn wrap<T: serde::Serialize, E: serde::Serialize>(r: Result<T, E>) -> Result<Value, Value> { ... }
```

First slice for this task: the read-only/config commands (`get_config`, `get_models`, `get_languages`, `parse_language_id`, `get_library_root`, `get_translation_providers`, `list_books`) + `COMMANDS` listing ONLY the implemented ones — the drift test stays red until Task 9 and is `#[ignore]`d in this commit with a `// until task 9` marker... **No — plans don't defer failing tests.** Instead: implement the full `COMMANDS` list now with unimplemented arms returning `Err("not yet bridged")` is also a lie. Correct approach: this task implements ALL arms mechanically (they are ~48 small arms of the same shape; the module signatures are all in the five files listed above), and the drift test passes at the end of this task. Tauri commands taking `Window`/`Webview` params: none exist in this codebase (all take `AppHandle`/`State` — verify by grepping `tauri::command` fns; if one does surface, bridge it with a clear `Err("requires webview")`).

- [ ] **Step 5: Boot smoke test**

Manual verification (documented, not automated here — Task 11 automates):

```bash
cargo build -p app --features e2e-bridge
FLTS_E2E_BRIDGE_PORT=0 FLTS_CONFIG_DIR=$(mktemp -d) FLTS_DISABLE_SYNC=1 ./target/debug/app &
# expect: FLTS_E2E_BRIDGE_LISTENING {"port":NNN} on stdout, no window appears
```

Plus `cargo test -p app --features e2e-bridge` (drift test passes) and `cargo build -p app` (feature off still compiles).

- [ ] **Step 6: Commit**

```bash
git add site/src-tauri
git commit -m "feat: e2e-bridge feature — headless WS invoke bridge with full command dispatch"
```

### Task 9: Event forwarding over the bridge

**Files:**
- Modify: `site/src-tauri/src/bridge.rs`

**Interfaces:**
- Produces: every backend `emit` of the known event set is forwarded to every connected WS client as `{"event": name, "payload": <json>}`.

The event name list (grep-verified; keep as a const with a terse comment noting it must track backend `emit` call sites):

```rust
const FORWARDED_EVENTS: &[&str] = &[
    "anki_sync_status_changed", "book_updated", "cards_updated", "config_updated",
    "library_updated", "lyrics_resolved", "lyrics_translation_done",
    "lyrics_translation_error", "lyrics_translation_progress",
    "paragraph_translation_finished", "paragraph_translation_progress",
    "paragraph_translation_started", "paragraph_updated", "spotify_queue",
    "spotify_state", "summary_generation_progress", "sync_status_changed",
];
```

- [ ] **Step 1: Drift test for the list**

Same `include_str!` technique is impractical across many files; instead a unit test greps at build time is fragile. Use a `#[test]` that runs over the source tree via `std::fs` from `CARGO_MANIFEST_DIR`:

```rust
#[test]
fn forwarded_events_track_emit_sites() {
    // scan site/src-tauri/src/**/*.rs for `.emit("name"` occurrences;
    // every found name must be in FORWARDED_EVENTS (superset is allowed).
}
```

Write it first; it fails (const absent).

- [ ] **Step 2: Implement** — on WS connect, for each name in `FORWARDED_EVENTS`: `app.listen(name, move |ev| { let payload = serde_json::from_str::<Value>(ev.payload()).unwrap_or_else(|_| Value::String(ev.payload().into())); let _ = tx.send(frame); })`; collect the `EventId`s and `app.unlisten(id)` them on disconnect.

- [ ] **Step 3: Run** `cargo test -p app --features e2e-bridge` — PASS.
- [ ] **Step 4: Commit** — `git commit -m "feat: bridge event forwarding"`

---

## Phase 4 — Frontend shim, harness, conformance

### Task 10: Transport shim + `PLAYWRIGHT_REAL` vite mode

**Files:**
- Create: `site/tests/real/bridge-transport.ts`, `site/tests/real/tauri-shim-core.ts`, `site/tests/real/tauri-shim-event.ts`
- Modify: `site/vite.config.ts`
- Test: `site/tests/real/bridge-transport.test.ts` (vitest, against a tiny in-test `ws` echo server — add dev-dep `ws` via `pnpm add -D ws @types/ws`)

**Interfaces:**
- Consumes: bridge frame protocol (Task 8/9). Bridge port read from `(window as any).__FLTS_BRIDGE_PORT` (set by the fixture's `addInitScript`).
- Produces: shim modules whose export surface mirrors what the app imports from `@tauri-apps/api/core` and `/event` — **copy the export list from the existing mocks** `site/tests/mocks/tauri-api.ts` / `tauri-event.ts` (at minimum `invoke`, `listen`, `once`, `emit`; keep signatures identical to `@tauri-apps/api` types).

`bridge-transport.ts` core:

```ts
type Pending = { resolve: (v: unknown) => void; reject: (e: unknown) => void };
let socket: WebSocket | null = null;
let ready: Promise<void> | null = null;
let nextId = 1;
const pending = new Map<number, Pending>();
const handlers = new Map<string, Set<(payload: unknown) => void>>();

function ensureConnected(): Promise<void> {
  if (ready) return ready;
  ready = new Promise((resolve, reject) => {
    const port = (window as any).__FLTS_BRIDGE_PORT;
    if (!port) return reject(new Error('bridge port not injected'));
    socket = new WebSocket(`ws://127.0.0.1:${port}/bridge`);
    socket.onopen = () => resolve();
    socket.onerror = (e) => reject(e);
    socket.onmessage = (msg) => {
      const frame = JSON.parse(msg.data as string);
      if (frame.id !== undefined) {
        const p = pending.get(frame.id);
        if (!p) return;
        pending.delete(frame.id);
        'err' in frame ? p.reject(frame.err) : p.resolve(frame.ok);
      } else if (frame.event) {
        handlers.get(frame.event)?.forEach((h) => h(frame.payload));
      }
    };
  });
  return ready;
}

export async function bridgeInvoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  await ensureConnected();
  return new Promise<T>((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
    socket!.send(JSON.stringify({ id, cmd, args }));
  });
}

export async function bridgeListen(event: string, handler: (e: { event: string; payload: unknown }) => void): Promise<() => void> {
  await ensureConnected();
  let set = handlers.get(event);
  if (!set) handlers.set(event, (set = new Set()));
  const wrapped = (payload: unknown) => handler({ event, payload });
  set.add(wrapped);
  return () => set!.delete(wrapped);
}
```

`tauri-shim-core.ts`: `export const invoke = bridgeInvoke;` plus any other core exports the app uses. `tauri-shim-event.ts`: `listen`/`once` built on `bridgeListen` (match `@tauri-apps/api/event`'s `UnlistenFn` promise shape exactly — the frontend `await`s it).

`vite.config.ts`: add a `PLAYWRIGHT_REAL` branch beside the existing `PLAYWRIGHT` one — core/event → the shims; **dialog/os keep the existing mocks** (`tests/mocks/tauri-dialog.ts`, `tauri-os.ts`); reuse the same `optimizeDeps.exclude` list under `PLAYWRIGHT_REAL` (the two-module-instances hazard in the existing comment applies identically).

- [ ] **Step 1: Write failing vitest** — start a `ws` server in-test speaking the frame protocol (reply `{id, ok: {echo: args}}`, push one `{event: 'book_updated', payload: 'x'}` after first invoke); set `(globalThis as any).__FLTS_BRIDGE_PORT`; assert invoke resolves, err frame rejects, listener fires, unlisten stops it. (jsdom lacks WebSocket — run this test file with `// @vitest-environment node` and polyfill `window` as `globalThis`, or inject a WebSocket impl; keep it simple with `ws`'s `WebSocket` assigned to `globalThis.WebSocket`.)
- [ ] **Step 2: Verify failure** — `pnpm vitest run tests/real/bridge-transport.test.ts`.
- [ ] **Step 3: Implement shims + vite branch.**
- [ ] **Step 4: Verify pass**, plus `pnpm check`, plus `pnpm test:e2e --project=chromium -g "smoke" ` (or the fastest existing spec) to prove the mock tier is unaffected.
- [ ] **Step 5: Commit** — `git commit -m "feat: real-mode Tauri transport shim over the bridge"`

### Task 11: Real-mode Playwright config, worker fixtures, sim clients, smoke spec

**Files:**
- Create: `site/playwright.real.config.ts`, `site/tests/real/fixtures.ts`, `site/tests/real/sim-client.ts`, `site/tests/real/global-setup.ts`, `site/tests/e2e/real/smoke.spec.ts`
- Modify: `site/package.json` (scripts)

**Interfaces:**
- Consumes: `flts-e2e-sims` stdout port JSON (Task 7), bridge stdout line (Task 8), env vars from Global Constraints.
- Produces (used by Tasks 12-17):

```ts
// fixtures.ts
export type RealHarness = {
  llm: SimClient; lrclib: SimClient; anki: SimClient;
  bridgePort: number; configDir: string;
  appStderr: () => string;          // captured, attached on failure
};
export const test: TestType<{ page: Page }, { harness: RealHarness }>; // harness is worker-scoped
export { expect } from '@playwright/test';
// sim-client.ts
export class SimClient {
  constructor(baseUrl: string);
  addRule(rule: SimRule): Promise<void>;        // POST /_sim/rules
  clearRules(): Promise<void>;
  reset(): Promise<void>;                        // POST /_sim/reset
  seed(data: unknown): Promise<void>;            // POST /_sim/seed
  requests(): Promise<Array<{ method: string; path: string; body: string; tsMs: number }>>;
}
export type SimRule = { matcher?: { method?: string; pathGlob?: string; bodyContains?: string; nthCall?: number };
                        action: { type: 'status'|'delay'|'stall'|'drop'|'truncate'|'corrupt'|'passthrough';
                                  code?: number; body?: unknown; ms?: number; afterBytes?: number;
                                  fraction?: number; mode?: string };
                        times?: number };
```

(Note: SimRule uses camelCase on the wire ⇒ Task 2/3's serde derives need `#[serde(rename_all = "camelCase")]` — add that in Task 2 from the start.)

Config: `playwright.real.config.ts` — `testDir: './tests/e2e'`, single project `real` (Desktop Chrome), `baseURL: 'http://localhost:5181'`, `webServer: { command: 'PLAYWRIGHT_REAL=true pnpm dev --port 5181', url: 'http://localhost:5181' }`, `globalSetup: './tests/real/global-setup.ts'`, `testIgnore` initialized to ALL existing specs (only `tests/e2e/real/**` runs; Task 13 shrinks the ignore list), `fullyParallel: true`, `workers: 4`.

`global-setup.ts`: unless `FLTS_E2E_SKIP_BUILD=1`, run `cargo build -p app --features e2e-bridge -p e2e-sims` (spawnSync, cwd repo root, stdio inherit; throw with a clear message on failure).

`fixtures.ts` worker fixture `harness`:
1. Spawn `<repoRoot>/target/debug/flts-e2e-sims`; parse the stdout JSON line (5s timeout).
2. `fs.mkdtempSync(os.tmpdir() + '/flts-e2e-')`; write `config.json`:

```json
{"targetLanguageId": "eng", "translationProvider": "google",
 "geminiApiKey": "sim-key", "model": "Gemini25Flash",
 "ankiEndpoint": "http://127.0.0.1:<anki>", "syncEnabled": false}
```

(Field names/values must match `site/src-tauri/src/app/config.rs:165-249` serde exactly — verify `model` enum string against `TranslationModel` serialization before hardcoding.)
3. Spawn `<repoRoot>/target/debug/app` with env: `FLTS_E2E_BRIDGE_PORT=0`, `FLTS_CONFIG_DIR`, `FLTS_GEMINI_BASE_URL=http://127.0.0.1:<llm>/v1beta/`, `OPENAI_BASE_URL=http://127.0.0.1:<llm>/v1`, `FLTS_DEEPSEEK_BASE_URL=http://127.0.0.1:<llm>`, `FLTS_ZAI_BASE_URL=http://127.0.0.1:<llm>`, `FLTS_LRCLIB_BASE_URL=http://127.0.0.1:<lrclib>`, `FLTS_DISABLE_SYNC=1`, `FLTS_ANKI_SYNC_INTERVAL_SECS=3600`; parse `FLTS_E2E_BRIDGE_LISTENING {"port":N}` from stdout (10s timeout); capture stderr into a ring buffer.
4. Teardown: SIGTERM both, SIGKILL after 3s, `rm -rf` configDir unless any test in the worker failed (then log the kept path).

Test-scoped auto fixture: before each test — `await Promise.all([llm, lrclib, anki].map(s => s.reset()))`; wipe the app's library state by removing `<configDir>/library`'s book dirs via a bridge `delete_book` sweep (`list_books` then delete each) so tests stay independent without restarting the app. On test failure: attach app stderr + each sim's `requests()` dump via `testInfo.attach`.

Page fixture override: `page.addInitScript` injecting `__FLTS_BRIDGE_PORT = <bridgePort>` before every navigation.

`smoke.spec.ts`:

```ts
import { test, expect } from '../../real/fixtures';

test('app boots headless and serves real config over the bridge', async ({ page, harness }) => {
  await page.goto('/');
  const cfg = await page.evaluate(() =>
    (window as any).__bridgeDebugInvoke?.('get_config') ??
    import('@tauri-apps/api/core').then((m) => m.invoke('get_config')));
  expect(cfg).toMatchObject({ translationProvider: 'google' });
});

test('lrclib request reaches the sim', async ({ page, harness }) => {
  await harness.lrclib.seed([{ artist: 'A', title: 'T', plainLyrics: 'la' }]);
  // exercised fully in Task 15; here just prove wiring:
  const reqs = await harness.lrclib.requests();
  expect(Array.isArray(reqs)).toBe(true);
});
```

(For the evaluate-import trick to work, the shim also assigns `window.__bridgeDebugInvoke = bridgeInvoke` — add that export in Task 10's shim; terse comment: test-only escape hatch.)

Scripts in `package.json`:

```json
"test:e2e:real": "playwright test -c playwright.real.config.ts",
"test:e2e:real:ui": "playwright test -c playwright.real.config.ts --ui",
"test:e2e:real:debug": "playwright test -c playwright.real.config.ts --debug"
```

- [ ] **Step 1: Write config, fixtures, sim-client, global-setup, smoke spec** (the fixture IS the test rig; the smoke spec is its failing test).
- [ ] **Step 2: Run** `pnpm test:e2e:real` — iterate until the smoke spec passes. Expected first-run failures to debug in order: binary paths, config.json field mismatches, bridge port parsing, shim connection.
- [ ] **Step 3: Verify isolation** — run twice concurrently (`pnpm test:e2e:real & pnpm test:e2e:real`); both pass (ephemeral ports, separate config dirs).
- [ ] **Step 4: Commit** — `git commit -m "feat: real-mode Playwright harness, fixtures, sim clients, smoke spec"`

### Task 12: Bridge conformance spec

**Files:**
- Create: `site/tests/e2e/real/bridge-conformance.spec.ts`

**Interfaces:** consumes fixtures + `window.__bridgeDebugInvoke`.

One test iterating a table of ALL ~48 commands (copy names from `site/src-tauri/src/lib.rs` `generate_handler!` block) with minimal valid args (e.g. `list_books: {}`, `get_word_info: {bookId: <seeded>, ...}`; commands needing real entities run after seeding a book via `import_plain_text`). Assertion per command: the invoke settles (resolves OR rejects with a **serialized error value**, never a shim/transport throw like "unknown command" or a WS close). Commands with side effects that would disturb others (`delete_book`, `update_config`) run last / against throwaway entities. Spotify commands may reject (no credentials) — that's a pass (serialized error).

- [ ] **Step 1: Write the spec with the full command table.**
- [ ] **Step 2: Run** `pnpm test:e2e:real -g conformance` — fix bridge arms it flushes out (arg-name casing bugs live here).
- [ ] **Step 3: Commit** — `git commit -m "test: bridge conformance spec covering every command"`

### Task 13: Helper backend-mode switch + enable reusable existing specs

**Files:**
- Create: `site/tests/e2e/helpers/backend-mode.ts`, `site/tests/e2e/helpers/real-seed.ts`
- Modify: `site/tests/e2e/helpers/paragraph.ts` (`seedAndOpen`, `getTranslateCalls`), `site/playwright.real.config.ts` (`testIgnore` shrink)

**Interfaces:**
- Produces: `isRealMode(): boolean` (reads `process.env.PLAYWRIGHT_REAL` at config level, threaded via Playwright project `metadata`/env into helpers — simplest: `!!process.env.PLAYWRIGHT_REAL` works since helpers run in the runner process for setup and use page-side only for evaluate); `seedAndOpen(page, spec, opts)` keeps its exact signature and returns `{ bookId }` in both modes.

Real-mode `seedAndOpen` (in `real-seed.ts`, called from `paragraph.ts` when `isRealMode()`):
1. Build plain text from `spec.chapters[].paragraphs[].html` (strip tags; blank-line paragraph separators; chapter titles as headings) and call `import_plain_text` over the bridge (open its signature in `site/src-tauri/src/app/library_view.rs` first and match args exactly; it returns/it derives the real `bookId` — return that, ignoring `spec.bookId`).
2. For each `translateConfigs` entry: convert the `segments` word list into a paragraph-translation JSON (same conversion the TS mock does — port the relevant builder from `tests/mocks/tauri-api.ts`) and `harness.llm.seed({scripts: [{matchSubstring: <paragraph text>, translation: <json>}]})`. `kind: 'error'` configs become an LLM `status 500` rule scoped by `bodyContains` on the paragraph text.
3. Unsupported seed fields in real mode (`inFlight`, `summaryStatus`, `wordInfos` pre-seeding, `readingState`) — throw `new Error('not supported in real mode: <field>')` so specs using them fail loudly and stay on the ignore list.
4. `page.goto('/book/<realBookId>/0')`.

`getTranslateCalls` in real mode reads `harness.llm.requests()` filtered to generateContent paths. Helpers that reach `window.__test` throw in real mode with a named error.

`testIgnore` shrink: run each of the 19 existing spec files under the real config one at a time (`pnpm test:e2e:real tests/e2e/<file>`); a file joins the enabled list when it passes unmodified or with mechanical edits ≤ the helper contract (no test-logic rewrites). Expected enabled: `app.spec.ts`, `text-import.spec.ts`, `epub-import.spec.ts`, core `paragraph-view` cases; expected still-ignored: specs built on `inFlight`/`summaryStatus`/`__mockSpotifyState`. Record the final lists in a comment in the config with one-line reasons.

- [ ] **Step 1: Implement mode switch + real-seed with a first passing target: `text-import.spec.ts` under real config.**
- [ ] **Step 2: Sweep the 19 files; shrink `testIgnore`; commit the passing set.**
- [ ] **Step 3: Run both tiers fully:** `pnpm test:e2e --project=chromium` (mock tier untouched) and `pnpm test:e2e:real` — both green.
- [ ] **Step 4: Commit** — `git commit -m "feat: dual-mode seed helpers; enable existing specs against the real backend"`

---

## Phase 5 — Failure-injection specs

Shared shape for Tasks 14-17: specs live in `site/tests/e2e/real/`, import `{ test, expect }` from `../../real/fixtures`, run only in the real project. Each task: write specs → run `pnpm test:e2e:real -g <name>` → fix what they flush out (these tests exist to find real bugs — a genuine app bug found here is reported to the human, not silently patched) → commit.

### Task 14: Translation/LLM failure specs

**Files:** Create `site/tests/e2e/real/translation-failures.spec.ts`

Specs (each seeds a small book via `seedAndOpen`, then programs the LLM sim):

1. **5xx retry then success:** rule `{matcher: {pathGlob: '*generateContent*'}, action: {type: 'status', code: 503}, times: 2}` + script for the paragraph → click translate (`translateButton` helper) → `expectTranslated` eventually passes; `llm.requests()` shows ≥3 generateContent calls.
2. **Malformed JSON:** `corrupt/malformed_json` always-rule → translate → UI shows the error affordance (reuse the error-state locator from `paragraph-view.spec.ts`'s error cases), app stays responsive (can navigate away and back).
3. **Stalled stream:** `stall` rule → translate → assert the UI remains in in-progress state and the app does not crash within 5s → `llm.reset()` → translate again → succeeds. (Bounded by the backend's own `TRANSLATION_REQUEST_TIMEOUT`/idle timeouts — if the timeout is longer than test patience, assert the in-progress state + post-reset recovery rather than waiting out the timeout.)
4. **Truncated stream:** `truncate 0.5` → translate → error surfaced, no partial garbage rendered as words.
5. **Translations never vanish (regression):** seed a book, translate successfully (scripted), verify; then program `status 500` always → trigger re-translate/save paths → assert existing translations still render after reload (`page.reload()` + `expectTranslated`). Guards the LibraryBook::save drain bug class.

- [ ] Steps: write → run → fix → `git commit -m "test: LLM failure-injection E2E specs"`

### Task 15: Lyrics failure specs

**Files:** Create `site/tests/e2e/real/lyrics-failures.spec.ts`

Driving lyrics without Spotify: read `site/src-tauri/src/app/lyrics.rs` and the `lyrics.spec.ts` mock spec first; the lyrics pipeline is triggered by track state — in real mode drive it via the bridge with the same command the frontend uses (`get_track_lyrics_state` with a track descriptor). Specs:

1. **Found:** seed catalog → invoke → state reaches resolved-with-lyrics; UI lyrics view (route from `lyrics.spec.ts`) renders lines.
2. **404 not-found:** empty catalog → state resolves to no-lyrics; UI shows its empty state; `lrclib.requests()` shows retries did NOT happen (404 is terminal — asserts `Ok(None)` path).
3. **5xx transient:** `status 503, times: 2` + seeded catalog → resolved (LRCLIB_RETRY covers 3 attempts); requests() length ≥3.
4. **Malformed payload:** `corrupt/malformed_json` → error state surfaced, no crash.
5. **Slow response:** `delay 2000` → resolved eventually (within the 10s client timeout).

- [ ] Steps: write → run → fix → `git commit -m "test: lyrics failure-injection E2E specs"`

### Task 16: Anki failure specs

**Files:** Create `site/tests/e2e/real/anki-failures.spec.ts`

Read `library/src/anki/sync.rs` + `anki-sync.spec.ts` first. Trigger via bridge `sync_anki_now`; observe via `get_anki_sync_status` + `anki_sync_status_changed` events (the UI's sync status affordance per `anki-sync.spec.ts`).

1. **Anki not running:** point `ankiEndpoint` at a port with no listener (write a per-test config via `update_config` over the bridge) → `sync_anki_now` → status reflects failure, UI shows it, app healthy.
2. **Mid-batch failure:** seed book with cards to export; rule `{matcher: {bodyContains: '"action":"addNote"', nthCall: <k>}, action: {type: 'status', code: 500}, times: 1}` → sync → assert next sync retries and converges; `anki.requests()` shows no duplicate `addNote` for already-succeeded notes.
3. **Duplicate prevention:** run sync twice with a clean sim → second run performs no `addNote` (requests diff).
4. **Recovery after outage:** `drop` always-rule → sync fails; `anki.reset()` → sync succeeds; state converges (findNotes over sim's `/_sim/`-seeded check or a third sync is a no-op).

- [ ] Steps: write → run → fix → `git commit -m "test: Anki failure-injection E2E specs"`

### Task 17: Cross-cutting resilience specs

**Files:** Create `site/tests/e2e/real/resilience.spec.ts`

1. **No data loss across total outage:** translate a book successfully; program ALL sims to `drop` always; exercise save-bearing flows (re-open chapter, toggle familiarity, trigger re-translate); reset sims; `page.reload()` → every previously-translated paragraph still translated; book still listed.
2. **Concurrent failure independence:** LLM stalled while lyrics resolves fine and Anki syncs fine (three sims programmed differently in one test) — proves no shared-runtime head-of-line blocking.
3. **App restart durability:** (worker restart is expensive — do this one in its own spec file with a test-scoped, not worker-scoped, app process: extend fixtures with `test.use`-able `freshApp` fixture, or spawn a second app on the same configDir after SIGTERM of the first is out of scope — implement via `harness`-provided `restartApp()` added to fixtures in this task) translate → SIGTERM app → relaunch on same configDir → translations present over fresh bridge.

- [ ] Steps: extend fixtures with `restartApp(): Promise<void>` (kills + respawns app, updates bridgePort, re-injects init script on next goto) → write specs → run → fix → `git commit -m "test: cross-cutting resilience E2E specs"`

### Task 18: Full-suite pass + docs

**Files:**
- Create: `site/tests/e2e/real/README.md`
- Modify: root `CLAUDE.md` only if it documents test commands (check first; do not add one if absent)

README covers: architecture sketch (4 processes), how to run (`pnpm test:e2e:real`, `FLTS_E2E_SKIP_BUILD=1` for fast re-runs), how to add a failure rule (SimRule JSON with two examples), the two-tier philosophy (mock = inner loop, real = integration truth), the deferred Linux true-binary tier pointer to the spec.

- [ ] **Step 1:** `cargo test --workspace` + `pnpm test` + `pnpm test:e2e --project=chromium` + `pnpm test:e2e:real` — all green (fix anything that isn't).
- [ ] **Step 2:** Write README.
- [ ] **Step 3:** `git commit -m "docs: real-mode E2E harness README; full-suite green"`

---

## Self-review notes (resolved inline)

- Spec coverage: every spec section maps to a task (seams→1, sims→2-7, bridge→8-9, shim→10, harness/isolation→11, conformance→12, reuse→13, failure specs→14-17, infra-tests→2-7 test steps + 12, docs/deferred→18).
- `SimRule` camelCase requirement folded back into Task 2 (serde `rename_all`).
- Task 8 initially sliced dispatch across two tasks leaving a red drift test — rewritten: full dispatch lands in Task 8; Task 9 is events only.
- Timing sources: sims use `SystemTime` (allowed), no `Date.now` constraints apply (that limit is Workflow-script-specific, not repo code).
