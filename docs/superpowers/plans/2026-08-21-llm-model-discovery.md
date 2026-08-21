# LLM Model Discovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The settings model dropdown lists whatever each LLM provider currently offers (capability-filtered), so a new model appears without an app release.

**Architecture:** A `ModelCatalog` in `library` fetches `GET /models` per provider, caches to disk (24h TTL), and falls back to one built-in default per provider. Model identity is the provider API id string everywhere (config, IPC, translators, Gemini prompt-cache keys). The numbered `TranslationModel` enum is deleted after a one-time migration table. Paragraph translations keep `Version::V2` and append the API id after the existing varint so older Syncthing peers still read the file.

**Tech Stack:** Rust (reqwest, tokio, serde, futures-util), Tauri 2 commands, Svelte 5 settings UI, existing `e2e-sims` axum LLM sim.

**Spec:** `docs/superpowers/specs/2026-08-21-llm-model-discovery-design.md`

## Global Constraints

- Do not bump translation `Version` to `V3` or add a new `FieldTag`. Unknown tags fail deserialize on old peers.
- `Config::load` must accept a numeric `model` without taking the corrupt-config path (that path would hide API keys).
- `get_models` never returns `Err` because a list call failed; log and use cache/fallback.
- Catalog HTTP timeout is 10s, TTL 24h, pagination cap 50 pages. Translation timeouts stay unchanged.
- Request shaping (JSON schema vs JSON object, Gemini prompt cache) stays **per `TranslationProvider`**, not per model.
- Gemini 2.5 Flash `thinking_budget: 0` remains only for ids `models/gemini-2.5-flash` and `gemini-2.5-flash`.
- Comments: terse whys/invariants only.
- Tasks 3–4 will not compile `app` / `cli` until Task 5. Until then run `cargo test -p library` only.
- Package manager for JS is **pnpm**, from `site/`.
- `docs/superpowers/` is gitignored; `git add -f` plan/spec files if committing them. Do not commit the unrelated `app_iOS.xcscheme` change.

## File structure

| File | Responsibility |
|---|---|
| **Create** `library/src/translator/catalog.rs` | Migration table, fallbacks, filters, `ModelListTransport`, `ModelCatalog` (cache, in-flight, prefetch) |
| Modify `library/src/translator.rs` | `pub mod catalog`; delete `TranslationModel`; `get_translator(..., model: &str, ...)` |
| Modify `library/src/translator/gemini.rs` | `Model::Custom(id)`; thinking-budget string match |
| Modify `library/src/translator/openai.rs` | Pass model id through; store `TranslationProvider` instead of enum |
| Modify `library/src/translator/gemini_cache.rs` | `CacheKey.model: String`; filesystem-safe disk key |
| Modify `library/src/lyrics/translation.rs` | Same as gemini/openai translators; delete duplicate `openai_model_name` |
| Modify `library/src/lyrics.rs`, `lyrics/cache.rs` | `LyricsTranslation.model: String`; path uses sanitized API id |
| Modify `library/src/book/translation/mod.rs` | In-memory `model: String`; write varint + len-prefixed string; read prefers string |
| Modify `library/src/book/chapter_summaries.rs` | `model: String`; varint via table only (no format bump) |
| Modify remaining `library` call sites | `add_paragraph_translation`, summary generator, benches, tests: string ids |
| Modify `site/src-tauri/src/app/config.rs` | `model: String` with number\|string serde; async `get_models`; `defaultModel` |
| Modify `site/src-tauri/src/app.rs` | Hold `ModelCatalog`; launch prefetch; key-change invalidate |
| Modify queues, lyrics, bridge, `lib.rs` | `model: String` on translate/lyrics commands |
| Modify `site/src/lib/config/store.ts`, `ConfigView.svelte` | String ids; orphan row; refetch after save |
| Modify `site/tests/mocks/tauri-api.ts`, e2e `MODEL` constants | String ids |
| Modify `e2e-sims/src/llm.rs` | `GET /v1beta/models`, `GET /v1/models`, `GET /models` |
| Modify `cli/src/main.rs` | Optional `--model`, default Google fallback |

---

### Task 1: Migration table, fallbacks, and list filters

**Files:**
- Create: `library/src/translator/catalog.rs`
- Modify: `library/src/translator.rs` — add `pub mod catalog;` next to `pub mod gemini_cache;`

**Interfaces:**
- Consumes: `TranslationProvider` in `translator.rs`.
- Produces:

```rust
pub const FALLBACK_GOOGLE: &str = "models/gemini-3.7-flash";
pub const FALLBACK_OPENAI: &str = "gpt-5-mini";
pub const FALLBACK_DEEPSEEK: &str = "deepseek-v4-flash";
pub const FALLBACK_ZAI: &str = "glm-5.2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedModel {
    pub id: String,
    pub name: String,
    pub provider: TranslationProvider,
}

pub fn fallback_for(provider: TranslationProvider) -> ListedModel;
pub fn all_fallbacks() -> Vec<ListedModel>; // four rows, one per provider
pub fn api_id_from_legacy(id: u64) -> String; // 0 or unknown → ""
pub fn legacy_id_from_api(id: &str) -> u64;   // unknown → 0
pub fn effective_model_id(provider: TranslationProvider, config_model: &str) -> String;
// empty/whitespace → fallback_for(provider).id

pub fn filter_gemini_models(body: &serde_json::Value) -> Vec<ListedModel>;
pub fn filter_openai_compat_models(
    body: &serde_json::Value,
    provider: TranslationProvider,
) -> Vec<ListedModel>;
```

`filter_gemini_models` always tags `provider: Google`. Keep a model if `supportedGenerationMethods` contains `"generateContent"` **and** `name` does not contain any of: `embedding`, `imagen`, `veo`, `-image`, `aqa`, `tts`. `id` = `name`. Display `name` = `displayName` if a non-empty string, else `id`.

`filter_openai_compat_models` uses `data[].id`. Drop if the id **equals or starts with** any of: `text-embedding-`, `embedding-`, `whisper-`, `tts-`, `dall-e-`, `gpt-image`, `chatgpt-image`, `omni-moderation`, `moderation-`, `sora-`, `computer-use`, `babbage`, `davinci`, `curie`, `ada`, `gpt-4o-transcribe`, `gpt-4o-mini-transcribe`, `gpt-4o-mini-tts`, `gpt-realtime`. Display name = id. Sort each filter’s output by display name, case-insensitive.

Legacy table (exact strings):

| u64 | API id |
|---|---|
| 1 | `models/gemini-2.5-flash` |
| 2 | `models/gemini-2.5-pro` |
| 3 | `models/gemini-2.5-flash-lite` |
| 4 | `gpt-5-mini` |
| 5 | `gpt-5.2` |
| 6 | `gpt-5.2-pro` |
| 7 | `gpt-5-nano` |
| 8 | `models/gemini-3-pro-preview` |
| 9 | `models/gemini-3-flash-preview` |
| 10 | `gpt-5.4` |
| 11 | `gpt-5.4-mini` |
| 12 | `models/gemini-3.1-pro-preview` |
| 13 | `models/gemini-3.1-flash-lite-preview` |
| 14 | `models/gemini-3.5-flash` |
| 15 | `deepseek-v4-flash` |
| 16 | `deepseek-v4-pro` |
| 17 | `glm-5.2` |
| 18 | `models/gemini-3.6-flash` |
| 19 | `models/gemini-3.7-flash` |

- [ ] **Step 1: Write failing tests** in `catalog.rs` (`#[cfg(test)] mod tests`).

```rust
#[test]
fn legacy_table_round_trips_known_ids() {
    assert_eq!(api_id_from_legacy(1), "models/gemini-2.5-flash");
    assert_eq!(api_id_from_legacy(19), "models/gemini-3.7-flash");
    assert_eq!(api_id_from_legacy(0), "");
    assert_eq!(api_id_from_legacy(99), "");
    assert_eq!(legacy_id_from_api("models/gemini-2.5-flash"), 1);
    assert_eq!(legacy_id_from_api("gpt-9-ultra"), 0);
}

#[test]
fn effective_id_uses_fallback_when_empty() {
    assert_eq!(
        effective_model_id(TranslationProvider::Google, ""),
        FALLBACK_GOOGLE
    );
    assert_eq!(
        effective_model_id(TranslationProvider::Openai, "gpt-5.2"),
        "gpt-5.2"
    );
}

#[test]
fn gemini_filter_keeps_generate_content_drops_the_rest() {
    let body = serde_json::json!({
        "models": [
            {
                "name": "models/gemini-3.7-flash",
                "displayName": "Gemini 3.7 Flash",
                "supportedGenerationMethods": ["generateContent", "countTokens"]
            },
            {
                "name": "models/text-embedding-004",
                "displayName": "Embeddings",
                "supportedGenerationMethods": ["embedContent"]
            },
            {
                "name": "models/gemini-2.5-flash-image",
                "displayName": "Flash Image",
                "supportedGenerationMethods": ["generateContent"]
            },
            {
                "name": "models/gemini-9-ultra",
                "supportedGenerationMethods": ["generateContent"]
            }
        ]
    });
    let got = filter_gemini_models(&body);
    let ids: Vec<_> = got.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["models/gemini-3.7-flash", "models/gemini-9-ultra"]);
    assert_eq!(got[0].name, "Gemini 3.7 Flash");
    assert_eq!(got[1].name, "models/gemini-9-ultra");
}

#[test]
fn openai_filter_drops_non_chat_keeps_unknown_chat() {
    let body = serde_json::json!({
        "data": [
            {"id": "gpt-9-ultra"},
            {"id": "text-embedding-3-large"},
            {"id": "whisper-1"},
            {"id": "dall-e-3"},
            {"id": "gpt-5-mini"}
        ]
    });
    let got = filter_openai_compat_models(&body, TranslationProvider::Openai);
    let ids: Vec<_> = got.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["gpt-5-mini", "gpt-9-ultra"]); // sorted by display name
}
```

- [ ] **Step 2: Run tests — expect compile fail** (`catalog` module missing)

Run: `cargo test -p library catalog:: -- --nocapture`

Expected: error `could not find catalog in translator` or file not found.

- [ ] **Step 3: Implement `catalog.rs`** with the table, fallbacks, `effective_model_id`, and the two filters. Wire `pub mod catalog;` in `translator.rs`. Do not add HTTP yet.

- [ ] **Step 4: Re-run tests — expect PASS**

Run: `cargo test -p library catalog:: -- --nocapture`

- [ ] **Step 5: Commit**

```bash
git add library/src/translator/catalog.rs library/src/translator.rs
git commit -m "feat: add LLM model catalog filters and legacy id table"
```

---

### Task 2: `ModelCatalog` — cache, TTL, transport, in-flight, pagination

**Files:**
- Modify: `library/src/translator/catalog.rs`

**Interfaces:**
- Consumes: Task 1 filters/fallbacks.
- Produces:

```rust
pub const LIST_TTL_SECS: u64 = 24 * 3600;
pub const LIST_TIMEOUT: Duration = Duration::from_secs(10);
pub const LIST_MAX_PAGES: usize = 50;

#[async_trait::async_trait]
pub trait ModelListTransport: Send + Sync {
    async fn get_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> anyhow::Result<serde_json::Value>;
}

pub struct ModelCatalog { /* cache_dir, transport, now_secs, inflight */ }

impl ModelCatalog {
    pub fn new(cache_dir: PathBuf, transport: Arc<dyn ModelListTransport>) -> Self;
    /// `now_secs` is unix seconds; tests inject a `AtomicU64`.
    pub fn new_with_clock(
        cache_dir: PathBuf,
        transport: Arc<dyn ModelListTransport>,
        now_secs: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self;

    pub async fn models_for(
        &self,
        provider: TranslationProvider,
        api_key: Option<&str>,
        list_base_url: &str, // already-resolved chat/list origin, no trailing-path models
    ) -> Vec<ListedModel>;

    pub fn invalidate(&self, provider: TranslationProvider);
}

pub fn join_models_url(base: &str) -> String; // `{base}/models` with slash folding
```

Resolve rules for `models_for` (must match the spec):

1. No API key (None or `""`) → `vec![fallback_for(provider)]`, no transport call.
2. Disk cache `{cache_dir}/model_catalog/{provider}.json` younger than 24h → return it, no transport. Provider filename: `google` / `openai` / `deepseek` / `zai`.
3. Otherwise one in-flight fetch per provider: concurrent callers share it.
4. Fetch: `GET {join_models_url(list_base_url)}`. Gemini: also `?key=` plus `pageToken` when paginating; OpenAI-compat: `Authorization: Bearer {key}` and `after` = last id when `has_more` is true. Follow `nextPageToken` (Gemini) or `has_more` (OpenAI-compat) up to `LIST_MAX_PAGES`.
5. Filter pages, concat, sort, **ensure fallback id is present** (append fallback row if missing). Write `{ fetchedAt, models: [{id, name}] }` (provider is implied by the filename; re-attach on read).
6. On any transport/parse error: log (`log::warn!`), return stale cache if any, else fallback. **Do not delete the cache file on 401.** Skip bad rows inside a page; do not fail the page if sibling rows are valid.
7. `invalidate` deletes the cache file and drops in-flight.

Cache JSON:

```json
{"fetchedAt": 1710000000, "models": [{"id": "gpt-5-mini", "name": "gpt-5-mini"}]}
```

Gemini vs OpenAI-compat: `provider == Google` uses Gemini query-key + `models` array + `nextPageToken`; the other three use Bearer + `data` array + `has_more`.

Do **not** implement the real reqwest transport in this task — tests inject a fake. Real transport is Task 5.

- [ ] **Step 1: Write failing tests** (same `catalog.rs` test module). Use a fake transport:

```rust
struct FakeTransport {
    hits: AtomicUsize,
    handler: Box<dyn Fn(&str) -> anyhow::Result<Value> + Send + Sync>,
}
#[async_trait::async_trait]
impl ModelListTransport for FakeTransport {
    async fn get_json(&self, url: &str, _headers: &[(&str, &str)]) -> anyhow::Result<Value> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        (self.handler)(url)
    }
}
```

Tests to include (names locked):

- `no_key_returns_fallback_without_http` — hits == 0, one fallback row.
- `fresh_cache_skips_http` — write a cache file with `fetchedAt = now`, hits == 0, returned ids match the file.
- `stale_cache_refetches` — `fetchedAt = now - LIST_TTL_SECS - 1`, handler returns one new model, hits == 1, disk updated.
- `http_failure_uses_stale_cache` — stale file present, handler returns `Err`, result is stale ids, file still on disk.
- `http_401_does_not_delete_cache` — same as failure, file exists after.
- `no_cache_http_failure_uses_fallback` — hits >= 1, result is fallback.
- `pagination_concatenates_two_pages_and_stops_at_cap` — Gemini handler: if url lacks `pageToken`, return `{models:[page1], nextPageToken:"t2"}`; if it has `pageToken=t2`, return `{models:[page2]}` with no token. Result contains both. Second test: always return a `nextPageToken`; assert hits == `LIST_MAX_PAGES`. Third: OpenAI-compat `{data:[a], has_more:true}` then `{data:[b], has_more:false}` using `after=` on the second URL.
- `inflight_dedupes_concurrent_calls` — handler sleeps 50ms (`std::thread::sleep` is fine inside the fake if the test is `#[tokio::test(flavor = "current_thread")]` and you use `tokio::time::sleep` in the fake instead). Two `tokio::join!` of `models_for`. hits == 1.
- `live_list_missing_fallback_still_includes_it` — live returns only `gpt-9-ultra`; result ids include `FALLBACK_OPENAI` and `gpt-9-ultra`.

- [ ] **Step 2: Run tests — expect FAIL** (functions missing)

Run: `cargo test -p library catalog:: -- --nocapture`

- [ ] **Step 3: Implement `ModelCatalog`**. Keep `now_secs` injectable. Use `tokio::sync::Mutex<HashMap<TranslationProvider, Arc<tokio::sync::OnceCell<Vec<ListedModel>>>>>` for in-flight: insert the cell under the mutex, `get_or_init` the fetch, then remove the cell. Fetch must not hold the map mutex.

- [ ] **Step 4: Re-run — expect PASS**

Run: `cargo test -p library catalog:: -- --nocapture`

- [ ] **Step 5: Commit**

```bash
git add library/src/translator/catalog.rs
git commit -m "feat: cache and fetch LLM model lists with in-flight dedupe"
```

---

### Task 3: String model identity in `library` (delete `TranslationModel`)

**Files:**
- Modify: `library/src/translator.rs` — delete the enum, `From<usize>`, `Display`, `provider()`, `EnumIter`. `Translator::get_model(&self) -> String`. `get_translator(..., model: &str, ...)`.
- Modify: `library/src/translator/gemini.rs` — `GeminiTranslator::create(..., model: &str, ...)`. Always `Model::Custom(model.to_string())`. Thinking:

```rust
fn is_gemini_25_flash(id: &str) -> bool {
    id == "models/gemini-2.5-flash" || id == "gemini-2.5-flash"
}
```

`thinking_budget: Some(0)` only when `is_gemini_25_flash`; else current default.
- Modify: `library/src/translator/openai.rs` — drop `openai_model_name`. Store `provider: TranslationProvider` and `model: Arc<str>`. `is_deepseek` from `provider`.
- Modify: `library/src/translator/gemini_cache.rs` — `CacheKey { model: String, ... }`. `disk_key` / `cache_display_name` must not use `usize::from`. Sanitize the id the same way lyrics will: non-ascii-alphanumeric → `_`.
- Modify: `library/src/lyrics/translation.rs` — delete the duplicate `openai_model_name`; `get_lyrics_translator(..., model: &str, ...)`; Gemini thinking via `is_gemini_25_flash` (duplicate the two-line helper in this file rather than making gemini.rs public — lyrics already duplicates thinking config).
- Modify: `library/src/lyrics.rs` — `LyricsTranslation.model: String`.
- Modify: `library/src/lyrics/cache.rs` — `path_for(..., model: &str)`. Filename `{safe_track}__{lang}_{safe_model}.json` where `safe_model` uses existing `sanitize()` (turns `/` into `_`).
- Modify: `library/src/book/translation/mod.rs` — `model: String` on the struct/view/`add_paragraph_translation`. **This task writes and reads only the varint** (via `legacy_id_from_api` / `api_id_from_legacy`). Trailing string is Task 4. Empty in-memory id ↔ varint `0`.
- Modify: `library/src/book/chapter_summaries.rs` — `ChapterSummary.model: String`; serialize/deserialize via the table.
- Modify every remaining `library` `TranslationModel::…` site to the API id string. Mapping:

| Enum variant | String |
|---|---|
| `Unknown` | `""` |
| `Gemini25Flash` | `"models/gemini-2.5-flash"` |
| `Gemini25Pro` | `"models/gemini-2.5-pro"` |
| `Gemini25FlashLight` | `"models/gemini-2.5-flash-lite"` |
| `Gemini3Pro` | `"models/gemini-3-pro-preview"` |
| `Gemini3Flash` | `"models/gemini-3-flash-preview"` |
| `Gemini31Pro` | `"models/gemini-3.1-pro-preview"` |
| `Gemini31FlashLite` | `"models/gemini-3.1-flash-lite-preview"` |
| `Gemini35Flash` | `"models/gemini-3.5-flash"` |
| `Gemini36Flash` | `"models/gemini-3.6-flash"` |
| `Gemini37Flash` | `"models/gemini-3.7-flash"` |
| `OpenAIGpt5Mini` | `"gpt-5-mini"` |
| `OpenAIGpt52` | `"gpt-5.2"` |
| `OpenAIGpt52Pro` | `"gpt-5.2-pro"` |
| `OpenAIGpt5Nano` | `"gpt-5-nano"` |
| `OpenAIGpt54` | `"gpt-5.4"` |
| `OpenAIGpt54Mini` | `"gpt-5.4-mini"` |
| `DeepSeekV4Flash` | `"deepseek-v4-flash"` |
| `DeepSeekV4Pro` | `"deepseek-v4-pro"` |
| `ZaiGlm52` | `"glm-5.2"` |

Known remaining sites (re-rg `TranslationModel` after edits): `library/src/library.rs`, `library/src/library/library_book/**`, `library/src/summary_generator.rs`, `library/src/book/translation/tests.rs`, `library/src/book/chapter_summaries.rs` tests, `library/src/translator/gemini_cache.rs` tests, `library/benches/translation_bench.rs`, `library/tests/*.rs`.

**Interfaces:**
- Consumes: `api_id_from_legacy` / `legacy_id_from_api` / `effective_model_id` from Task 1.
- Produces: `get_translator(..., model: &str, ...) -> anyhow::Result<Box<dyn Translator>>`; `Translator::get_model() -> String`; `add_paragraph_translation(..., model: String)` (or `&str` — pick `&str` and store `model.to_string()`).

Empty `config` id is **not** handled inside `get_translator`; callers pass `effective_model_id`. Translators do not map names and do not return `UnknownModel` for an unrecognized id.

- [ ] **Step 1: Change one library test to strings and watch compile fail**

In `library/src/book/translation/tests.rs` replace `TranslationModel::Gemini25Pro` with `"models/gemini-2.5-pro"` in `test_add_paragraph_translation` (the test that asserts `paragraph_view.model`).

Run: `cargo test -p library --lib book::translation::tests::test_add_paragraph_translation`

Expected: type mismatch (`TranslationModel` vs `&str`).

- [ ] **Step 2: Switch in-memory + varint disk + translators + lyrics cache path.** Delete the enum. Fix every `library` compile error. `gemini_model()` and both `openai_model_name` functions go away.

Varint write (translation tagged field, still `FieldTag::TranslationModel = 1`):

```rust
write_var_u64(&mut cursor, FieldTag::TranslationModel as u64)?;
write_var_u64(&mut cursor, crate::translator::catalog::legacy_id_from_api(&pt.model))?;
```

Varint read:

```rust
let n = read_var_u64(&mut cursor)?;
translation.model = crate::translator::catalog::api_id_from_legacy(n);
```

Do not write the len-prefixed string yet.

- [ ] **Step 3: Run library tests**

Run: `cargo test -p library`

Expected: PASS. (`app` / `cli` may fail to compile — do not fix them here.)

- [ ] **Step 4: Commit**

```bash
git add library
git commit -m "refactor: identify translation models by API id string"
```

---

### Task 4: Mixed-version translation blob (varint + trailing API id)

**Files:**
- Modify: `library/src/book/translation/mod.rs` serialize/deserialize of `FieldTag::TranslationModel`
- Test: `library/src/book/translation/tests.rs`

**Interfaces:**
- Consumes: `write_len_prefixed_str` / `read_len_prefixed_string` in `book/serialization.rs`; `legacy_id_from_api` / `api_id_from_legacy`.
- Produces: field blob layout:

```
varint tag=1
varint legacy_id     // reverse-map, or 0
len-prefixed string  // API id, UTF-8
```

Stay on `Version::V2`. No new tag. Reverse-map is exact match on the table (Gemini ids stay `models/…`; do not strip the prefix).

Read: after the legacy varint, if `cursor.position() < buf.len()`, read the len-prefixed string and use it (even if empty). Else map the varint.

- [ ] **Step 1: Write failing tests**

Extract tagged-field helpers `write_model_field(model: &str) -> Vec<u8>` and `read_model_field(buf: &[u8]) -> io::Result<String>` (tag byte included in the blob, same as today’s field). Tests:

```rust
#[test]
fn translation_model_field_round_trips_discovered_id() {
    let mut t = Translation::create("en", "ru");
    t.add_paragraph_translation(0, &make_paragraph(1, "hi"), "models/gemini-9-ultra");
    let mut buf = Vec::new();
    t.serialize(&mut buf).unwrap();
    let back = Translation::deserialize(&mut std::io::Cursor::new(&buf)).unwrap();
    assert_eq!(back.paragraph_view(0).unwrap().model, "models/gemini-9-ultra");
}

#[test]
fn read_model_field_accepts_varint_only_legacy_blob() {
    let mut blob = Vec::new();
    crate::book::serialization::write_var_u64(&mut blob, 1).unwrap(); // tag
    crate::book::serialization::write_var_u64(&mut blob, 2).unwrap(); // Gemini 2.5 Pro
    assert_eq!(read_model_field(&blob).unwrap(), "models/gemini-2.5-pro");
}
```

- [ ] **Step 2: Run — expect FAIL** (discovered id round-trips as `""` because varint 0)

Run: `cargo test -p library --lib book::translation::tests::translation_model_field_round_trips_discovered_id`

- [ ] **Step 3: Write trailing string in `write_model_field`; prefer it in `read_model_field`.**

- [ ] **Step 4: Run translation tests — expect PASS**

Run: `cargo test -p library --lib book::translation`

- [ ] **Step 5: Commit**

```bash
git add library/src/book/translation/mod.rs library/src/book/translation/tests.rs
git commit -m "feat: persist translation model API ids without a format bump"
```

---

### Task 5: Tauri config, `get_models`, launch prefetch, key invalidation

**Files:**
- Modify: `site/src-tauri/src/app/config.rs`
- Modify: `site/src-tauri/src/app.rs` (`AppState`, `apply_config`, `new`)
- Create (or add to catalog.rs): `ReqwestListTransport` in `library/src/translator/catalog.rs` using `reqwest` + `LIST_TIMEOUT`
- Modify: `site/src-tauri/src/app/translation_queue.rs`, `summary_generation_queue.rs`, `lyrics.rs`, `library_view/mod.rs`, `bridge.rs`, `lib.rs` — `model: String`
- Modify: `site/src-tauri/src/app.rs` `translate_paragraph` / `translate_chapter` signatures

**Interfaces:**
- Consumes: `ModelCatalog`, `all_fallbacks`, `effective_model_id`, `FALLBACK_*`.
- Produces:

```rust
// config.rs
pub struct Model {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<TranslationProvider>,
}
pub struct ProviderMeta {
    pub id: TranslationProvider,
    pub name: &'static str,
    #[serde(rename = "defaultModel")]
    pub default_model: String, // FALLBACK_* for that provider
    #[serde(rename = "apiKeyField")]
    pub api_key_field: &'static str,
}

#[tauri::command]
pub async fn get_models(state: tauri::State<'_, Arc<AppState>>) -> Result<Vec<Model>, String>;

#[tauri::command]
pub fn get_translation_providers() -> Vec<ProviderMeta>; // defaultModel strings

impl Config {
    pub model: String, // serde via deserialize_model
}
```

`deserialize_model`: JSON string → as-is; JSON number `n` → `api_id_from_legacy(n as u64)` (0/unknown → `""`). Serialize always as a string.

`Config::default().model` = `FALLBACK_GOOGLE.to_string()`.

`get_models`: `tokio::join!` of `models_for` for all four providers (keyed or not — no-key is instant fallback), then concat in provider order Google, OpenAI, DeepSeek, z.AI. Map `ListedModel` → `Model`. Never `Err` on list failure.

`list_base_url(provider)` (pure, env injected in tests like existing openai/gemini resolvers):

- Google: `FLTS_GEMINI_BASE_URL` if non-empty (already includes `/v1beta/` + trailing slash per prior work) else `https://generativelanguage.googleapis.com/v1beta/`. Catalog `join_models_url` yields `…/v1beta/models`.
- OpenAI: `OPENAI_BASE_URL` or `https://api.openai.com/v1`.
- DeepSeek / z.AI: existing `openai_compat_base_url` (make it `pub` if catalog/app need it, or a thin wrapper in catalog that takes the resolved string).

`ReqwestListTransport`: `reqwest::Client::builder().timeout(LIST_TIMEOUT).build()`. `get_json` applies headers and `send().error_for_status()?.json()`.

`AppState` gains `model_catalog: Arc<ModelCatalog>`. In `new`, `ModelCatalog::new(resolve_cache_dir(...)? , Arc::new(ReqwestListTransport::new()))`. After config is loaded, `tauri::async_runtime::spawn` prefetch: for each provider with a non-empty key, `models_for(...).await`. Errors are already swallowed inside `models_for`. Do not await in `new`.

`apply_config`: if a provider’s key **string** changed (including empty↔set), `model_catalog.invalidate(provider)` **before** saving. Compare against `self.config.borrow()` old keys.

Translate paths: `get_translator(..., &effective_model_id(config.translation_provider, &model), ...)`. If the command’s `model` argument is non-empty, use it; else `config.model`; then `effective_model_id`.

Bridge: `"get_models" => wrap(crate::app::config::get_models(state).await)` — it is now async and needs `State`. Translate/lyrics args: `model: String`. Drop `use TranslationModel` from `bridge.rs`.

- [ ] **Step 1: Write failing config serde tests** in `site/src-tauri/src/app/config.rs` `mod tests`:

```rust
#[test]
fn config_model_number_migrates_without_corrupt_path() {
    let legacy = serde_json::json!({
        "targetLanguageId": "eng",
        "translationProvider": "google",
        "model": 1
    });
    let parsed: Config = serde_json::from_value(legacy).unwrap();
    assert_eq!(parsed.model, "models/gemini-2.5-flash");
}

#[test]
fn config_model_string_passthrough() {
    let v = serde_json::json!({
        "targetLanguageId": "eng",
        "translationProvider": "openai",
        "model": "gpt-9-ultra"
    });
    let parsed: Config = serde_json::from_value(v).unwrap();
    assert_eq!(parsed.model, "gpt-9-ultra");
    let dumped = serde_json::to_value(&parsed).unwrap();
    assert_eq!(dumped["model"], "gpt-9-ultra");
}

#[test]
fn config_model_zero_and_unknown_become_empty() {
    for n in [0, 99] {
        let v = serde_json::json!({
            "targetLanguageId": "eng",
            "translationProvider": "google",
            "model": n
        });
        let parsed: Config = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.model, "");
    }
}
```

Existing tests that use `"model": 0` must still parse (they already will once the visitor exists). `Config::default()` assertions that compare `model` need updating if any exist.

- [ ] **Step 2: Run — expect FAIL** (`model` still `TranslationModel`, type errors)

Run: `cargo test -p app config::tests -- --nocapture`

- [ ] **Step 3: Implement serde visitor, `ReqwestListTransport`, `AppState` catalog + prefetch, async `get_models`, string `model` on all app/bridge command signatures. Fix compile across `app`.**

`get_models` in `lib.rs` `generate_handler!` stays listed; the command is now `async`.

- [ ] **Step 4: Run app + library tests**

Run: `cargo test -p library && cargo test -p app`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add library/src/translator/catalog.rs site/src-tauri
git commit -m "feat: discover LLM models at runtime and persist string ids in config"
```

---

### Task 6: Frontend settings — string ids, orphan row, refetch

**Files:**
- Modify: `site/src/lib/config/store.ts`
- Modify: `site/src/lib/config/store.spec.ts`
- Modify: `site/src/lib/config/ConfigView.svelte`
- Modify: `site/src/lib/data/library.ts` — `translateParagraph` / `translateChapter` `model?: string`
- Modify: `site/src/lib/lyrics/LyricsView.svelte` if it passes `cfg.model`
- Modify: `site/tests/mocks/tauri-api.ts`

**Interfaces:**
- Consumes: backend `Model.id: string`, `ProviderMeta.defaultModel: string`.
- Produces:

```ts
export type Model = {
    id: string,
    name: string,
    provider?: TranslationProvider,
}

export type ProviderMeta = {
    id: TranslationProvider,
    name: string,
    defaultModel: string,
    apiKeyField: 'geminiApiKey' | 'openaiApiKey' | 'deepseekApiKey' | 'zaiApiKey',
}

export type Config = { model: string, /* rest unchanged */ }

export function modelsForDropdown(
    models: Model[],
    provider: TranslationProvider,
    selectedId: string,
): { list: Model[]; orphan: boolean }
```

`modelsForDropdown`: filter `m.provider === provider`. If `selectedId` is non-empty and not in that list, prepend `{ id: selectedId, name: selectedId, provider }` and `orphan: true`. Empty `selectedId` is not an orphan.

- [ ] **Step 1: Write failing vitest** in `store.spec.ts`:

```ts
import { modelsForDropdown } from './store';

it('keeps a saved id missing from the catalog', () => {
    const models = [
        { id: 'models/gemini-3.7-flash', name: 'Gemini 3.7 Flash', provider: 'google' as const },
    ];
    const { list, orphan } = modelsForDropdown(models, 'google', 'models/gemini-2.5-flash');
    expect(orphan).toBe(true);
    expect(list[0].id).toBe('models/gemini-2.5-flash');
    expect(list.map(m => m.id)).toContain('models/gemini-3.7-flash');
});

it('does not treat empty selection as orphan', () => {
    const { list, orphan } = modelsForDropdown([], 'google', '');
    expect(orphan).toBe(false);
    expect(list).toEqual([]);
});
```

Also change existing `Config` type fixtures from `model: 0` to `model: ''`.

- [ ] **Step 2: Run — expect FAIL** (`modelsForDropdown` not exported)

Run: `cd site && pnpm exec vitest run src/lib/config/store.spec.ts`

- [ ] **Step 3: Implement `modelsForDropdown`. Update `ConfigView.svelte`:**

```ts
let model: string = $state('');
const filtered = $derived(modelsForDropdown(models, translationProvider, model));
const filteredModels = $derived(filtered.list);
const modelOrphan = $derived(filtered.orphan);
```

Replace the `$effect` that auto-picks a default:

```ts
$effect(() => {
    const providerMeta = providers.find((p) => p.id === translationProvider);
    if (!providerMeta) return;
    const selected = models.find((m) => m.id === model);
    const providerMismatch = !!selected && selected.provider !== translationProvider;
    if (!model || providerMismatch) {
        model = providerMeta.defaultModel;
    }
});
```

Do **not** reset when the id is simply missing from `models`.

Template: `{#if modelOrphan}<div class="spotify-notice">Not in the current catalog</div>{/if}` under the model `<select>`. Copy is exactly `Not in the current catalog`.

After successful `setConfig` in `save()`, `models = await getModels();`.

Mocks in `tauri-api.ts`:

```ts
const mockModels: Model[] = [
  { id: 'models/gemini-2.5-flash', name: 'Gemini 2.5 Flash', provider: 'google' },
  { id: 'models/gemini-2.5-pro', name: 'Gemini 2.5 Pro', provider: 'google' },
  { id: 'gpt-5-mini', name: 'OpenAI GPT-5 mini', provider: 'openai' },
];
const mockProviders: ProviderMeta[] = [
  { id: 'google', name: 'Google', defaultModel: 'models/gemini-2.5-flash', apiKeyField: 'geminiApiKey' },
  { id: 'openai', name: 'OpenAI', defaultModel: 'gpt-5-mini', apiKeyField: 'openaiApiKey' },
];
```

`mockConfig.model` becomes `''` or `'models/gemini-2.5-flash'`. Update the `Config` type in that mock file (`model: number` → `string`). Lyrics mock helper `translationKey(..., model: number)` → `string`.

- [ ] **Step 4: Run vitest + `pnpm check`**

Run: `cd site && pnpm exec vitest run src/lib/config/store.spec.ts && pnpm check`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add site/src/lib/config site/src/lib/data/library.ts site/src/lib/lyrics site/tests/mocks/tauri-api.ts
git commit -m "feat: show discovered LLM models and keep orphan selections"
```

---

### Task 7: e2e-sims list endpoints and e2e string `MODEL`

**Files:**
- Modify: `e2e-sims/src/llm.rs` — add GET list routes **before** the POST `/{*model_action}` routes so they do not steal listing
- Test: `e2e-sims/tests/llm_sim.rs`
- Modify: `site/tests/real/spec-helpers.ts` `MODEL`
- Modify: `site/tests/e2e/real/*.ts` local `MODEL = 1` constants
- Modify: `site/tests/real/fixtures.ts` comment that referenced usize

**Interfaces:**
- Consumes: none.
- Produces: list fixtures:

Gemini `GET /v1beta/models` (and `/models` if you also serve unprefixed):

```json
{
  "models": [
    {
      "name": "models/gemini-2.5-flash",
      "displayName": "Gemini 2.5 Flash",
      "supportedGenerationMethods": ["generateContent", "countTokens"]
    },
    {
      "name": "models/text-embedding-004",
      "displayName": "Embeddings",
      "supportedGenerationMethods": ["embedContent"]
    }
  ]
}
```

OpenAI-compat `GET /v1/models` and `GET /models`:

```json
{
  "object": "list",
  "data": [
    { "id": "gpt-5.2", "object": "model" },
    { "id": "text-embedding-3-large", "object": "model" },
    { "id": "glm-5.2", "object": "model" },
    { "id": "deepseek-v4-flash", "object": "model" }
  ]
}
```

Register with `get(...)` on those exact paths. Existing POST generate routes stay.

Frontend `MODEL` constant: `'models/gemini-2.5-flash'` (the sim already serves that generate path).

- [ ] **Step 1: Write failing sim test** in `e2e-sims/tests/llm_sim.rs`:

```rust
#[tokio::test]
async fn lists_gemini_and_openai_models() {
    // start llm_router the same way other tests in this file do
    let resp = client.get(format!("{base}/v1beta/models")).send().await.unwrap();
    assert!(resp.status().is_success());
    let v: serde_json::Value = resp.json().await.unwrap();
    assert!(v["models"].as_array().unwrap().iter().any(|m| m["name"] == "models/gemini-2.5-flash"));

    let resp = client.get(format!("{base}/v1/models")).send().await.unwrap();
    let v: serde_json::Value = resp.json().await.unwrap();
    assert!(v["data"].as_array().unwrap().iter().any(|m| m["id"] == "gpt-5.2"));
}
```

Match the test’s existing client/base helper; do not invent a new server harness.

- [ ] **Step 2: Run — expect FAIL** (404)

Run: `cargo test -p e2e-sims lists_gemini_and_openai_models -- --nocapture`

- [ ] **Step 3: Add the GET routes and switch TS `MODEL` constants.**

- [ ] **Step 4: Run sim tests**

Run: `cargo test -p e2e-sims`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add e2e-sims site/tests
git commit -m "test: serve LLM model lists from e2e sims and use string model ids"
```

---

### Task 8: CLI `--model` and workspace compile

**Files:**
- Modify: `cli/src/main.rs`

**Interfaces:**
- Consumes: `get_translator(..., model: &str)`, `FALLBACK_GOOGLE`, `effective_model_id`.
- Produces: `Translate` subcommand:

```rust
/// Translation model API id (default: Gemini 3.7 Flash)
#[arg(long, value_name = "MODEL")]
model: Option<String>,
```

Default when omitted: `FALLBACK_GOOGLE`. Pass `effective_model_id(TranslationProvider::Google, &model)` into `get_translator`. Do not list models from the CLI.

- [ ] **Step 1: `cargo build -p cli` — expect FAIL** (`TranslationModel` missing)

Run: `cargo build -p cli`

- [ ] **Step 2: Add `--model`, delete `TranslationModel` import, use the fallback string.**

- [ ] **Step 3: Workspace compile + library/app/cli/e2e-sims tests**

Run:

```bash
cargo test -p library
cargo test -p app
cargo test -p cli
cargo test -p e2e-sims
cd site && pnpm exec vitest run src/lib/config/store.spec.ts && pnpm check
```

Expected: all PASS.

- [ ] **Step 4: `rg TranslationModel` from repo root** — only this plan, the spec, and maybe comments should remain. No Rust/TS references to the enum or numeric `MODEL = 1`.

- [ ] **Step 5: Commit**

```bash
git add cli/src/main.rs
git commit -m "feat: accept --model API id on the CLI"
```

---

## Self-review (spec coverage)

| Spec requirement | Task |
|---|---|
| Live listing, capability filter (not allowlist) | 1, 2 |
| Cache 24h + four fallbacks | 1, 2 |
| Keep saved id if missing from catalog; warn in settings | 6 |
| String identity; enum deleted except migration table | 1, 3 |
| Request shaping per provider | 3 (openai `provider` field) |
| Gemini 2.5 Flash thinking_budget special case | 3 |
| Async launch prefetch; `get_models` joins in-flight | 2 (in-flight), 5 (spawn) |
| `get_models` never fails the command | 2, 5 |
| 401 does not delete cache | 2 |
| Pagination cap 50 | 2 |
| Config number\|string serde; no corrupt-config path | 5 |
| Empty model → fallback at use time | 1 `effective_model_id`, 5 callers |
| Translation V2 varint + trailing string | 4 |
| Chapter summaries varint only | 3 |
| Lyrics cache filesystem-safe id | 3 |
| Key change invalidates that provider’s cache; settings refetch | 5, 6 |
| e2e-sims GET list routes; string `MODEL` | 7 |
| CLI `--model`, no listing | 8 |
| No TLA change | (none) |
| No hosted catalog / typed-in id / `models_updated` event | (none) |
