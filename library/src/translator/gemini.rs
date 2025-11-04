use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use gemini_rust::{Gemini, Model, ThinkingConfig};
use isolang::Language;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{
    book::translation_import::ParagraphTranslation, cache::TranslationsCache,
    translator::Translator,
};

pub struct GeminiTranslator {
    cache: Arc<Mutex<TranslationsCache>>,
    client: Gemini,
    schema: Value,
    model: Model,
    from: Language,
    to: Language,
}

impl GeminiTranslator {
    pub fn create(
        cache: Arc<Mutex<TranslationsCache>>,
        model: Model,
        api_key: String,
        from: &Language,
        to: &Language,
    ) -> anyhow::Result<GeminiTranslator> {
        let schema = json!(
            {
                "type": "object",
                "properties": {
                    "sentences": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "words": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "original": {
                                                "type": "string",
                                                "description": "Original word",
                                            },
                                            "contextualTranslations": {
                                                "type": "array",
                                                "items": {
                                                    "type": "string"
                                                },
                                                "description": "Translation variants which are suitable for the current context",
                                            },
                                            "note": {
                                                "type": "string",
                                                "description": "Note about the translation, if necessary for understanding",
                                            },
                                            "isPunctuation": {
                                                "type": "boolean"
                                            },
                                            "grammar": {
                                                "type": "object",
                                                "properties": {
                                                    "originalInitialForm": {
                                                        "type": "string",
                                                        "description": "Original word in its initial (dictionary) form",
                                                    },
                                                    "targetInitialForm": {
                                                        "type": "string",
                                                        "description": "Translated word in its initial (dictionary) form",
                                                    },
                                                    "partOfSpeech": {
                                                        "type": "string",
                                                        "description": "Which part of speech the original word is",
                                                    },
                                                    "plurality": {
                                                        "type": "string",
                                                        "description": "Plurality of the original word, if applicable",
                                                    },
                                                    "person": {
                                                        "type": "string",
                                                        "description": "Person of the original word, if applicable",
                                                    },
                                                    "tense": {
                                                        "type": "string",
                                                        "description": "Tense of the original word, if applicable",
                                                    },
                                                    "case": {
                                                        "type": "string",
                                                        "description": "What case the original word is in, if applicable",
                                                    },
                                                    "other": {
                                                        "type": "string",
                                                        "description": "Other grammatical information about the original word, if not described by other fields",
                                                    }
                                                },
                                                "required": [
                                                    "partOfSpeech",
                                                    "originalInitialForm",
                                                    "targetInitialForm"
                                                ]
                                            }
                                        },
                                        "required": [
                                            "original",
                                            "contextualTranslations",
                                            "grammar",
                                            "isPunctuation"
                                        ]
                                    }
                                },
                                "fullTranslation": {
                                    "type": "string",
                                    "description": "Full translation of the sentence",
                                }
                            },
                            "required": [
                                "words",
                                "fullTranslation"
                            ]
                        }
                    },
                    "sourceLanguage": {
                        "type": "string"
                    },
                    "targetLanguage": {
                        "type": "string"
                    }
                },
                "required": [
                    "sentences",
                    "sourceLanguage",
                    "targetLanguage"
                ]
            }
        );

        let client = Gemini::with_model(api_key, model.clone())?;

        Ok(Self {
            cache,
            schema,
            client,
            model,
            from: *from,
            to: *to,
        })
    }
}

impl Translator for GeminiTranslator {
    async fn get_translation(&self, paragraph: &str) -> anyhow::Result<ParagraphTranslation> {
        if let Some(cached_result) = self
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

        let thinking_config = match &self.model {
            Model::Gemini25Flash | Model::Gemini25FlashLite => ThinkingConfig {
                thinking_budget: Some(0),
                include_thoughts: Some(false),
            },
            _ => ThinkingConfig {
                thinking_budget: None,
                include_thoughts: None,
            },
        };

        let result = self
            .client
            .generate_content()
            .with_system_prompt(Self::get_prompt(self.from.to_name(), self.to.to_name()))
            .with_user_message(paragraph)
            .with_response_mime_type("application/json")
            .with_response_schema(self.schema.clone())
            .with_thinking_config(thinking_config)
            .execute()
            .await?;

        let mut result: ParagraphTranslation = serde_json::from_str(&result.text())?;

        let now = SystemTime::now();
        let duration_since_epoch = now.duration_since(UNIX_EPOCH)?;
        result.timestamp = duration_since_epoch.as_secs();

        self.cache
            .lock()
            .await
            .set(&self.from, &self.to, paragraph, &result);

        Ok(result)
    }
}
