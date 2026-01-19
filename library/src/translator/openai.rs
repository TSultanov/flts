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
use tokio::sync::Mutex;

use crate::{
    book::translation_import::ParagraphTranslation,
    cache::TranslationsCache,
    translator::{TranslationErrors, TranslationModel, Translator},
};

pub struct OpenAITranslator {
    cache: Arc<Mutex<TranslationsCache>>,
    client: Client<OpenAIConfig>,
    schema: Value,
    model: String,
    translation_model: TranslationModel,
    from: Language,
    to: Language,
}

impl OpenAITranslator {
    pub fn create(
        cache: Arc<Mutex<TranslationsCache>>,
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
            schema,
            model: model.to_string(),
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
        callback: Option<Box<dyn Fn(usize) + Send + Sync>>,
    ) -> anyhow::Result<ParagraphTranslation> {
        if use_cache
            && let Some(cached_result) = self
                .cache
                .lock()
                .await
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
            .model(self.model.clone())
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
                    schema: Some(self.schema.clone()),
                    strict: Some(true),
                },
            })
            .stream(true)
            .build()?;

        let mut stream = self.client.chat().create_stream(request).await?;
        let mut full_content = String::new();

        while let Some(result) = stream.next().await {
            match result {
                Ok(response) => {
                    if let Some(choice) = response.choices.first() {
                        if let Some(delta) = &choice.delta.content {
                            full_content.push_str(delta);
                            if let Some(cb) = &callback {
                                cb(full_content.len());
                            }
                        }
                    }
                }
                Err(err) => {
                    log::warn!("Error in OpenAI stream: {}", err);
                }
            }
        }

        if full_content.is_empty() {
            anyhow::bail!("OpenAI returned empty content");
        }

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
            .lock()
            .await
            .set(&self.from, &self.to, paragraph, &translation);

        Ok(translation)
    }
}
