# LLM Model Discovery — Design

**Date:** 2026-08-21
**Status:** Approved design, pre-implementation

## Goal

The settings model dropdown tracks whatever each LLM provider currently
lists, so a new Flash/GPT/GLM variant appears without an app release.
Hardcoded model names remain only as a four-entry bootstrap and as a
one-time config/on-disk migration table.

## Key decisions (agreed during brainstorming)

- **Live listing, capability filter only** — show chat / `generateContent`
  models as soon as the provider lists them. Untested ids are allowed;
  structured-output 400s fail at request time like any other provider error.
- **Cache + tiny fallback** — disk cache of the last successful list per
  provider; one built-in default per provider when there is no cache and no
  key.
- **Keep a saved id even if it leaves the catalog** — settings may warn;
  never silently rewrite `config.model`.
- **String identity** — persist and pass the provider API id, not a numbered
  enum. `TranslationProvider` stays the closed enum; request shaping stays
  per provider.
- **Async launch prefetch** — non-blocking, same TTL/filters as settings;
  `get_models` joins an in-flight fetch instead of starting a second one.
- **No** hosted catalog, typed-in custom id field, `models_updated` event,
  or TLA change.

## Current state

- `TranslationModel` is a closed enum with frozen numeric ids
  (`library/src/translator.rs`). Comment: do not reorder or renumber.
  Serde is `from = "usize", into = "usize"`.
- `get_models` is sync `TranslationModel::iter()` plus pretty names in
  `site/src-tauri/src/app/config.rs`. No network, no API key.
- Wire names live in `gemini_model()` and two copies of
  `openai_model_name()` (paragraph translator and lyrics).
- Config, Tauri translate/lyrics commands, frontend `Config.model`, and
  e2e `MODEL = 1` all use the number.
- The same enum is stored on paragraph translations (tagged field,
  varint), chapter summaries (fixed varint), and lyrics cache filenames
  (`{model as usize}.json`).
- Paragraph tagged fields are length-prefixed; extra unread bytes in a
  field blob are ignored. Unknown `FieldTag` values currently fail
  deserialize, so a new tag would break mixed-version Syncthing peers.
  Config is per-device (not synced); library files are synced.

## Architecture

```
App launch ──spawn──► ModelCatalog::prefetch(providers with keys)
                          │
Settings / get_models ────┤  join in-flight, or cache, or GET /models
                          ▼
                    disk cache/model_catalog/<provider>.json
                          │
Config.model: String  ──► translators (Gemini Model::Custom / OpenAI model field)
```

A model is the provider’s API id string (`gpt-5-mini`,
`models/gemini-3.7-flash`, …). That string is what config stores, what IPC
passes, what translators send, and what Gemini’s prompt-cache key uses.

`TranslationProvider` is unchanged. JSON-schema vs JSON-object, base URL,
and Gemini prompt cache stay keyed on provider. The only leftover
per-model special case: if the Gemini id is `models/gemini-2.5-flash` or
`gemini-2.5-flash`, keep `thinking_budget: 0`; every other Gemini model
keeps today’s default thinking config.

### IPC / frontend

| Surface | Today | After |
|---|---|---|
| `Config.model` | number | string |
| `Model.id` | number | string |
| `ProviderMeta.defaultModelId` | number | `defaultModel: string` |
| `get_models` | sync, no keys | async, key-aware |
| `translate_paragraph` / `translate_chapter` / lyrics | `TranslationModel` | `model: String` |
| `Translator::get_model()` | enum | `String` |

CLI `translate` gains optional `--model <id>`, defaulting to the Google
fallback. It does not list models.

## Fallbacks (bootstrap + `defaultModel`)

| Provider | id | Display name |
|---|---|---|
| Google | `models/gemini-3.7-flash` | Gemini 3.7 Flash |
| OpenAI | `gpt-5-mini` | OpenAI GPT-5 mini |
| DeepSeek | `deepseek-v4-flash` | DeepSeek V4 Flash |
| z.AI | `glm-5.2` | z.AI GLM-5.2 |

These are also `Config`’s default model (Google row) and each
`ProviderMeta.defaultModel`. If a live list is missing its fallback id,
still include that fallback in `get_models` for that provider so the
default remains selectable.

## `ModelCatalog`

Lives in `library` (e.g. `translator/catalog.rs`) and on `AppState`. It is
the only code that calls list-model HTTP.

Disk: `{cache_dir}/model_catalog/<provider>.json` with
`{ fetchedAt: unix_secs, models: [{ id, name }] }`. `cache_dir` is
`resolve_cache_dir` (same root as Gemini prompt caches).

TTL: **24 hours**. List HTTP timeout: **10 seconds** (not the translation
timeouts). One in-flight list per provider (launch prefetch and
`get_models` share it).

### Resolve rules (per provider)

1. No API key → fallback row only, no network.
2. Disk cache younger than TTL → return it, no network.
3. Otherwise GET the list on the same base URL already used for chat
   (`FLTS_GEMINI_BASE_URL`, `OPENAI_BASE_URL`, `FLTS_DEEPSEEK_BASE_URL`,
   `FLTS_ZAI_BASE_URL`). Follow pagination (`nextPageToken` / `has_more`)
   up to **50 pages**, then stop with what was collected.
4. Success → write cache, return live list (plus fallback id if absent).
5. Failure (timeout, 401, 5xx, parse error, missing route) → log, return
   stale cache if present, else fallback. A 401 does **not** delete the
   cache. A bad row is skipped. One provider failing does not empty the
   others.
6. `get_models` itself does not return `Err` for list failures.

Launch: after config is loaded, spawn prefetch for every provider that has
a key. Failure never fails app start. Setup does not await the fetches.

`update_config`: if a provider’s API key **value** changed, delete that
provider’s cache file (and drop any in-memory/in-flight result). After a
successful save, settings calls `get_models` again.

### Filters

**Gemini** `GET {base}/v1beta/models?key=…` (key as query, matching
current Gemini client auth):

- Keep if `supportedGenerationMethods` contains `generateContent`.
- Drop if `name` contains any of: `embedding`, `imagen`, `veo`, `-image`,
  `aqa`, `tts` (non-text models that still advertise `generateContent`).
- `id` = `name` as returned (`models/…`).
- `name` (display) = `displayName` when non-empty, else `id`.

**OpenAI-compatible** (OpenAI, DeepSeek, z.AI) `GET {base}/models` via the
same client base as chat:

- `id` = `data[].id`.
- Display name = `id`.
- Drop if the id equals or starts with any of:

  `text-embedding-`, `embedding-`, `whisper-`, `tts-`, `dall-e-`,
  `gpt-image`, `chatgpt-image`, `omni-moderation`, `moderation-`,
  `sora-`, `computer-use`, `babbage`, `davinci`, `curie`, `ada`,
  `gpt-4o-transcribe`, `gpt-4o-mini-transcribe`, `gpt-4o-mini-tts`,
  `gpt-realtime`.

This is an exclude list of non-chat families, not an allowlist of model
names. New `gpt-*` / `o*` / `glm-*` / `deepseek-*` ids pass.

Sort each provider’s list by display name, case-insensitive.

## Config migration

`model` must deserialize **either** a JSON number **or** a string.
`Config::load` today treats any parse failure as corrupt and replaces the
live file with defaults (keys would appear gone). Numeric `model` from
existing installs must not take that path.

- Number `1…19` → string via the table below; persist as string on the
  next save.
- Number `0` or any other number → empty string (treated as fallback at
  use time).
- String → stored as-is (including unknown / orphan ids).

| Old id | API id |
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

Those strings are the names we already send today (`gemini-rust`
`Model::as_str()` for the built-in variants; `Model::Custom(…)` for the
rest; `openai_model_name` for OpenAI-compatible). After migration the
table is only used for this config path and for on-disk varint mapping
below.

Empty `config.model` when building a translator → that provider’s
fallback id, not an error.

## On-disk library files (mixed-version)

In-memory `ParagraphTranslation.model` and `ChapterSummary.model` become
`String`. Syncthing peers on the previous app version must still read
files we write.

**Paragraph translations (keep `FieldTag::TranslationModel = 1`, no new
tag, stay on `Version::V2`):**

Write the existing varint **and** a length-prefixed API id in the same
length-prefixed field blob:

```
varint legacy_id    // reverse-map through the table, or 0 if unknown
len-prefixed string // API id
```

Reverse-map is exact string match on the table’s API id column. Gemini
ids we persist are the `models/…` form the list endpoint returns;
do not strip or add that prefix at write time.

Old readers consume the varint and ignore trailing bytes. New readers
prefer the string when present; if the string is absent (file written by
an old app), map the varint through the table (`0` → empty).

If an old peer re-saves a file, the trailing string is dropped.
Provenance for ids not in the legacy table may collapse to unknown on
that rewrite; translation text is unaffected. Document that; do not bump
to `V3`.

**Chapter summaries (`CS01` v1, fixed layout):** keep the varint only.
Write reverse-mapped legacy id or `0`. Read maps varint → string via the
table. No format bump. Summaries are not shown in the model dropdown;
lossy provenance for post-table ids is acceptable.

**Lyrics disk cache filename:** stop using `model as usize`. Use a
filesystem-safe form of the API id (`/` → `_`). Old files miss and
refetch.

## Translators

`get_translator` / `get_lyrics_translator` take `(provider, model_id: &str, …)`.

- Gemini: `Model::Custom(id.to_string())` always. Drop `gemini_model()`.
- OpenAI-compatible: pass `model_id` through as the chat `model`. Drop
  both `openai_model_name` maps.
- Gemini prompt-cache `CacheKey.model` is the API id string.
- Do not locally reject unknown ids. Provider 400/404 surfaces as today.
- Provider/model mismatch is a settings concern (filter + provider-change
  reset), not a translator check.

## Settings UI

- Dropdown `value` is the string id. Filter by `provider === translationProvider`.
- Drop the `id === 0` “Not set” row.
- **Provider change or empty selection** → set `model` to that provider’s
  `defaultModel`. **Do not** reset merely because the saved id is missing
  from the current list (this changes today’s `$effect`, which treats
  “not in list” as mismatch).
- If `config.model` is non-empty and not in `filteredModels`, insert it
  at the top of the dropdown and show a short “not in the current
  catalog” note. Saving keeps the id.
- After a successful `update_config`, refetch `get_models` (covers key
  change → invalidated cache).

## Error handling (catalog)

- List 401 / timeout / 5xx / parse / 404 → log; stale cache or fallback.
- 401 does not delete cache.
- Skip malformed rows; do not fail the whole list.
- Launch prefetch errors are logs only.
- Translate/lyrics with an orphan or untested id: no extra local error
  path.

## Testing

Fixture HTTP, not live providers.

- Gemini filter: keep `generateContent`, drop embedding-only and
  `imagen` / `-image` names; `id` is `name`; `displayName` used when present.
- OpenAI-compatible filter: drop the exclude-prefix set; keep an id we
  have never shipped (e.g. `gpt-9-ultra`).
- No keys at all → exactly the four fallbacks (one per provider).
  One keyed provider plus three without → live (or cache) rows for the
  keyed provider, fallback row for each of the others.
- Fresh cache → no HTTP. Stale cache → HTTP. HTTP fail → stale if
  present, else fallback. 401 leaves the cache file in place.
- Pagination: two pages concatenate; a 51st page is not requested.
- Config load: `"model": 1` → `models/gemini-2.5-flash` without taking the
  corrupt-config path; `"model": "gpt-5-mini"` stored as-is; `0` / `99`
  → empty. Round-trip save writes a string.
- Empty `config.model` at translator build → fallback id.
- Translation serialize: new blob is readable as legacy varint; new
  reader recovers the string; a varint-only blob maps through the table.
- Gemini cache tests key by string id.
- In-flight: two concurrent `list` calls for the same provider issue one
  HTTP request.

`e2e-sims` LLM router adds `GET /v1beta/models`, `GET /v1/models`, and
`GET /models` returning a small fixture list (include one embedding id
that the filter must drop). These must not steal the existing POST
`/v1beta/models/{*model_action}` generate/stream routes.

Frontend/e2e: `model` is a string everywhere. `MODEL = 1` becomes
`models/gemini-2.5-flash` (the id the sim already uses). Settings: orphan
id remains in the dropdown; post-save refetch is covered. Bridge
conformance still lists `get_models`.

No new live-network tests. No TLA / trace-harness change: listing is not
on the translation-queue protocol. `TranslationModel` as a public enum
goes away; tests construct string ids.

## Out of scope

- Hosted / curated catalog.
- Free-typed model id field.
- Prefetch blocking app start.
- Changing JSON-schema vs JSON-object per model (stays per provider).
- Spotify, Anki, sync protocol.
- CI provider-list snapshots.
