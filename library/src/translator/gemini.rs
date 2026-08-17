use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures_util::TryStreamExt;
use gemini_rust::{
    CachedContentHandle, FinishReason, Gemini, HarmBlockThreshold, HarmCategory, Model,
    SafetySetting, ThinkingConfig, UsageMetadata,
};
use isolang::Language;
use log::{debug, info, warn};
use serde_json::Value;
use tokio::time::timeout;

use crate::{
    book::translation_import::ParagraphTranslation,
    cache::TranslationsCache,
    translator::{
        ChapterContextProvider, ProgressCallback, TranslationContext, TranslationErrors,
        TranslationModel, Translator,
        gemini_cache::{
            CacheContent, CacheKey, GeminiPromptCache, build_reference_material,
            is_cache_missing_error,
        },
        paragraph_translation_schema, strip_additional_properties,
    },
};
use uuid::Uuid;

use super::{
    StreamChunkAccumulator, TRANSLATION_REQUEST_TIMEOUT, TRANSLATION_STREAM_IDLE_TIMEOUT,
    total_stream_timeout,
};

/// The cached-content POST runs before `TRANSLATION_REQUEST_TIMEOUT` covers
/// anything and gemini-rust's client has no timeout, so an unbounded await
/// here would silently hang every paragraph sharing the cache init future.
const CACHE_CREATE_TIMEOUT: Duration = Duration::from_secs(120);

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
        },
        TranslationModel::Gemini35Flash => {
            Model::Custom("models/gemini-3.5-flash".to_string())
        },
        TranslationModel::Gemini36Flash => {
            Model::Custom("models/gemini-3.6-flash".to_string())
        },
        TranslationModel::Gemini37Flash => {
            Model::Custom("models/gemini-3.7-flash".to_string())
        },
        _ => Err(TranslationErrors::UnknownModel)?,
    })
}

/// Empty is treated as unset so an exported-but-blank var doesn't break the client.
fn gemini_base_url_override(raw: Option<String>) -> Option<reqwest::Url> {
    raw.filter(|s| !s.is_empty()).and_then(|s| s.parse().ok())
}

pub(crate) fn gemini_client(api_key: String, model: Model) -> anyhow::Result<Gemini> {
    match gemini_base_url_override(std::env::var("FLTS_GEMINI_BASE_URL").ok()) {
        Some(url) => Ok(Gemini::with_model_and_base_url(api_key, model, url)?),
        None => Ok(Gemini::with_model(api_key, model)?),
    }
}

/// Permissive safety settings: book translation reproduces published source
/// material that the chat-tuned defaults over-block. Google's non-configurable
/// prohibited-use filters are unaffected.
pub(crate) fn permissive_safety_settings() -> Vec<SafetySetting> {
    [
        HarmCategory::Harassment,
        HarmCategory::HateSpeech,
        HarmCategory::SexuallyExplicit,
        HarmCategory::DangerousContent,
        HarmCategory::CivicIntegrity,
    ]
    .into_iter()
    .map(|category| SafetySetting {
        category,
        threshold: HarmBlockThreshold::BlockNone,
    })
    .collect()
}

/// Gemini rejects the shared schema's `additionalProperties` with HTTP 400, so
/// it gets a stripped variant. The `required` arrays are relaxed too: Gemini
/// omits non-required properties that have no content, which drops empty
/// inflection slots, absent notes, and punctuation's whole grammar block.
pub(crate) fn gemini_paragraph_schema() -> Value {
    let mut s = paragraph_translation_schema();
    strip_additional_properties(&mut s);
    relax_required_for_gemini(&mut s);
    add_property_ordering_for_gemini(&mut s);
    s
}

/// Keep only the anchors (`o` per word, `lf`/`lt`/`pos` per grammar) required
/// so Gemini omits empty fields. `p` is optional as well — only punctuation
/// emits it, and the importer's serde `default` reads absence as false.
fn relax_required_for_gemini(schema: &mut Value) {
    let word = &mut schema["properties"]["s"]["items"]["properties"]["wl"]["items"];
    word["required"] = serde_json::json!(["o"]);
    word["properties"]["g"]["required"] = serde_json::json!(["lf", "lt", "pos"]);
}

/// Pin the decoder's key order. Without it the decoder follows serde_json's
/// alphabetical map order rather than the order the prompt legend teaches,
/// which Google documents as a source of structured-output unreliability (here:
/// repetition loops that stream until a timeout). `o` leads so every word item
/// opens by anchoring to a fresh source token, and `p` follows so punctuation
/// closes after two keys. OpenAI strict mode rejects the keyword.
fn add_property_ordering_for_gemini(schema: &mut Value) {
    schema["propertyOrdering"] = serde_json::json!(["s"]);
    let sentence = &mut schema["properties"]["s"]["items"];
    sentence["propertyOrdering"] = serde_json::json!(["wl", "ft"]);
    let word = &mut sentence["properties"]["wl"]["items"];
    word["propertyOrdering"] = serde_json::json!(["o", "p", "t", "n", "g"]);
    word["properties"]["g"]["propertyOrdering"] =
        serde_json::json!(["lf", "lt", "pos", "pl", "pe", "te", "ca", "ot"]);
}

pub struct GeminiTranslator {
    cache: Arc<TranslationsCache>,
    context_provider: Arc<dyn ChapterContextProvider>,
    prompt_cache: Arc<GeminiPromptCache>,
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
        prompt_cache: Arc<GeminiPromptCache>,
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
            prompt_cache,
            client,
            schema: Arc::new(gemini_paragraph_schema()),
            model,
            translation_model,
            from: *from,
            to: *to,
        })
    }

    fn cache_key(&self, book_id: Uuid, chapter_id: usize) -> CacheKey {
        CacheKey {
            model: self.translation_model,
            from: self.from,
            to: self.to,
            book_id,
            chapter_id,
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

    /// One full attempt: build or reuse the chapter cache, request, drain,
    /// decode. Callers wrap it to evict and retry a dead server-side cache.
    async fn attempt_translation(
        &self,
        paragraph: &str,
        book_id: Uuid,
        chapter_id: usize,
        prior_summaries: String,
        chapter_text: String,
        callback: Option<&ProgressCallback>,
    ) -> anyhow::Result<ParagraphTranslation> {
        let from = self.from;
        let to = self.to;
        let key = self.cache_key(book_id, chapter_id);

        let cache_handle: Arc<CachedContentHandle> = timeout(
            CACHE_CREATE_TIMEOUT,
            self.prompt_cache
                .get_or_create(&self.client, key.clone(), || {
                    let reference = build_reference_material(&prior_summaries, &chapter_text);
                    CacheContent {
                        system_instruction: Self::get_prompt(from.to_name(), to.to_name()),
                        user_reference_material: reference,
                    }
                }),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Gemini cache creation timed out"))??;

        let user_message = format!("Translate this paragraph: {paragraph}");
        let mut stream = timeout(
            TRANSLATION_REQUEST_TIMEOUT,
            self.client
                .generate_content()
                .with_cached_content(&cache_handle)
                .with_user_message(user_message)
                .with_response_mime_type("application/json")
                .with_response_schema((*self.schema).clone())
                .with_thinking_config(self.thinking_config())
                .with_safety_settings(permissive_safety_settings())
                .execute_stream(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Gemini request timed out"))??;

        // Kept outside the timed future: a timeout drops the future, but the
        // diagnostics below still need what arrived and what the server said.
        let mut accumulator = StreamChunkAccumulator::new("Gemini");
        let mut last_finish_reason: Option<FinishReason> = None;
        let mut last_usage: Option<UsageMetadata> = None;
        let started = Instant::now();

        let drain = async {
            loop {
                let next = timeout(TRANSLATION_STREAM_IDLE_TIMEOUT, stream.try_next())
                    .await
                    .map_err(|_| anyhow::anyhow!("Gemini stream timed out"))?;
                let item = match next {
                    Ok(Some(response)) => {
                        if let Some(reason) = response
                            .candidates
                            .first()
                            .and_then(|c| c.finish_reason.clone())
                        {
                            last_finish_reason = Some(reason);
                        }
                        if let Some(usage) = response.usage_metadata.clone() {
                            last_usage = Some(usage);
                        }
                        Ok(Some(response.text()))
                    }
                    Ok(None) => Ok(None),
                    Err(err) => Err(err.into()),
                };
                if !accumulator.handle_result(item, callback)? {
                    break;
                }
            }
            anyhow::Ok(())
        };

        let drained = timeout(total_stream_timeout(paragraph.len()), drain)
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("Gemini total stream timeout")));

        if let Err(err) = drained {
            warn!(
                "Gemini stream aborted after {:.1?}: {err} (paragraph {} chars, accumulated {} chars, finish_reason {:?}, usage {:?})",
                started.elapsed(),
                paragraph.len(),
                accumulator.len(),
                last_finish_reason,
                last_usage,
            );
            if !accumulator.is_empty() {
                debug!("Gemini aborted stream tail: …{}", accumulator.tail(300));
            }
            return Err(err);
        }

        let full_content = accumulator.finish()?;

        // MAX_TOKENS truncates the JSON. Bail before serde, whose error is
        // classified permanent, since a decoding runaway is worth retrying.
        if last_finish_reason == Some(FinishReason::MaxTokens) {
            warn!(
                "Gemini hit max output tokens after {:.1?} (paragraph {} chars, accumulated {} chars, usage {:?})",
                started.elapsed(),
                paragraph.len(),
                full_content.len(),
                last_usage,
            );
            anyhow::bail!(
                "Gemini hit max output tokens ({} chars accumulated)",
                full_content.len()
            );
        }

        let usage = last_usage.as_ref();
        info!(
            "Gemini stream finished in {:.1?}: finish_reason {:?}, tokens prompt={:?} cached={:?} thoughts={:?} output={:?} total={:?}",
            started.elapsed(),
            last_finish_reason,
            usage.and_then(|u| u.prompt_token_count),
            usage.and_then(|u| u.cached_content_token_count),
            usage.and_then(|u| u.thoughts_token_count),
            usage.and_then(|u| u.candidates_token_count),
            usage.and_then(|u| u.total_token_count),
        );

        let mut translation: ParagraphTranslation = serde_json::from_str(&full_content)?;
        translation.normalize_html_entities();
        translation.total_tokens = usage.and_then(|u| u.total_token_count).map(|c| c as u64);
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
        let book_id = ctx.book_id;
        let chapter_id = ctx.chapter_id;
        let cb = ctx.callback.as_deref();

        // The UI gates translate on the same predicate, so this is normally
        // instant. Errors propagate; there is no summary-free fallback.
        self.context_provider
            .wait_ready(book_id, chapter_id)
            .await?;
        let prior_summaries = self
            .context_provider
            .prior_summaries(book_id, chapter_id)
            .await
            .unwrap_or_default();
        let chapter_text = self
            .context_provider
            .chapter_text(book_id, chapter_id)
            .await
            .unwrap_or_default();

        let first = self
            .attempt_translation(
                paragraph,
                book_id,
                chapter_id,
                prior_summaries.clone(),
                chapter_text.clone(),
                cb,
            )
            .await;
        let mut translation = match first {
            Ok(t) => t,
            Err(err) if is_cache_missing_error(&err) => {
                warn!(
                    "Gemini cache appears expired/missing; evicting and retrying. ({err})"
                );
                self.prompt_cache
                    .evict(&self.cache_key(book_id, chapter_id))
                    .await;
                self.attempt_translation(
                    paragraph,
                    book_id,
                    chapter_id,
                    prior_summaries,
                    chapter_text,
                    cb,
                )
                .await?
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_override_parses() {
        assert!(gemini_base_url_override(None).is_none());
        assert!(gemini_base_url_override(Some(String::new())).is_none());
        let url = gemini_base_url_override(Some("http://127.0.0.1:4001/v1beta/".into())).unwrap();
        assert_eq!(url.as_str(), "http://127.0.0.1:4001/v1beta/");
    }

    fn word_node(schema: &Value) -> &Value {
        &schema["properties"]["s"]["items"]["properties"]["wl"]["items"]
    }

    #[test]
    fn gemini_schema_relaxes_required_and_strips_additional_properties() {
        let schema = gemini_paragraph_schema();

        assert!(
            !serde_json::to_string(&schema)
                .unwrap()
                .contains("additionalProperties")
        );

        let word = word_node(&schema);
        assert_eq!(word["required"], serde_json::json!(["o"]));
        assert_eq!(
            word["properties"]["g"]["required"],
            serde_json::json!(["lf", "lt", "pos"])
        );
    }

    #[test]
    fn gemini_schema_pins_property_ordering() {
        let schema = gemini_paragraph_schema();

        assert_eq!(schema["propertyOrdering"], serde_json::json!(["s"]));
        assert_eq!(
            schema["properties"]["s"]["items"]["propertyOrdering"],
            serde_json::json!(["wl", "ft"])
        );
        let word = word_node(&schema);
        assert_eq!(
            word["propertyOrdering"],
            serde_json::json!(["o", "p", "t", "n", "g"])
        );
        assert_eq!(
            word["properties"]["g"]["propertyOrdering"],
            serde_json::json!(["lf", "lt", "pos", "pl", "pe", "te", "ca", "ot"])
        );

        // Gemini rejects an ordering array that isn't exactly its node's keys.
        for (node, ordering) in [
            (&schema, &schema["propertyOrdering"]),
            (
                &schema["properties"]["s"]["items"],
                &schema["properties"]["s"]["items"]["propertyOrdering"],
            ),
            (word, &word["propertyOrdering"]),
            (
                &word["properties"]["g"],
                &word["properties"]["g"]["propertyOrdering"],
            ),
        ] {
            let mut ordered: Vec<&str> = ordering
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            let mut declared: Vec<&str> = node["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            ordered.sort_unstable();
            declared.sort_unstable();
            assert_eq!(ordered, declared);
        }
    }

    #[test]
    fn openai_base_schema_keeps_everything_required() {
        // The OpenAI-strict base must keep every key required and stay free of
        // the Gemini-only propertyOrdering keyword.
        let schema = paragraph_translation_schema();
        let serialized = serde_json::to_string(&schema).unwrap();
        assert!(serialized.contains("additionalProperties"));
        assert!(!serialized.contains("propertyOrdering"));

        let word = word_node(&schema);
        assert_eq!(
            word["required"],
            serde_json::json!(["o", "t", "n", "g", "p"])
        );
        assert_eq!(
            word["properties"]["g"]["required"],
            serde_json::json!(["pos", "lf", "lt", "pl", "pe", "te", "ca", "ot"])
        );
    }
}
