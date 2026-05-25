use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures_util::TryStreamExt;
use gemini_rust::{Gemini, Model, ThinkingConfig};
use isolang::Language;
use serde_json::Value;
use tokio::time::timeout;

use crate::{
    book::translation_import::ParagraphTranslation,
    cache::TranslationsCache,
    translator::{
        ProgressCallback, TranslationErrors, TranslationModel, Translator,
        paragraph_translation_schema, strip_additional_properties,
    },
};

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
        translation_model: TranslationModel,
        api_key: String,
        from: &Language,
        to: &Language,
    ) -> anyhow::Result<GeminiTranslator> {
        let model = gemini_model(translation_model)?;
        let client = gemini_client(api_key, model.clone())?;

        Ok(Self {
            cache,
            client,
            schema: Arc::new(gemini_paragraph_schema()),
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

        let thinking_config = match &self.model {
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
        };

        let mut stream = timeout(
            TRANSLATION_REQUEST_TIMEOUT,
            self.client
                .generate_content()
                .with_system_prompt(Self::get_prompt(self.from.to_name(), self.to.to_name()))
                .with_user_message(paragraph)
                .with_response_mime_type("application/json")
                .with_response_schema((*self.schema).clone())
                .with_thinking_config(thinking_config)
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
                    callback.as_deref(),
                )?;
                if !should_continue {
                    break;
                }
            }
            accumulator.finish()
        })
        .await
        .map_err(|_| anyhow::anyhow!("Gemini total stream timeout"))??;

        let mut translation: ParagraphTranslation = serde_json::from_str(&full_content)?;

        let now = SystemTime::now();
        let duration_since_epoch = now.duration_since(UNIX_EPOCH)?;
        translation.timestamp = duration_since_epoch.as_secs();

        self.cache
            .set(&self.from, &self.to, paragraph, &translation);

        Ok(translation)
    }
}
