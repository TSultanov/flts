use std::{
    sync::{Arc, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures_util::TryStreamExt;
use gemini_rust::{CachedContentHandle, Gemini, Model, ThinkingConfig};
use isolang::Language;
use log::{info, warn};
use serde_json::Value;
use tokio::time::timeout;

use crate::{
    book::translation_import::ParagraphTranslation,
    cache::TranslationsCache,
    translator::{
        ChapterContextProvider, ProgressCallback, TranslationContext, TranslationErrors,
        TranslationModel, Translator,
        gemini_cache::{CacheKey, GeminiCacheRegistry, is_cache_missing_error},
        paragraph_translation_schema, strip_additional_properties,
    },
};

/// Process-internal Gemini content-cache registry. Created lazily on first
/// translation. Lives for the lifetime of the process — its entries are
/// cheap (one `Arc<CachedContentHandle>` per (model, from, to)) and the
/// server-side caches behind them auto-expire on TTL.
static REGISTRY: OnceLock<Arc<GeminiCacheRegistry>> = OnceLock::new();

fn registry() -> &'static Arc<GeminiCacheRegistry> {
    REGISTRY.get_or_init(GeminiCacheRegistry::new)
}

use super::{
    StreamChunkAccumulator, TRANSLATION_REQUEST_TIMEOUT, TRANSLATION_STREAM_IDLE_TIMEOUT,
    total_stream_timeout,
};

pub(crate) fn gemini_model(m: TranslationModel) -> anyhow::Result<Model> {
    Ok(match m {
        TranslationModel::Gemini25Flash => Model::Gemini25Flash,
        TranslationModel::Gemini25Pro => Model::Gemini25Pro,
        TranslationModel::Gemini25FlashLight => Model::Gemini25FlashLite,
        TranslationModel::Gemini3Pro => Model::Gemini3Pro,
        TranslationModel::Gemini3Flash => Model::Gemini3Flash,
        TranslationModel::Gemini31Pro => Model::Custom("models/gemini-3.1-pro-preview".to_string()),
        TranslationModel::Gemini31FlashLite => {
            Model::Custom("models/gemini-3.1-flash-lite-preview".to_string())
        }
        _ => Err(TranslationErrors::UnknownModel)?,
    })
}

pub(crate) fn gemini_client(api_key: String, model: Model) -> anyhow::Result<Gemini> {
    Ok(Gemini::with_model(api_key, model)?)
}

/// The shared paragraph schema is OpenAI-strict (uses `additionalProperties: false`).
/// Gemini rejects that key with HTTP 400, so we hand it a stripped variant.
pub(crate) fn gemini_paragraph_schema() -> Value {
    let mut s = paragraph_translation_schema();
    strip_additional_properties(&mut s);
    s
}

pub struct GeminiTranslator {
    cache: Arc<TranslationsCache>,
    #[allow(dead_code)] // wired in Phase 4 (per-chapter cache payload)
    context_provider: Arc<dyn ChapterContextProvider>,
    client: Gemini,
    schema: Arc<Value>,
    model: Model,
    translation_model: TranslationModel,
    from: Language,
    to: Language,
}

impl GeminiTranslator {
    pub fn create(
        cache: Arc<TranslationsCache>,
        context_provider: Arc<dyn ChapterContextProvider>,
        translation_model: TranslationModel,
        api_key: String,
        from: &Language,
        to: &Language,
    ) -> anyhow::Result<GeminiTranslator> {
        let model = gemini_model(translation_model)?;
        let client = gemini_client(api_key, model.clone())?;

        Ok(Self {
            cache,
            context_provider,
            client,
            schema: Arc::new(gemini_paragraph_schema()),
            model,
            translation_model,
            from: *from,
            to: *to,
        })
    }

    fn cache_key(&self) -> CacheKey {
        CacheKey {
            model: self.translation_model,
            from: self.from,
            to: self.to,
        }
    }

    fn thinking_config(&self) -> ThinkingConfig {
        match &self.model {
            Model::Gemini25Flash => ThinkingConfig {
                thinking_budget: Some(0),
                include_thoughts: Some(false),
                thinking_level: None,
            },
            _ => ThinkingConfig {
                thinking_budget: None,
                include_thoughts: Some(false),
                thinking_level: None,
            },
        }
    }

    /// One full attempt: fetch (or create) the cache, build the request,
    /// drain the stream, decode. The caller wraps this so that a missing /
    /// expired cache can be evicted and retried once.
    async fn attempt_translation(
        &self,
        paragraph: &str,
        callback: Option<&ProgressCallback>,
    ) -> anyhow::Result<ParagraphTranslation> {
        let from = self.from;
        let to = self.to;
        let cache_handle: Arc<CachedContentHandle> = registry()
            .get_or_create(&self.client, self.cache_key(), || {
                Self::get_prompt(from.to_name(), to.to_name())
            })
            .await?;

        let mut stream = timeout(
            TRANSLATION_REQUEST_TIMEOUT,
            self.client
                .generate_content()
                .with_cached_content(&cache_handle)
                .with_user_message(paragraph)
                .with_response_mime_type("application/json")
                .with_response_schema((*self.schema).clone())
                .with_thinking_config(self.thinking_config())
                .execute_stream(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Gemini request timed out"))??;

        let mut accumulator = StreamChunkAccumulator::new("Gemini");

        let full_content = timeout(total_stream_timeout(paragraph.len()), async {
            loop {
                let next = timeout(TRANSLATION_STREAM_IDLE_TIMEOUT, stream.try_next())
                    .await
                    .map_err(|_| anyhow::anyhow!("Gemini stream timed out"))?;
                let should_continue = accumulator.handle_result(
                    match next {
                        Ok(Some(response)) => Ok(Some(response.text())),
                        Ok(None) => Ok(None),
                        Err(err) => Err(err.into()),
                    },
                    callback,
                )?;
                if !should_continue {
                    break;
                }
            }
            accumulator.finish()
        })
        .await
        .map_err(|_| anyhow::anyhow!("Gemini total stream timeout"))??;

        let translation: ParagraphTranslation = serde_json::from_str(&full_content)?;
        Ok(translation)
    }
}

#[async_trait]
impl Translator for GeminiTranslator {
    fn get_model(&self) -> super::TranslationModel {
        self.translation_model
    }

    async fn get_translation(
        &self,
        ctx: TranslationContext<'_>,
    ) -> anyhow::Result<ParagraphTranslation> {
        if ctx.use_cache
            && let Some(cached_result) = self
                .cache
                .get(&self.from, &self.to, ctx.paragraph_text)
                .await
                .ok()
                .flatten()
        {
            return Ok(cached_result);
        }

        let paragraph = ctx.paragraph_text;
        let cb = ctx.callback.as_deref();
        let mut translation = match self.attempt_translation(paragraph, cb).await {
            Ok(t) => t,
            Err(err) if is_cache_missing_error(&err) => {
                warn!(
                    "Gemini cache appears expired/missing; evicting and retrying. ({err})"
                );
                registry().evict(&self.cache_key()).await;
                self.attempt_translation(paragraph, cb).await?
            }
            Err(err) => return Err(err),
        };

        let now = SystemTime::now();
        let duration_since_epoch = now.duration_since(UNIX_EPOCH)?;
        translation.timestamp = duration_since_epoch.as_secs();

        self.cache
            .set(&self.from, &self.to, paragraph, &translation);

        info!(
            "Gemini translation complete (paragraph {} chars, response {} chars)",
            paragraph.len(),
            full_content_size(&translation),
        );

        Ok(translation)
    }
}

fn full_content_size(t: &ParagraphTranslation) -> usize {
    serde_json::to_string(t).map(|s| s.len()).unwrap_or(0)
}
