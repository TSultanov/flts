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
use futures::StreamExt;
use isolang::Language;
use serde_json::{Value, json};
use tokio::time::timeout;

use crate::{
    book::translation_import::ParagraphTranslation,
    cache::TranslationsCache,
    translator::{ProgressCallback, TranslationErrors, TranslationModel, Translator},
};

use super::{
    StreamChunkAccumulator, TRANSLATION_REQUEST_TIMEOUT, TRANSLATION_STREAM_IDLE_TIMEOUT,
    total_stream_timeout,
};

pub struct OpenAITranslator {
    cache: Arc<TranslationsCache>,
    client: Client<OpenAIConfig>,
    schema: Arc<Value>,
    model: Arc<str>,
    translation_model: TranslationModel,
    from: Language,
    to: Language,
}

impl OpenAITranslator {
    pub fn create(
        cache: Arc<TranslationsCache>,
        translation_model: TranslationModel,
        api_key: String,
        from: &Language,
        to: &Language,
    ) -> anyhow::Result<Self> {
        let schema = json!(
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "sentences": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "words": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "additionalProperties": false,
                                        "properties": {
                                            "original": {
                                                "type": "string",
                                                "description": "Original word"
                                            },
                                            "contextualTranslations": {
                                                "type": "array",
                                                "items": { "type": "string" },
                                                "description": "Translation variants which are suitable for the current context"
                                            },
                                            "note": {
                                                "type": "string",
                                                "description": "Note about the translation, if necessary for understanding"
                                            },
                                            "isPunctuation": {
                                                "type": "boolean"
                                            },
                                            "grammar": {
                                                "type": "object",
                                                "additionalProperties": false,
                                                "properties": {
                                                    "originalInitialForm": { "type": "string" },
                                                    "targetInitialForm": { "type": "string" },
                                                    "partOfSpeech": { "type": "string" },
                                                    "plurality": { "type": "string" },
                                                    "person": { "type": "string" },
                                                    "tense": { "type": "string" },
                                                    "case": { "type": "string" },
                                                    "other": { "type": "string" }
                                                },
                                                   "required": [
                                                       "partOfSpeech",
                                                       "originalInitialForm",
                                                       "targetInitialForm",
                                                       "plurality",
                                                       "person",
                                                       "tense",
                                                       "case",
                                                       "other"
                                                   ]
                                            }
                                        },
                                        "required": [
                                            "original",
                                            "contextualTranslations",
                                            "note",
                                            "grammar",
                                            "isPunctuation"
                                        ]
                                    }
                                },
                                "fullTranslation": {
                                    "type": "string",
                                    "description": "Full translation of the sentence"
                                }
                            },
                            "required": [
                                "words",
                                "fullTranslation"
                            ]
                        }
                    },
                    "sourceLanguage": { "type": "string" },
                    "targetLanguage": { "type": "string" }
                },
                "required": [
                    "sentences",
                    "sourceLanguage",
                    "targetLanguage"
                ]
            }
        );

        let model = match translation_model {
            TranslationModel::OpenAIGpt52 => "gpt-5.2",
            TranslationModel::OpenAIGpt52Pro => "gpt-5.2-pro",
            TranslationModel::OpenAIGpt5Mini => "gpt-5-mini",
            TranslationModel::OpenAIGpt5Nano => "gpt-5-nano",
            _ => Err(TranslationErrors::UnknownModel)?,
        };

        let config = OpenAIConfig::new().with_api_key(api_key);
        let client = Client::with_config(config);

        Ok(Self {
            cache,
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
        paragraph: &str,
        use_cache: bool,
        callback: Option<Box<ProgressCallback>>,
    ) -> anyhow::Result<ParagraphTranslation> {
        if use_cache
            && let Some(cached_result) = self
                .cache
                .get(&self.from, &self.to, paragraph)
                .await
                .ok()
                .flatten()
        {
            return Ok(cached_result);
        }

        let system_prompt = format!(
            "{}\n\nReturn ONLY a single JSON object that matches the requested schema. Do not wrap it in markdown.",
            Self::get_prompt(self.from.to_name(), self.to.to_name())
        );

        let request = CreateChatCompletionRequestArgs::default()
            .model(self.model.as_ref())
            .messages([
                ChatCompletionRequestMessage::System(
                    ChatCompletionRequestSystemMessageArgs::default()
                        .content(system_prompt)
                        .build()?,
                ),
                ChatCompletionRequestMessage::User(
                    ChatCompletionRequestUserMessageArgs::default()
                        .content(paragraph)
                        .build()?,
                ),
            ])
            .response_format(ResponseFormat::JsonSchema {
                json_schema: ResponseFormatJsonSchema {
                    description: Some("Paragraph translation".to_string()),
                    name: "paragraph_translation".to_string(),
                    schema: Some((*self.schema).clone()),
                    strict: Some(true),
                },
            })
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
