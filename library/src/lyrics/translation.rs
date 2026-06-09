use std::{sync::Arc, time::Duration};

use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs, ResponseFormat,
    ResponseFormatJsonSchema,
};
use async_openai::{Client, config::OpenAIConfig};
use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt};
use gemini_rust::{Gemini, Model, ThinkingConfig};
use isolang::Language;
use log::info;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::timeout;

use crate::{
    lyrics::{LyricsLine, LyricsLineTranslation},
    retry::{RetryConfig, retry},
    translator::{
        ProgressCallback, StreamChunkAccumulator, TRANSLATION_REQUEST_TIMEOUT,
        TRANSLATION_STREAM_IDLE_TIMEOUT, TranslationErrors, TranslationModel, TranslationProvider,
        is_transient_translation_error, strip_additional_properties, total_stream_timeout,
    },
};

/// Songs are short; constrain LLM responses to a generous size even for long ones.
/// Used to compute `total_stream_timeout` budget when `input_len` is small (the LRC
/// itself is short, but the response can be 3–5× larger thanks to translations + glosses).
const RESPONSE_LENGTH_FACTOR: usize = 6;

const TRANSLATION_RETRY: RetryConfig = RetryConfig {
    max_attempts: 2,
    base_delay: Duration::from_secs(2),
    max_delay: Duration::from_secs(10),
    jitter_frac: 0.3,
};

#[derive(Debug, Deserialize)]
struct LyricsResponse {
    lines: Vec<LyricsLineTranslation>,
}

#[async_trait]
pub trait LyricsTranslator: Send + Sync {
    async fn translate_song(
        &self,
        lines: &[LyricsLine],
        progress: Option<Box<ProgressCallback>>,
    ) -> anyhow::Result<Vec<LyricsLineTranslation>>;
}

pub fn get_lyrics_translator(
    provider: TranslationProvider,
    model: TranslationModel,
    api_key: String,
    to: Language,
) -> anyhow::Result<Box<dyn LyricsTranslator>> {
    match provider {
        TranslationProvider::Google => Ok(Box::new(LyricsGeminiTranslator::create(
            model, api_key, to,
        )?)),
        TranslationProvider::Openai | TranslationProvider::Deepseek => Ok(Box::new(
            LyricsOpenAITranslator::create(model, api_key, to)?,
        )),
    }
}

fn lyrics_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "lines": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "translation": { "type": "string" },
                        "glosses": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "fragment": { "type": "string" },
                                    "gloss": { "type": "string" },
                                    "note": { "type": "string" }
                                },
                                "required": ["fragment", "gloss", "note"]
                            }
                        }
                    },
                    "required": ["translation", "glosses"]
                }
            }
        },
        "required": ["lines"]
    })
}

fn system_prompt(to: &str) -> String {
    format!(
        "You are translating song lyrics into {to}, in the spirit of \
         «Метод чтения Ильи Франка» (Ilya Frank's reading method). \
         Detect the source language of the lyrics on your own from the text.\n\n\
         For EACH line of the input lyrics, produce one entry in the \"lines\" array, \
         in order. Empty input lines (stanza breaks) get an empty translation and \
         no glosses. The number of output entries MUST equal the number of input lines.\n\n\
         Per line:\n\
         - \"translation\": a natural, close translation into {to}. Preserve imagery; \
           do not over-poeticize, do not flatten metaphors.\n\
         - \"glosses\": 0–6 entries for words or phrases a learner of the song's language \
           whose native language is {to} is unlikely to know — idioms, slang, poetic register, \
           cultural references, less common vocabulary. Skip cognates and trivial words. \
           Each gloss:\n\
           - \"fragment\": exact substring of the original line.\n\
           - \"gloss\": the translation/gloss in {to}.\n\
           - \"note\": short clause (register, idiom, cultural context). Empty string \
             if not applicable.\n\n\
         Return ONLY the JSON object that matches the schema. No markdown."
    )
}

fn user_message(lines: &[LyricsLine]) -> String {
    let mut out = String::with_capacity(lines.iter().map(|l| l.text.len() + 6).sum());
    for (i, line) in lines.iter().enumerate() {
        out.push('[');
        out.push_str(&i.to_string());
        out.push_str("] ");
        out.push_str(&line.text);
        out.push('\n');
    }
    out
}

fn validate_alignment(expected: usize, got: usize) -> anyhow::Result<()> {
    if expected != got {
        anyhow::bail!("Lyrics translation alignment error: expected {expected} lines, got {got}");
    }
    Ok(())
}

/// Approximate response budget for the total-stream timeout. Songs are short, so
/// `total_stream_timeout(_)` of 30s + 0.1s/char is generous when we feed it
/// `lines_len * 6` (translation + glosses inflation).
fn stream_budget_chars(lines: &[LyricsLine]) -> usize {
    let raw: usize = lines.iter().map(|l| l.text.len()).sum();
    raw * RESPONSE_LENGTH_FACTOR + 256
}

// -------- OpenAI ----------------------------------------------------------

pub struct LyricsOpenAITranslator {
    client: Client<OpenAIConfig>,
    schema: Arc<Value>,
    model_name: Arc<str>,
    is_deepseek: bool,
    to: Language,
}

impl LyricsOpenAITranslator {
    pub fn create(
        translation_model: TranslationModel,
        api_key: String,
        to: Language,
    ) -> anyhow::Result<Self> {
        let model_name = openai_model_name(translation_model)?;
        let provider = translation_model.provider();
        let mut config = OpenAIConfig::new().with_api_key(api_key);
        if let Some(url) = provider.and_then(crate::translator::openai::openai_compat_base_url) {
            config = config.with_api_base(url);
        }
        let client = Client::with_config(config);
        Ok(Self {
            client,
            schema: Arc::new(lyrics_schema()),
            model_name: Arc::from(model_name),
            is_deepseek: provider == Some(TranslationProvider::Deepseek),
            to,
        })
    }
}

fn openai_model_name(m: TranslationModel) -> anyhow::Result<&'static str> {
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

#[async_trait]
impl LyricsTranslator for LyricsOpenAITranslator {
    async fn translate_song(
        &self,
        lines: &[LyricsLine],
        progress: Option<Box<ProgressCallback>>,
    ) -> anyhow::Result<Vec<LyricsLineTranslation>> {
        // Re-borrow per attempt so the retry closure can be `FnMut` without consuming progress.
        let progress = progress.as_deref();

        retry(
            TRANSLATION_RETRY,
            is_transient_translation_error,
            "OpenAI lyrics",
            || async move {
                let mut system = format!(
                    "{}\n\nReturn ONLY a single JSON object that matches the requested schema. Do not wrap it in markdown.",
                    system_prompt(self.to.to_name())
                );
                if self.is_deepseek {
                    if let Ok(schema_text) = serde_json::to_string_pretty(&*self.schema) {
                        system.push_str("\n\nJSON schema for the response:\n");
                        system.push_str(&schema_text);
                    }
                }
                let user = user_message(lines);

                let response_format = if self.is_deepseek {
                    ResponseFormat::JsonObject
                } else {
                    ResponseFormat::JsonSchema {
                        json_schema: ResponseFormatJsonSchema {
                            description: Some("Per-line song lyrics translation".to_string()),
                            name: "lyrics_translation".to_string(),
                            schema: Some((*self.schema).clone()),
                            strict: Some(true),
                        },
                    }
                };
                let request = CreateChatCompletionRequestArgs::default()
                    .model(self.model_name.as_ref())
                    .messages([
                        ChatCompletionRequestMessage::System(
                            ChatCompletionRequestSystemMessageArgs::default()
                                .content(system)
                                .build()?,
                        ),
                        ChatCompletionRequestMessage::User(
                            ChatCompletionRequestUserMessageArgs::default()
                                .content(user)
                                .build()?,
                        ),
                    ])
                    .response_format(response_format)
                    .stream(true)
                    .build()?;

                info!(
                    "OpenAI lyrics: model={} to={} lines={}",
                    self.model_name,
                    self.to.to_639_3(),
                    lines.len()
                );

                let mut stream = timeout(
                    TRANSLATION_REQUEST_TIMEOUT,
                    self.client.chat().create_stream(request),
                )
                .await
                .map_err(|_| anyhow::anyhow!("OpenAI lyrics request timed out"))??;

                let mut accumulator = StreamChunkAccumulator::new("OpenAI");
                let full = timeout(
                    total_stream_timeout(stream_budget_chars(lines)),
                    async {
                        loop {
                            let next = timeout(TRANSLATION_STREAM_IDLE_TIMEOUT, stream.next())
                                .await
                                .map_err(|_| anyhow::anyhow!("OpenAI lyrics stream idle timeout"))?;
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
                                progress,
                            )?;
                            if !should_continue {
                                break;
                            }
                        }
                        accumulator.finish()
                    },
                )
                .await
                .map_err(|_| anyhow::anyhow!("OpenAI lyrics total stream timeout"))??;

                let parsed: LyricsResponse = serde_json::from_str(&full)?;
                validate_alignment(lines.len(), parsed.lines.len())?;
                Ok(parsed.lines)
            },
        )
        .await
    }
}

// -------- Gemini ----------------------------------------------------------

/// Gemini rejects `additionalProperties` in `response_schema`; serve it the
/// same source-of-truth schema with that key stripped.
fn gemini_lyrics_schema() -> Value {
    let mut s = lyrics_schema();
    strip_additional_properties(&mut s);
    s
}

pub struct LyricsGeminiTranslator {
    client: Gemini,
    schema: Arc<Value>,
    model: Model,
    to: Language,
}

impl LyricsGeminiTranslator {
    pub fn create(
        translation_model: TranslationModel,
        api_key: String,
        to: Language,
    ) -> anyhow::Result<Self> {
        let model = crate::translator::gemini::gemini_model(translation_model)?;
        let client = crate::translator::gemini::gemini_client(api_key, model.clone())?;
        Ok(Self {
            client,
            schema: Arc::new(gemini_lyrics_schema()),
            model,
            to,
        })
    }
}

#[async_trait]
impl LyricsTranslator for LyricsGeminiTranslator {
    async fn translate_song(
        &self,
        lines: &[LyricsLine],
        progress: Option<Box<ProgressCallback>>,
    ) -> anyhow::Result<Vec<LyricsLineTranslation>> {
        let progress = progress.as_deref();

        retry(
            TRANSLATION_RETRY,
            is_transient_translation_error,
            "Gemini lyrics",
            || async move {
                let system = system_prompt(self.to.to_name());
                let user = user_message(lines);

                info!(
                    "Gemini lyrics: model={:?} to={} lines={}",
                    self.model,
                    self.to.to_639_3(),
                    lines.len()
                );

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
                        .with_system_prompt(system)
                        .with_user_message(user)
                        .with_response_mime_type("application/json")
                        .with_response_schema((*self.schema).clone())
                        .with_thinking_config(thinking_config)
                        .with_safety_settings(
                            crate::translator::gemini::permissive_safety_settings(),
                        )
                        .execute_stream(),
                )
                .await
                .map_err(|_| anyhow::anyhow!("Gemini lyrics request timed out"))??;

                let mut accumulator = StreamChunkAccumulator::new("Gemini");
                let full = timeout(total_stream_timeout(stream_budget_chars(lines)), async {
                    loop {
                        let next = timeout(TRANSLATION_STREAM_IDLE_TIMEOUT, stream.try_next())
                            .await
                            .map_err(|_| anyhow::anyhow!("Gemini lyrics stream idle timeout"))?;
                        let should_continue = accumulator.handle_result(
                            match next {
                                Ok(Some(response)) => Ok(Some(response.text())),
                                Ok(None) => Ok(None),
                                Err(err) => Err(err.into()),
                            },
                            progress,
                        )?;
                        if !should_continue {
                            break;
                        }
                    }
                    accumulator.finish()
                })
                .await
                .map_err(|_| anyhow::anyhow!("Gemini lyrics total stream timeout"))??;

                let parsed: LyricsResponse = serde_json::from_str(&full)?;
                validate_alignment(lines.len(), parsed.lines.len())?;
                Ok(parsed.lines)
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_additional_properties(v: &Value) -> usize {
        match v {
            Value::Object(map) => {
                let here = if map.contains_key("additionalProperties") {
                    1
                } else {
                    0
                };
                here + map.values().map(count_additional_properties).sum::<usize>()
            }
            Value::Array(items) => items.iter().map(count_additional_properties).sum(),
            _ => 0,
        }
    }

    #[test]
    fn openai_schema_contains_additional_properties() {
        // Sanity: source schema does set strict mode for OpenAI.
        assert!(count_additional_properties(&lyrics_schema()) > 0);
    }

    #[test]
    fn gemini_schema_strips_additional_properties() {
        // Gemini rejects `additionalProperties` with HTTP 400, so the variant
        // it sees must have none — recursively.
        assert_eq!(count_additional_properties(&gemini_lyrics_schema()), 0);
    }

    #[test]
    fn classifier_treats_self_emitted_timeouts_as_transient() {
        // The lyrics path emits its own "...lyrics..." timeout strings; confirm
        // the shared classifier still catches them via its generic signatures.
        assert!(is_transient_translation_error(&anyhow::anyhow!(
            "OpenAI lyrics request timed out"
        )));
        assert!(is_transient_translation_error(&anyhow::anyhow!(
            "Gemini lyrics stream idle timeout"
        )));
        assert!(is_transient_translation_error(&anyhow::anyhow!(
            "OpenAI lyrics total stream timeout"
        )));
    }

    #[test]
    fn classifier_rejects_lyrics_alignment_error() {
        // Alignment mismatch is lyrics-specific and permanent — retrying a
        // deterministic miscount just burns tokens.
        assert!(!is_transient_translation_error(&anyhow::anyhow!(
            "Lyrics translation alignment error: expected 5 lines, got 4"
        )));
    }
}
