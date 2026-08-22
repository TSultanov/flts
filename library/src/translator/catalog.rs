use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::TranslationProvider;

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

pub struct ReqwestListTransport {
    client: reqwest::Client,
}

impl ReqwestListTransport {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(LIST_TIMEOUT)
                .build()
                .expect("reqwest client"),
        }
    }
}

#[async_trait::async_trait]
impl ModelListTransport for ReqwestListTransport {
    async fn get_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> anyhow::Result<serde_json::Value> {
        let mut req = self.client.get(url);
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        Ok(req.send().await?.error_for_status()?.json().await?)
    }
}

type InflightCell = Arc<tokio::sync::OnceCell<Vec<ListedModel>>>;

pub struct ModelCatalog {
    cache_dir: PathBuf,
    transport: Arc<dyn ModelListTransport>,
    now_secs: Arc<dyn Fn() -> u64 + Send + Sync>,
    inflight: tokio::sync::Mutex<HashMap<TranslationProvider, InflightCell>>,
}

#[derive(Serialize, Deserialize)]
struct CachedCatalog {
    #[serde(rename = "fetchedAt")]
    fetched_at: u64,
    models: Vec<CachedModel>,
}

#[derive(Serialize, Deserialize)]
struct CachedModel {
    id: String,
    name: String,
}

impl ModelCatalog {
    pub fn new(cache_dir: PathBuf, transport: Arc<dyn ModelListTransport>) -> Self {
        Self::new_with_clock(
            cache_dir,
            transport,
            Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            }),
        )
    }

    /// `now_secs` is unix seconds; tests inject a `AtomicU64`.
    pub fn new_with_clock(
        cache_dir: PathBuf,
        transport: Arc<dyn ModelListTransport>,
        now_secs: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            cache_dir,
            transport,
            now_secs,
            inflight: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub async fn models_for(
        &self,
        provider: TranslationProvider,
        api_key: Option<&str>,
        list_base_url: &str,
    ) -> Vec<ListedModel> {
        let Some(api_key) = api_key.filter(|k| !k.is_empty()) else {
            return vec![fallback_for(provider)];
        };
        if let Some(models) = self.read_fresh_cache(provider) {
            return models;
        }

        let api_key = api_key.to_string();
        let list_base_url = list_base_url.to_string();
        let cell = {
            let mut map = self.inflight.lock().await;
            map.entry(provider)
                .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
                .clone()
        };
        let models = cell
            .get_or_init(|| async {
                match self.fetch_live(provider, &api_key, &list_base_url).await {
                    Ok(models) => {
                        self.write_cache(provider, &models);
                        models
                    }
                    Err(err) => {
                        log::warn!("model list fetch failed for {provider:?}: {err:#}");
                        self.read_cache(provider)
                            .unwrap_or_else(|| vec![fallback_for(provider)])
                    }
                }
            })
            .await
            .clone();
        {
            let mut map = self.inflight.lock().await;
            // Leave a newer fetch's cell in place.
            if map.get(&provider).is_some_and(|c| Arc::ptr_eq(c, &cell)) {
                map.remove(&provider);
            }
        }
        models
    }

    pub fn invalidate(&self, provider: TranslationProvider) {
        let _ = std::fs::remove_file(self.cache_path(provider));
        // Sync API over a tokio mutex; insert/remove holds are brief.
        if let Ok(mut map) = self.inflight.try_lock() {
            map.remove(&provider);
        }
    }

    fn cache_path(&self, provider: TranslationProvider) -> PathBuf {
        self.cache_dir
            .join("model_catalog")
            .join(format!("{}.json", provider_file_stem(provider)))
    }

    fn read_fresh_cache(&self, provider: TranslationProvider) -> Option<Vec<ListedModel>> {
        let (fetched_at, models) = self.read_cache_file(provider)?;
        let age = (self.now_secs)().saturating_sub(fetched_at);
        (age < LIST_TTL_SECS).then_some(models)
    }

    fn read_cache(&self, provider: TranslationProvider) -> Option<Vec<ListedModel>> {
        self.read_cache_file(provider).map(|(_, models)| models)
    }

    fn read_cache_file(&self, provider: TranslationProvider) -> Option<(u64, Vec<ListedModel>)> {
        let bytes = std::fs::read(self.cache_path(provider)).ok()?;
        let parsed: CachedCatalog = serde_json::from_slice(&bytes).ok()?;
        let models = parsed
            .models
            .into_iter()
            .map(|m| ListedModel {
                id: m.id,
                name: m.name,
                provider,
            })
            .collect();
        Some((parsed.fetched_at, models))
    }

    fn write_cache(&self, provider: TranslationProvider, models: &[ListedModel]) {
        let path = self.cache_path(provider);
        if let Err(err) = write_cache_file(&path, (self.now_secs)(), models) {
            log::warn!("failed to write model catalog cache: {err:#}");
        }
    }

    async fn fetch_live(
        &self,
        provider: TranslationProvider,
        api_key: &str,
        list_base_url: &str,
    ) -> anyhow::Result<Vec<ListedModel>> {
        let mut collected = Vec::new();
        if provider == TranslationProvider::Google {
            let mut page_token: Option<String> = None;
            for _ in 0..LIST_MAX_PAGES {
                let url = gemini_list_url(list_base_url, api_key, page_token.as_deref());
                let body = self.transport.get_json(&url, &[]).await?;
                collected.extend(filter_gemini_models(&body));
                page_token = body
                    .get("nextPageToken")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                if page_token.is_none() {
                    break;
                }
            }
        } else {
            let auth = format!("Bearer {api_key}");
            let headers = [("Authorization", auth.as_str())];
            let mut after: Option<String> = None;
            for _ in 0..LIST_MAX_PAGES {
                let url = openai_list_url(list_base_url, after.as_deref());
                let body = self.transport.get_json(&url, &headers).await?;
                collected.extend(filter_openai_compat_models(&body, provider));
                let has_more = body
                    .get("has_more")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !has_more {
                    break;
                }
                after = last_openai_id(&body);
                if after.is_none() {
                    break;
                }
            }
        }
        ensure_fallback(&mut collected, provider);
        sort_by_display_name(&mut collected);
        Ok(collected)
    }
}

pub fn join_models_url(base: &str) -> String {
    format!("{}/models", base.trim_end_matches('/'))
}

const DEFAULT_GEMINI_LIST_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/";
const DEFAULT_OPENAI_LIST_BASE: &str = "https://api.openai.com/v1";

fn nonempty_or(env_val: Option<String>, default: &str) -> String {
    env_val
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// List origin for `models_for`. Empty env values are treated as unset.
pub fn list_base_url(
    provider: TranslationProvider,
    gemini_env: Option<String>,
    openai_env: Option<String>,
    deepseek_env: Option<String>,
    zai_env: Option<String>,
    openrouter_env: Option<String>,
) -> String {
    match provider {
        TranslationProvider::Google => nonempty_or(gemini_env, DEFAULT_GEMINI_LIST_BASE),
        TranslationProvider::Openai => nonempty_or(openai_env, DEFAULT_OPENAI_LIST_BASE),
        TranslationProvider::Deepseek => {
            nonempty_or(deepseek_env, crate::translator::openai::DEEPSEEK_BASE_URL)
        }
        TranslationProvider::Zai => nonempty_or(zai_env, crate::translator::openai::ZAI_BASE_URL),
        TranslationProvider::Openrouter => {
            nonempty_or(openrouter_env, crate::translator::openai::OPENROUTER_BASE_URL)
        }
    }
}

pub fn list_base_url_from_env(provider: TranslationProvider) -> String {
    list_base_url(
        provider,
        std::env::var("FLTS_GEMINI_BASE_URL").ok(),
        std::env::var("OPENAI_BASE_URL").ok(),
        std::env::var("FLTS_DEEPSEEK_BASE_URL").ok(),
        std::env::var("FLTS_ZAI_BASE_URL").ok(),
        std::env::var("FLTS_OPENROUTER_BASE_URL").ok(),
    )
}

// Serde lowercase (`google`/`zai`), not display_name() (`Google`/`z.AI`).
fn provider_file_stem(provider: TranslationProvider) -> &'static str {
    match provider {
        TranslationProvider::Google => "google",
        TranslationProvider::Openai => "openai",
        TranslationProvider::Deepseek => "deepseek",
        TranslationProvider::Zai => "zai",
        TranslationProvider::Openrouter => "openrouter",
    }
}

fn encode_query_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

fn gemini_list_url(base: &str, api_key: &str, page_token: Option<&str>) -> String {
    let mut url = format!(
        "{}?key={}",
        join_models_url(base),
        encode_query_component(api_key)
    );
    if let Some(token) = page_token {
        url.push_str("&pageToken=");
        url.push_str(&encode_query_component(token));
    }
    url
}

fn openai_list_url(base: &str, after: Option<&str>) -> String {
    match after {
        Some(id) => format!(
            "{}?after={}",
            join_models_url(base),
            encode_query_component(id)
        ),
        None => join_models_url(base),
    }
}

fn last_openai_id(body: &serde_json::Value) -> Option<String> {
    body.get("data")
        .and_then(|v| v.as_array())
        .and_then(|rows| rows.last())
        .and_then(|row| row.get("id"))
        .and_then(|id| id.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn ensure_fallback(models: &mut Vec<ListedModel>, provider: TranslationProvider) {
    let fallback = fallback_for(provider);
    if !models.iter().any(|m| m.id == fallback.id) {
        models.push(fallback);
    }
}

fn write_cache_file(path: &Path, fetched_at: u64, models: &[ListedModel]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = CachedCatalog {
        fetched_at,
        models: models
            .iter()
            .map(|m| CachedModel {
                id: m.id.clone(),
                name: m.name.clone(),
            })
            .collect(),
    };
    std::fs::write(path, serde_json::to_vec(&body)?)?;
    Ok(())
}

pub const FALLBACK_GOOGLE: &str = "models/gemini-3.7-flash";
pub const FALLBACK_OPENAI: &str = "gpt-5-mini";
pub const FALLBACK_DEEPSEEK: &str = "deepseek-v4-flash";
pub const FALLBACK_ZAI: &str = "glm-5.2";
pub const FALLBACK_OPENROUTER: &str = "~deepseek/deepseek-v4-flash-latest";
/// OpenRouter alias IDs use a leading `~`; this unprefixed form was never valid.
const DEPRECATED_OPENROUTER_FLASH_LATEST: &str = "deepseek/deepseek-v4-flash-latest";

const LEGACY_API_IDS: &[&str] = &[
    "", // 0 / unknown
    "models/gemini-2.5-flash",
    "models/gemini-2.5-pro",
    "models/gemini-2.5-flash-lite",
    "gpt-5-mini",
    "gpt-5.2",
    "gpt-5.2-pro",
    "gpt-5-nano",
    "models/gemini-3-pro-preview",
    "models/gemini-3-flash-preview",
    "gpt-5.4",
    "gpt-5.4-mini",
    "models/gemini-3.1-pro-preview",
    "models/gemini-3.1-flash-lite-preview",
    "models/gemini-3.5-flash",
    "deepseek-v4-flash",
    "deepseek-v4-pro",
    "glm-5.2",
    "models/gemini-3.6-flash",
    "models/gemini-3.7-flash",
];

const GEMINI_NAME_EXCLUDES: &[&str] = &["embedding", "imagen", "veo", "-image", "aqa", "tts"];

const OPENAI_ID_EXCLUDES: &[&str] = &[
    "text-embedding-",
    "embedding-",
    "whisper-",
    "tts-",
    "dall-e-",
    "gpt-image",
    "chatgpt-image",
    "omni-moderation",
    "moderation-",
    "sora-",
    "computer-use",
    "babbage",
    "davinci",
    "curie",
    "ada",
    "gpt-4o-transcribe",
    "gpt-4o-mini-transcribe",
    "gpt-4o-mini-tts",
    "gpt-realtime",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedModel {
    pub id: String,
    pub name: String,
    pub provider: TranslationProvider,
}

pub fn fallback_for(provider: TranslationProvider) -> ListedModel {
    let id = match provider {
        TranslationProvider::Google => FALLBACK_GOOGLE,
        TranslationProvider::Openai => FALLBACK_OPENAI,
        TranslationProvider::Deepseek => FALLBACK_DEEPSEEK,
        TranslationProvider::Zai => FALLBACK_ZAI,
        TranslationProvider::Openrouter => FALLBACK_OPENROUTER,
    };
    ListedModel {
        id: id.to_string(),
        name: id.to_string(),
        provider,
    }
}

pub fn all_fallbacks() -> Vec<ListedModel> {
    vec![
        fallback_for(TranslationProvider::Google),
        fallback_for(TranslationProvider::Openai),
        fallback_for(TranslationProvider::Deepseek),
        fallback_for(TranslationProvider::Zai),
        fallback_for(TranslationProvider::Openrouter),
    ]
}

pub fn api_id_from_legacy(id: u64) -> String {
    LEGACY_API_IDS
        .get(id as usize)
        .copied()
        .unwrap_or("")
        .to_string()
}

pub fn legacy_id_from_api(id: &str) -> u64 {
    if id.is_empty() {
        return 0;
    }
    LEGACY_API_IDS
        .iter()
        .enumerate()
        .find(|(_, api)| **api == id)
        .map(|(i, _)| i as u64)
        .unwrap_or(0)
}

pub fn effective_model_id(provider: TranslationProvider, config_model: &str) -> String {
    if config_model.trim().is_empty() {
        return fallback_for(provider).id;
    }
    if provider == TranslationProvider::Openrouter
        && config_model == DEPRECATED_OPENROUTER_FLASH_LATEST
    {
        return FALLBACK_OPENROUTER.to_string();
    }
    config_model.to_string()
}

pub fn filter_gemini_models(body: &serde_json::Value) -> Vec<ListedModel> {
    let Some(models) = body.get("models").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for model in models {
        let Some(id) = model.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let methods = model
            .get("supportedGenerationMethods")
            .and_then(|v| v.as_array());
        let Some(methods) = methods else {
            continue;
        };
        let supports_generate = methods
            .iter()
            .any(|m| m.as_str() == Some("generateContent"));
        if !supports_generate {
            continue;
        }
        if GEMINI_NAME_EXCLUDES.iter().any(|tok| id.contains(tok)) {
            continue;
        }
        let name = model
            .get("displayName")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(id)
            .to_string();
        out.push(ListedModel {
            id: id.to_string(),
            name,
            provider: TranslationProvider::Google,
        });
    }

    sort_by_display_name(&mut out);
    out
}

pub fn filter_openai_compat_models(
    body: &serde_json::Value,
    provider: TranslationProvider,
) -> Vec<ListedModel> {
    let Some(data) = body.get("data").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for model in data {
        let Some(id) = model.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if OPENAI_ID_EXCLUDES
            .iter()
            .any(|tok| id == *tok || id.starts_with(tok))
        {
            continue;
        }
        out.push(ListedModel {
            id: id.to_string(),
            name: id.to_string(),
            provider,
        });
    }

    sort_by_display_name(&mut out);
    out
}

fn sort_by_display_name(models: &mut [ListedModel]) {
    models.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::Duration;

    use serde_json::{Value, json};

    struct FakeTransport {
        hits: AtomicUsize,
        delay: Duration,
        handler: Box<dyn Fn(&str) -> anyhow::Result<Value> + Send + Sync>,
    }

    #[async_trait::async_trait]
    impl ModelListTransport for FakeTransport {
        async fn get_json(&self, url: &str, _headers: &[(&str, &str)]) -> anyhow::Result<Value> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            (self.handler)(url)
        }
    }

    impl FakeTransport {
        fn new(
            handler: impl Fn(&str) -> anyhow::Result<Value> + Send + Sync + 'static,
        ) -> Arc<Self> {
            Arc::new(Self {
                hits: AtomicUsize::new(0),
                delay: Duration::ZERO,
                handler: Box::new(handler),
            })
        }

        fn with_delay(
            delay: Duration,
            handler: impl Fn(&str) -> anyhow::Result<Value> + Send + Sync + 'static,
        ) -> Arc<Self> {
            Arc::new(Self {
                hits: AtomicUsize::new(0),
                delay,
                handler: Box::new(handler),
            })
        }
    }

    const NOW: u64 = 1_710_000_000;

    fn clock(now: &Arc<AtomicU64>) -> Arc<dyn Fn() -> u64 + Send + Sync> {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst))
    }

    fn catalog(
        cache_dir: std::path::PathBuf,
        transport: Arc<dyn ModelListTransport>,
        now: &Arc<AtomicU64>,
    ) -> ModelCatalog {
        ModelCatalog::new_with_clock(cache_dir, transport, clock(now))
    }

    fn cache_path(dir: &std::path::Path, provider: TranslationProvider) -> std::path::PathBuf {
        let stem = match provider {
            TranslationProvider::Google => "google",
            TranslationProvider::Openai => "openai",
            TranslationProvider::Deepseek => "deepseek",
            TranslationProvider::Zai => "zai",
            TranslationProvider::Openrouter => "openrouter",
        };
        dir.join("model_catalog").join(format!("{stem}.json"))
    }

    fn write_cache(
        dir: &std::path::Path,
        provider: TranslationProvider,
        fetched_at: u64,
        models: &[(&str, &str)],
    ) {
        let path = cache_path(dir, provider);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let models: Vec<Value> = models
            .iter()
            .map(|(id, name)| json!({"id": id, "name": name}))
            .collect();
        std::fs::write(
            path,
            json!({"fetchedAt": fetched_at, "models": models}).to_string(),
        )
        .unwrap();
    }

    fn ids(models: &[ListedModel]) -> Vec<&str> {
        models.iter().map(|m| m.id.as_str()).collect()
    }

    fn gemini_chat(id: &str) -> Value {
        json!({
            "name": id,
            "displayName": id,
            "supportedGenerationMethods": ["generateContent"]
        })
    }

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
    fn effective_id_migrates_deprecated_openrouter_flash_latest_alias() {
        assert_eq!(
            effective_model_id(
                TranslationProvider::Openrouter,
                DEPRECATED_OPENROUTER_FLASH_LATEST,
            ),
            FALLBACK_OPENROUTER
        );
        assert_eq!(
            effective_model_id(
                TranslationProvider::Openrouter,
                FALLBACK_OPENROUTER,
            ),
            FALLBACK_OPENROUTER
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

    #[tokio::test]
    async fn no_key_returns_fallback_without_http() {
        let tmp = crate::test_utils::TempDir::new("catalog");
        let transport = FakeTransport::new(|_| panic!("http should not be called"));
        let now = Arc::new(AtomicU64::new(NOW));
        let cat = catalog(tmp.path.clone(), transport.clone(), &now);

        let none = cat
            .models_for(TranslationProvider::Google, None, "https://example/v1beta")
            .await;
        let empty = cat
            .models_for(
                TranslationProvider::Google,
                Some(""),
                "https://example/v1beta",
            )
            .await;

        assert_eq!(none, vec![fallback_for(TranslationProvider::Google)]);
        assert_eq!(empty, none);
        assert_eq!(transport.hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn fresh_cache_skips_http() {
        let tmp = crate::test_utils::TempDir::new("catalog");
        write_cache(
            &tmp.path,
            TranslationProvider::Openai,
            NOW,
            &[("gpt-from-cache", "gpt-from-cache")],
        );
        let transport = FakeTransport::new(|_| panic!("http should not be called"));
        let now = Arc::new(AtomicU64::new(NOW));
        let cat = catalog(tmp.path.clone(), transport.clone(), &now);

        let got = cat
            .models_for(
                TranslationProvider::Openai,
                Some("k"),
                "https://api.openai.com/v1/",
            )
            .await;

        assert_eq!(ids(&got), ["gpt-from-cache"]);
        assert_eq!(got[0].provider, TranslationProvider::Openai);
        assert_eq!(transport.hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn stale_cache_refetches() {
        let tmp = crate::test_utils::TempDir::new("catalog");
        write_cache(
            &tmp.path,
            TranslationProvider::Openai,
            NOW - LIST_TTL_SECS - 1,
            &[("gpt-old", "gpt-old")],
        );
        let transport = FakeTransport::new(|_| Ok(json!({"data": [{"id": "gpt-new"}]})));
        let now = Arc::new(AtomicU64::new(NOW));
        let cat = catalog(tmp.path.clone(), transport.clone(), &now);

        let got = cat
            .models_for(
                TranslationProvider::Openai,
                Some("k"),
                "https://api.openai.com/v1",
            )
            .await;

        assert_eq!(transport.hits.load(Ordering::SeqCst), 1);
        assert!(ids(&got).contains(&"gpt-new"));
        let disk: Value = serde_json::from_str(
            &std::fs::read_to_string(cache_path(&tmp.path, TranslationProvider::Openai)).unwrap(),
        )
        .unwrap();
        assert_eq!(disk["fetchedAt"], NOW);
        assert!(
            disk["models"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["id"] == "gpt-new")
        );
        assert!(disk.get("provider").is_none());
    }

    #[tokio::test]
    async fn http_failure_uses_stale_cache() {
        let tmp = crate::test_utils::TempDir::new("catalog");
        write_cache(
            &tmp.path,
            TranslationProvider::Openai,
            NOW - LIST_TTL_SECS - 1,
            &[("gpt-stale", "gpt-stale")],
        );
        let transport = FakeTransport::new(|_| Err(anyhow::anyhow!("network down")));
        let now = Arc::new(AtomicU64::new(NOW));
        let cat = catalog(tmp.path.clone(), transport.clone(), &now);

        let got = cat
            .models_for(
                TranslationProvider::Openai,
                Some("k"),
                "https://api.openai.com/v1",
            )
            .await;

        assert_eq!(ids(&got), ["gpt-stale"]);
        assert!(cache_path(&tmp.path, TranslationProvider::Openai).exists());
    }

    #[tokio::test]
    async fn http_401_does_not_delete_cache() {
        let tmp = crate::test_utils::TempDir::new("catalog");
        write_cache(
            &tmp.path,
            TranslationProvider::Openai,
            NOW - LIST_TTL_SECS - 1,
            &[("gpt-stale", "gpt-stale")],
        );
        let transport = FakeTransport::new(|_| Err(anyhow::anyhow!("401 unauthorized")));
        let now = Arc::new(AtomicU64::new(NOW));
        let cat = catalog(tmp.path.clone(), transport.clone(), &now);

        let _got = cat
            .models_for(
                TranslationProvider::Openai,
                Some("k"),
                "https://api.openai.com/v1",
            )
            .await;

        assert!(cache_path(&tmp.path, TranslationProvider::Openai).exists());
    }

    #[tokio::test]
    async fn no_cache_http_failure_uses_fallback() {
        let tmp = crate::test_utils::TempDir::new("catalog");
        let transport = FakeTransport::new(|_| Err(anyhow::anyhow!("no route")));
        let now = Arc::new(AtomicU64::new(NOW));
        let cat = catalog(tmp.path.clone(), transport.clone(), &now);

        let got = cat
            .models_for(
                TranslationProvider::Deepseek,
                Some("k"),
                "https://api.deepseek.com",
            )
            .await;

        assert!(transport.hits.load(Ordering::SeqCst) >= 1);
        assert_eq!(got, vec![fallback_for(TranslationProvider::Deepseek)]);
    }

    #[tokio::test]
    async fn pagination_concatenates_two_pages_and_stops_at_cap() {
        let tmp = crate::test_utils::TempDir::new("catalog");
        let now = Arc::new(AtomicU64::new(NOW));

        let gemini = FakeTransport::new(|url| {
            assert!(url.contains("/models"), "{url}");
            assert!(url.contains("key="), "{url}");
            if url.contains("pageToken=t2") {
                Ok(json!({"models": [gemini_chat("models/gemini-page-two")]}))
            } else {
                assert!(
                    !url.contains("pageToken="),
                    "first page must omit pageToken, got {url}"
                );
                Ok(json!({
                    "models": [gemini_chat("models/gemini-page-one")],
                    "nextPageToken": "t2"
                }))
            }
        });
        let cat = catalog(tmp.path.clone(), gemini.clone(), &now);
        let got = cat
            .models_for(
                TranslationProvider::Google,
                Some("k"),
                "https://generativelanguage.googleapis.com/v1beta",
            )
            .await;
        let got_ids = ids(&got);
        assert!(got_ids.contains(&"models/gemini-page-one"), "{got_ids:?}");
        assert!(got_ids.contains(&"models/gemini-page-two"), "{got_ids:?}");
        assert_eq!(gemini.hits.load(Ordering::SeqCst), 2);
        cat.invalidate(TranslationProvider::Google);

        let cap = FakeTransport::new(|_| {
            Ok(json!({
                "models": [gemini_chat("models/gemini-cap")],
                "nextPageToken": "more"
            }))
        });
        let cat = catalog(tmp.path.clone(), cap.clone(), &now);
        let _got = cat
            .models_for(
                TranslationProvider::Google,
                Some("k"),
                "https://generativelanguage.googleapis.com/v1beta",
            )
            .await;
        assert_eq!(cap.hits.load(Ordering::SeqCst), LIST_MAX_PAGES);

        let openai = FakeTransport::new(|url| {
            if url.contains("after=") {
                assert!(
                    url.contains("after=gpt-page-a"),
                    "second page must pass after= last id, got {url}"
                );
                Ok(json!({"data": [{"id": "gpt-page-b"}], "has_more": false}))
            } else {
                Ok(json!({"data": [{"id": "gpt-page-a"}], "has_more": true}))
            }
        });
        let cat = catalog(tmp.path.clone(), openai.clone(), &now);
        let got = cat
            .models_for(
                TranslationProvider::Openai,
                Some("k"),
                "https://api.openai.com/v1/",
            )
            .await;
        let got_ids = ids(&got);
        assert!(got_ids.contains(&"gpt-page-a"), "{got_ids:?}");
        assert!(got_ids.contains(&"gpt-page-b"), "{got_ids:?}");
        assert_eq!(openai.hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn inflight_dedupes_concurrent_calls() {
        let tmp = crate::test_utils::TempDir::new("catalog");
        let transport = FakeTransport::with_delay(Duration::from_millis(50), |_| {
            Ok(json!({"data": [{"id": "gpt-9-ultra"}]}))
        });
        let now = Arc::new(AtomicU64::new(NOW));
        let cat = Arc::new(catalog(tmp.path.clone(), transport.clone(), &now));

        let (a, b) = tokio::join!(
            cat.models_for(
                TranslationProvider::Openai,
                Some("k"),
                "https://api.openai.com/v1"
            ),
            cat.models_for(
                TranslationProvider::Openai,
                Some("k"),
                "https://api.openai.com/v1"
            ),
        );

        assert_eq!(transport.hits.load(Ordering::SeqCst), 1);
        assert_eq!(ids(&a), ids(&b));
    }

    #[tokio::test]
    async fn live_list_missing_fallback_still_includes_it() {
        let tmp = crate::test_utils::TempDir::new("catalog");
        let transport = FakeTransport::new(|_| Ok(json!({"data": [{"id": "gpt-9-ultra"}]})));
        let now = Arc::new(AtomicU64::new(NOW));
        let cat = catalog(tmp.path.clone(), transport.clone(), &now);

        let got = cat
            .models_for(
                TranslationProvider::Openai,
                Some("k"),
                "https://api.openai.com/v1",
            )
            .await;
        let got_ids = ids(&got);
        assert!(got_ids.contains(&FALLBACK_OPENAI), "{got_ids:?}");
        assert!(got_ids.contains(&"gpt-9-ultra"), "{got_ids:?}");
    }

    #[test]
    fn list_base_url_uses_env_or_defaults() {
        assert_eq!(
            list_base_url(TranslationProvider::Google, None, None, None, None, None),
            "https://generativelanguage.googleapis.com/v1beta/"
        );
        assert_eq!(
            list_base_url(
                TranslationProvider::Google,
                Some(String::new()),
                None,
                None,
                None,
                None
            ),
            "https://generativelanguage.googleapis.com/v1beta/"
        );
        assert_eq!(
            list_base_url(
                TranslationProvider::Google,
                Some("https://proxy/v1beta/".into()),
                None,
                None,
                None,
                None
            ),
            "https://proxy/v1beta/"
        );
        assert_eq!(
            list_base_url(
                TranslationProvider::Openai,
                None,
                Some("http://127.0.0.1:8080/v1".into()),
                None,
                None,
                None
            ),
            "http://127.0.0.1:8080/v1"
        );
        assert_eq!(
            list_base_url(TranslationProvider::Openai, None, None, None, None, None),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            list_base_url(TranslationProvider::Deepseek, None, None, None, None, None),
            crate::translator::openai::DEEPSEEK_BASE_URL
        );
        assert_eq!(
            list_base_url(
                TranslationProvider::Deepseek,
                None,
                None,
                Some("http://ds".into()),
                None,
                None
            ),
            "http://ds"
        );
        assert_eq!(
            list_base_url(TranslationProvider::Zai, None, None, None, None, None),
            crate::translator::openai::ZAI_BASE_URL
        );
        assert_eq!(
            list_base_url(TranslationProvider::Openrouter, None, None, None, None, None),
            crate::translator::openai::OPENROUTER_BASE_URL
        );
        assert_eq!(
            list_base_url(
                TranslationProvider::Openrouter,
                None,
                None,
                None,
                None,
                Some("http://or".into())
            ),
            "http://or"
        );
        assert_eq!(
            fallback_for(TranslationProvider::Openrouter).id,
            FALLBACK_OPENROUTER
        );
    }

    #[test]
    fn list_urls_percent_encode_query_values() {
        let url = gemini_list_url("https://example/v1beta", "k/&=", Some("t/2"));
        assert!(url.contains("key=k%2F%26%3D"), "{url}");
        assert!(url.contains("pageToken=t%2F2"), "{url}");

        let url = openai_list_url("https://api.openai.com/v1", Some("id&x"));
        assert!(url.contains("after=id%26x"), "{url}");
    }
}
