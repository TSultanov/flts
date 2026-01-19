use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures::TryStreamExt;
use gemini_rust::{Gemini, Model, ThinkingConfig};
use isolang::Language;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{
    book::translation_import::ParagraphTranslation,
    cache::TranslationsCache,
    translator::{TranslationErrors, TranslationModel, Translator},
};

pub struct GeminiTranslator {
    cache: Arc<Mutex<TranslationsCache>>,
    client: Gemini,
    schema: Value,
    model: Model,
    translation_model: TranslationModel,
    from: Language,
    to: Language,
}

impl GeminiTranslator {
    pub fn create(
        cache: Arc<Mutex<TranslationsCache>>,
        translation_model: TranslationModel,
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

        let model = match translation_model {
            TranslationModel::Gemini25Flash => Model::Gemini25Flash,
            TranslationModel::Gemini25Pro => Model::Gemini25Pro,
            TranslationModel::Gemini25FlashLight => Model::Gemini25FlashLite,
            TranslationModel::Gemini3Pro => {
                Model::Custom("models/gemini-3-pro-preview".to_string())
            }
            TranslationModel::Gemini3Flash => {
                Model::Custom("models/gemini-3-flash-preview".to_string())
            }
            _ => Err(TranslationErrors::UnknownModel)?,
        };

        let client = Gemini::with_model(api_key, model.clone())?;

        Ok(Self {
            cache,
            schema,
            client,
            model,
            translation_model,
            from: *from,
            to: *to,
        })
    }
}

#[async_trait]
impl Translator for GeminiTranslator {
    fn get_model(&self) -> super::TranslationModel {
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

        let thinking_config = match &self.model {
            Model::Gemini25Flash => ThinkingConfig {
                thinking_budget: Some(0),
                include_thoughts: Some(false),
            },
            _ => ThinkingConfig {
                thinking_budget: None,
                include_thoughts: Some(false),
            },
        };

        let mut stream = self
            .client
            .generate_content()
            .with_system_prompt(Self::get_prompt(self.from.to_name(), self.to.to_name()))
            .with_user_message(paragraph)
            .with_response_mime_type("application/json")
            .with_response_schema(self.schema.clone())
            .with_thinking_config(thinking_config)
            .execute_stream()
            .await?;

        let mut full_content = String::new();

        while let Some(response) = stream.try_next().await? {
            let text = response.text();
            if !text.is_empty() {
                full_content.push_str(&text);
                if let Some(cb) = &callback {
                    cb(full_content.len());
                }
            }
        }

        if full_content.is_empty() {
            anyhow::bail!("Gemini returned empty content");
        }

        let mut translation: ParagraphTranslation = serde_json::from_str(&full_content)?;

        // Usage metadata might be in the last chunk?
        // We'll skip token count for consistent streaming support for now.

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
