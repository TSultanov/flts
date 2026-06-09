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

/// The cached-content POST runs before `TRANSLATION_REQUEST_TIMEOUT` wraps
/// anything, and gemini-rust's reqwest client has no timeout of its own — an
/// unbounded await here hangs every paragraph of the chapter (they share the
/// cache init future) with no error and no requeue.
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
        _ => Err(TranslationErrors::UnknownModel)?,
    })
}

pub(crate) fn gemini_client(api_key: String, model: Model) -> anyhow::Result<Gemini> {
    Ok(Gemini::with_model(api_key, model)?)
}

/// Permissive safety_settings for every Gemini request the project
/// makes. Book translation legitimately reproduces content (drugs,
/// violence, prejudice, sexuality) from published source material; the
/// chat-assistant-tuned defaults over-block on this workload. Does NOT
/// affect Google's non-configurable prohibited-use-policy filters.
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

/// The shared paragraph schema is OpenAI-strict (uses `additionalProperties: false`).
/// Gemini rejects that key with HTTP 400, so we hand it a stripped variant.
///
/// We also relax the `required` arrays for Gemini: unlike OpenAI strict mode,
/// Gemini omits a non-required property when it has no content. Dropping the
/// optional grammar/translation fields from `required` lets Gemini skip empty
/// inflection slots, absent notes, and the entire grammar block for punctuation
/// — which is where most of the per-word output scaffolding was being spent.
pub(crate) fn gemini_paragraph_schema() -> Value {
    let mut s = paragraph_translation_schema();
    strip_additional_properties(&mut s);
    relax_required_for_gemini(&mut s);
    add_property_ordering_for_gemini(&mut s);
    s
}

/// Narrow the `required` arrays so Gemini emits only the fields it has content
/// for. Keeps the always-present anchors (`o` per word, `lf`/`lt`/`pos` per
/// grammar) required; everything else becomes optional and is omitted when
/// empty. `p` is optional too: only punctuation tokens emit it (as `true`),
/// so every normal word saves the `"p":false` scaffolding — the importer's
/// serde `default` reads absence as false.
fn relax_required_for_gemini(schema: &mut Value) {
    let word = &mut schema["properties"]["s"]["items"]["properties"]["wl"]["items"];
    word["required"] = serde_json::json!(["o"]);
    word["properties"]["g"]["required"] = serde_json::json!(["lf", "lt", "pos"]);
}

/// Pin the key order Gemini's constrained decoder emits. Without
/// `propertyOrdering` the decoder follows the schema's own key order, and
/// serde_json's `json!` maps are alphabetical — NOT the order the prompt
/// legend teaches; Google documents `propertyOrdering` as the fix for the
/// resulting structured-output unreliability (we see it as repetition
/// loops that stream until a timeout). `o` goes first so every word item
/// must open by anchoring to a fresh source token — the strongest
/// anti-repetition anchor — and `p` second so punctuation items close
/// after two keys. Gemini-only: OpenAI strict mode rejects the keyword.
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

    /// One full attempt: build (or reuse) the per-chapter cache, send the
    /// per-paragraph request, drain the stream, decode. Callers wrap this
    /// so a missing / expired server-side cache can be evicted and retried.
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

        // The accumulator and stream metadata live OUTSIDE the timed future:
        // when a timeout fires the future is dropped, but the diagnostics
        // below still need to report how much arrived and what the server
        // last said (finish reason / usage), otherwise aborts are opaque.
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

        // A MAX_TOKENS finish means the JSON is truncated. Bail before the
        // serde parse: a serde error is classified permanent and would kill
        // the requeue path, but hitting the server's output cap is a
        // constrained-decoding runaway worth retrying.
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

        // Block until the prerequisite per-chapter summaries are ready.
        // The UI gates translate buttons on the same predicate, so this
        // is normally near-instant. Any actual error propagates — there
        // is no "translate without summaries" fallback any more.
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

    fn word_node(schema: &Value) -> &Value {
        &schema["properties"]["s"]["items"]["properties"]["wl"]["items"]
    }

    #[test]
    fn gemini_schema_relaxes_required_and_strips_additional_properties() {
        let schema = gemini_paragraph_schema();

        // additionalProperties stripped recursively (Gemini rejects it).
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

        // Every ordering array must list exactly the properties of its node,
        // or Gemini rejects the schema.
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
        // The OpenAI-strict base must keep all keys required, retain
        // additionalProperties:false, and stay free of the Gemini-only
        // propertyOrdering keyword (OpenAI strict mode rejects it).
        let schema = paragraph_translation_schema();
        let serialized = serde_json::to_string(&schema).unwrap();
        assert!(serialized.contains("additionalProperties"));
        assert!(!serialized.contains("propertyOrdering"));

        let word = word_node(&schema);
        assert_eq!(word["required"], serde_json::json!(["o", "t", "n", "g", "p"]));
        assert_eq!(
            word["properties"]["g"]["required"],
            serde_json::json!(["pos", "lf", "lt", "pl", "pe", "te", "ca", "ot"])
        );
    }
}
