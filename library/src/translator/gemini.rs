use gemini_rust::{Gemini};
use serde_json::{json, Value};

use crate::{book::translation_import::ParagraphTranslation, translator::Translator};

pub struct GeminiTranslator {
    client: Gemini,
    schema: Value,
    to: String,
}

impl GeminiTranslator {
    pub fn create(api_key: String, to: String) -> anyhow::Result<GeminiTranslator> {
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
                                                "type": "string"
                                            },
                                            "contextualTranslations": {
                                                "type": "array",
                                                "items": {
                                                    "type": "string"
                                                }
                                            },
                                            "note": {
                                                "type": "string"
                                            },
                                            "isPunctuation": {
                                                "type": "boolean"
                                            },
                                            "grammar": {
                                                "type": "object",
                                                "properties": {
                                                    "originalInitialForm": {
                                                        "type": "string"
                                                    },
                                                    "targetInitialForm": {
                                                        "type": "string"
                                                    },
                                                    "partOfSpeech": {
                                                        "type": "string"
                                                    },
                                                    "plurality": {
                                                        "type": "string"
                                                    },
                                                    "person": {
                                                        "type": "string"
                                                    },
                                                    "tense": {
                                                        "type": "string"
                                                    },
                                                    "case": {
                                                        "type": "string"
                                                    },
                                                    "other": {
                                                        "type": "string"
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
                                            "translations",
                                            "note",
                                            "grammar",
                                            "isPunctuation"
                                        ]
                                    }
                                },
                                "fullTranslation": {
                                    "type": "string"
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

        let client = Gemini::new(api_key)?;

        Ok(Self {
            schema,
            client,
            to
        })
    }
}

impl Translator for GeminiTranslator {
    async fn get_translation(&self, paragraph: &str) -> anyhow::Result<ParagraphTranslation> {
        let result = self.client.generate_content()
        .with_system_prompt(Self::get_prompt(&self.to))
        .with_user_message(paragraph)
        .with_response_mime_type("application/json")
        .with_response_schema(self.schema.clone())
        .execute()
        .await?;

        Ok(serde_json::from_str(&result.text())?)
    }
}