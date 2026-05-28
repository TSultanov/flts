use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs, ResponseFormat,
    ResponseFormatJsonSchema,
};
use async_openai::{Client, config::OpenAIConfig};
use async_trait::async_trait;
use futures_util::StreamExt;
use isolang::Language;
use serde_json::Value;
use tokio::time::timeout;

use crate::{
    book::translation_import::ParagraphTranslation,
    cache::TranslationsCache,
    translator::{
        ChapterContextProvider, TranslationContext, TranslationErrors, TranslationModel,
        TranslationProvider, Translator, paragraph_translation_schema,
    },
};

use super::{
    StreamChunkAccumulator, TRANSLATION_REQUEST_TIMEOUT, TRANSLATION_STREAM_IDLE_TIMEOUT,
    total_stream_timeout,
};

pub struct OpenAITranslator {
    cache: Arc<TranslationsCache>,
    context_provider: Arc<dyn ChapterContextProvider>,
    client: Client<OpenAIConfig>,
    schema: Arc<Value>,
    model: Arc<str>,
    translation_model: TranslationModel,
    from: Language,
    to: Language,
}

pub(crate) const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";

pub(crate) fn openai_model_name(m: TranslationModel) -> anyhow::Result<&'static str> {
    Ok(match m {
        TranslationModel::OpenAIGpt52 => "gpt-5.2",
        TranslationModel::OpenAIGpt52Pro => "gpt-5.2-pro",
        TranslationModel::OpenAIGpt5Mini => "gpt-5-mini",
        TranslationModel::OpenAIGpt5Nano => "gpt-5-nano",
        TranslationModel::OpenAIGpt54 => "gpt-5.4",
        TranslationModel::OpenAIGpt54Mini => "gpt-5.4-mini",
        TranslationModel::DeepSeekV4Flash => "deepseek-v4-flash",
        TranslationModel::DeepSeekV4Pro => "deepseek-v4-pro",
        _ => Err(TranslationErrors::UnknownModel)?,
    })
}

pub(crate) fn openai_client(api_key: String, base_url: Option<&str>) -> Client<OpenAIConfig> {
    let mut config = OpenAIConfig::new().with_api_key(api_key);
    if let Some(url) = base_url {
        config = config.with_api_base(url);
    }
    Client::with_config(config)
}

/// Returns the base URL override for OpenAI-compatible providers. `None`
/// means use async_openai's default (api.openai.com).
pub(crate) fn openai_compat_base_url(
    provider: crate::translator::TranslationProvider,
) -> Option<&'static str> {
    match provider {
        crate::translator::TranslationProvider::Deepseek => Some(DEEPSEEK_BASE_URL),
        _ => None,
    }
}

impl OpenAITranslator {
    pub fn create(
        cache: Arc<TranslationsCache>,
        context_provider: Arc<dyn ChapterContextProvider>,
        translation_model: TranslationModel,
        api_key: String,
        from: &Language,
        to: &Language,
    ) -> anyhow::Result<Self> {
        let schema = paragraph_translation_schema();
        let model = openai_model_name(translation_model)?;
        let base_url = translation_model
            .provider()
            .and_then(openai_compat_base_url);
        let client = openai_client(api_key, base_url);

        Ok(Self {
            cache,
            context_provider,
            client,
            schema: Arc::new(schema),
            model: Arc::from(model),
            translation_model,
            from: *from,
            to: *to,
        })
    }
}

#[async_trait]
impl Translator for OpenAITranslator {
    fn get_model(&self) -> TranslationModel {
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
        let callback = ctx.callback;
        let is_deepseek = matches!(
            self.translation_model.provider(),
            Some(TranslationProvider::Deepseek)
        );
        let mut system_prompt = format!(
            "{}\n\nReturn ONLY a single JSON object that matches the requested schema. Do not wrap it in markdown.",
            Self::get_prompt(self.from.to_name(), self.to.to_name())
        );
        // DeepSeek's JSON mode does not enforce a schema server-side — it only
        // guarantees valid JSON — so we inline the schema in the prompt so the
        // model has the target shape to fill in.
        if is_deepseek {
            if let Ok(schema_text) = serde_json::to_string_pretty(&*self.schema) {
                system_prompt.push_str("\n\nJSON schema for the response:\n");
                system_prompt.push_str(&schema_text);
            }
        }

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

        let mut messages: Vec<ChatCompletionRequestMessage> = Vec::with_capacity(3);
        messages.push(ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessageArgs::default()
                .content(system_prompt)
                .build()?,
        ));
        if let Some(reference) = build_reference_material(&prior_summaries, &chapter_text) {
            messages.push(ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessageArgs::default()
                    .content(reference)
                    .build()?,
            ));
        }
        messages.push(ChatCompletionRequestMessage::User(
            ChatCompletionRequestUserMessageArgs::default()
                .content(format!("Translate this paragraph: {paragraph}"))
                .build()?,
        ));

        let response_format = if is_deepseek {
            ResponseFormat::JsonObject
        } else {
            ResponseFormat::JsonSchema {
                json_schema: ResponseFormatJsonSchema {
                    description: Some("Paragraph translation".to_string()),
                    name: "paragraph_translation".to_string(),
                    schema: Some((*self.schema).clone()),
                    strict: Some(true),
                },
            }
        };

        let request = CreateChatCompletionRequestArgs::default()
            .model(self.model.as_ref())
            .messages(messages)
            .response_format(response_format)
            .stream(true)
            .build()?;

        let mut stream = timeout(
            TRANSLATION_REQUEST_TIMEOUT,
            self.client.chat().create_stream(request),
        )
        .await
        .map_err(|_| anyhow::anyhow!("OpenAI request timed out"))??;
        let mut accumulator = StreamChunkAccumulator::new("OpenAI");

        let full_content = timeout(total_stream_timeout(paragraph.len()), async {
            loop {
                let next = timeout(TRANSLATION_STREAM_IDLE_TIMEOUT, stream.next())
                    .await
                    .map_err(|_| anyhow::anyhow!("OpenAI stream timed out"))?;
                let should_continue = accumulator.handle_result(
                    match next {
                        Some(Ok(response)) => Ok(Some(
                            response
                                .choices
                                .first()
                                .and_then(|choice| choice.delta.content.clone())
                                .unwrap_or_default(),
                        )),
                        Some(Err(err)) => Err(err.into()),
                        None => Ok(None),
                    },
                    callback.as_deref(),
                )?;
                if !should_continue {
                    break;
                }
            }
            accumulator.finish()
        })
        .await
        .map_err(|_| anyhow::anyhow!("OpenAI total stream timeout"))??;

        let mut translation: ParagraphTranslation = serde_json::from_str(&full_content)?;
        translation.normalize_html_entities();

        // Note: Usage data might not be available in stream chunks easily or at all in some API versions for stream.
        // We'll skip setting usage for now or check if the final chunk has usage?
        // async-openai stream items might have usage in the last chunk?
        // Checked docs: `ChatCompletionChunk` has `usage` field optionally?
        // If not, we lose token count. That's acceptable for now.
        // translation.total_tokens = ...;

        let now = SystemTime::now();
        let duration_since_epoch = now.duration_since(UNIX_EPOCH)?;
        translation.timestamp = duration_since_epoch.as_secs();

        self.cache
            .set(&self.from, &self.to, paragraph, &translation);

        Ok(translation)
    }
}

/// Same composition as the Gemini reference material — kept byte-identical
/// across consecutive paragraphs in the same chapter so OpenAI's implicit
/// prefix caching can match.
fn build_reference_material(prior_summaries: &str, chapter_text: &str) -> Option<String> {
    if prior_summaries.is_empty() && chapter_text.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(prior_summaries.len() + chapter_text.len() + 256);
    if !prior_summaries.is_empty() {
        out.push_str("Summaries of prior chapters in this book (for cross-chapter context only — do not translate them):\n\n");
        out.push_str(prior_summaries);
        out.push_str("\n\n");
    }
    if !chapter_text.is_empty() {
        out.push_str("Full text of the current chapter (use as surrounding context; the specific paragraph to translate will follow in a separate message):\n\n");
        out.push_str(chapter_text);
    }
    Some(out)
}
