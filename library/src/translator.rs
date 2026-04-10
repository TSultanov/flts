mod gemini;
mod openai;

use std::{fmt::Display, sync::Arc, time::Duration};

use async_trait::async_trait;
use isolang::Language;
use serde::{Deserialize, Serialize};
use strum::EnumIter;
use tokio::time::Instant;

use crate::{
    book::translation_import::ParagraphTranslation, cache::TranslationsCache,
    translator::gemini::GeminiTranslator, translator::openai::OpenAITranslator,
};

const TRANSLATION_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const TRANSLATION_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const TRANSLATION_INTER_CHUNK_TIMEOUT: Duration = Duration::from_secs(30);
const TRANSLATION_TOTAL_TIMEOUT_BASE: Duration = Duration::from_secs(30);
const TRANSLATION_TOTAL_TIMEOUT_PER_CHAR: Duration = Duration::from_millis(100);

type ProgressCallback = dyn Fn(usize) + Send + Sync;

fn total_stream_timeout(input_len: usize) -> Duration {
    TRANSLATION_TOTAL_TIMEOUT_BASE + TRANSLATION_TOTAL_TIMEOUT_PER_CHAR * (input_len as u32)
}

#[derive(Debug)]
struct StreamChunkAccumulator {
    provider: &'static str,
    full_content: String,
    saw_chunk_error: bool,
    last_progress_at: Instant,
}

impl StreamChunkAccumulator {
    fn new(provider: &'static str) -> Self {
        Self {
            provider,
            full_content: String::new(),
            saw_chunk_error: false,
            last_progress_at: Instant::now(),
        }
    }

    fn handle_result(
        &mut self,
        result: anyhow::Result<Option<String>>,
        callback: Option<&ProgressCallback>,
    ) -> anyhow::Result<bool> {
        match result {
            Ok(Some(text)) => {
                if !text.is_empty() {
                    self.full_content.push_str(&text);
                    self.last_progress_at = Instant::now();
                    if let Some(cb) = callback {
                        cb(self.full_content.len());
                    }
                } else if self.last_progress_at.elapsed() > TRANSLATION_INTER_CHUNK_TIMEOUT {
                    anyhow::bail!(
                        "{} stream inter-chunk timeout (no progress for {:?})",
                        self.provider,
                        TRANSLATION_INTER_CHUNK_TIMEOUT
                    );
                }
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(err) if !self.saw_chunk_error => {
                self.saw_chunk_error = true;
                log::warn!("Error in {} stream chunk, retrying once: {err}", self.provider);
                Ok(true)
            }
            Err(err) => anyhow::bail!("{} stream failed after retry: {err}", self.provider),
        }
    }

    fn finish(self) -> anyhow::Result<String> {
        if self.full_content.is_empty() {
            anyhow::bail!("{} returned empty content", self.provider);
        }

        Ok(self.full_content)
    }
}

#[derive(Debug)]
pub enum TranslationErrors {
    UnknownModel,
}

impl std::error::Error for TranslationErrors {}

impl Display for TranslationErrors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Unknown model")
    }
}

#[derive(Debug, Clone, Copy, EnumIter, PartialEq, Eq)]
pub enum TranslationModel {
    Unknown = 0,
    Gemini25Flash = 1,
    Gemini25Pro = 2,
    Gemini25FlashLight = 3,

    // IMPORTANT: do not reorder or renumber existing variants.
    OpenAIGpt5Mini = 4,
    OpenAIGpt52 = 5,
    OpenAIGpt52Pro = 6,
    OpenAIGpt5Nano = 7,

    Gemini3Pro = 8,
    Gemini3Flash = 9,

    OpenAIGpt54 = 10,
    OpenAIGpt54Mini = 11,
    Gemini31Pro = 12,
    Gemini31FlashLite = 13,
}

impl TranslationModel {
    pub fn provider(&self) -> Option<TranslationProvider> {
        match self {
            TranslationModel::Gemini25Flash
            | TranslationModel::Gemini25Pro
            | TranslationModel::Gemini25FlashLight
            | TranslationModel::Gemini3Pro
            | TranslationModel::Gemini3Flash
            | TranslationModel::Gemini31Pro
            | TranslationModel::Gemini31FlashLite => Some(TranslationProvider::Google),

            TranslationModel::OpenAIGpt52
            | TranslationModel::OpenAIGpt52Pro
            | TranslationModel::OpenAIGpt5Mini
            | TranslationModel::OpenAIGpt5Nano
            | TranslationModel::OpenAIGpt54
            | TranslationModel::OpenAIGpt54Mini => Some(TranslationProvider::Openai),

            TranslationModel::Unknown => None,
        }
    }
}

impl From<usize> for TranslationModel {
    fn from(value: usize) -> Self {
        match value {
            1 => TranslationModel::Gemini25Flash,
            2 => TranslationModel::Gemini25Pro,
            3 => TranslationModel::Gemini25FlashLight,
            4 => TranslationModel::OpenAIGpt5Mini,
            5 => TranslationModel::OpenAIGpt52,
            6 => TranslationModel::OpenAIGpt52Pro,
            7 => TranslationModel::OpenAIGpt5Nano,
            8 => TranslationModel::Gemini3Pro,
            9 => TranslationModel::Gemini3Flash,
            10 => TranslationModel::OpenAIGpt54,
            11 => TranslationModel::OpenAIGpt54Mini,
            12 => TranslationModel::Gemini31Pro,
            13 => TranslationModel::Gemini31FlashLite,
            _ => TranslationModel::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TranslationProvider {
    #[default]
    Google,
    Openai,
}

impl TranslationProvider {
    pub fn display_name(&self) -> &'static str {
        match self {
            TranslationProvider::Google => "Google",
            TranslationProvider::Openai => "OpenAI",
        }
    }
}

#[async_trait]
pub trait Translator: Send + Sync {
    fn get_model(&self) -> TranslationModel;

    async fn get_translation(
        &self,
        paragraph: &str,
        use_cache: bool,
        callback: Option<Box<ProgressCallback>>,
    ) -> anyhow::Result<ParagraphTranslation>;

    fn get_prompt(from: &str, to: &str) -> String
    where
        Self: Sized,
    {
        format!(
        "You are given a paragraph in a foreign language. The goal is to construct a translation which can be used by somebody who speaks the {to} language to learn the original language.
        For each sentence provide a good, but close to the original, translation from {from} into the {to} language.
        For each word in the sentence, provide a full translation from {from} into {to} language. Give several translation variants if necessary.
        For compound words and contractions treat them as single words with appropriate grammatical information. Describe the full form in the 'note' field if necessary.
        Add a note on the use of the word if it's not clear how translation maps to the original.
        Preserve all punctuation, including all quotation marks and various kinds of parenthesis or braces.
        Put HTML-encoded values for punctuation signs in the 'original' field, e.g. comma turns into &comma;.
        If you see an HTML line break (<br>) treat it as punctuation and preserve it in the output correspondingly.
        Provide grammatical information for each word.
            - Grammatical information should ONLY be about the original word and how it's used in the original language.
            - Do NOT use concepts from the {to} language when decribing the grammar.
            - Use ONLY concepts which make sense and exist in the {from} language grammatical system, but explain them in the {to} language.
            - All the information given must be in {to} language except for the 'originalInitialForm', 'sourceLanguage' and 'targetLanguage' fields, which should be in the {from} language.
            - Example: For Japanese, use concepts like 'て-form', 'potential form', '連体形'
            - Example: For German, use concepts like 'dative case', 'strong declension'
            - Example: For Russian, use concepts like 'perfective aspect', 'genitive case'
            - Explain these concepts in the TARGET language for the learner
            - In the 'other' field, include any language-specific grammatical features not covered by standard fields
        Initial forms in the grammar section must be contain the form as it appears in the dictionaries in the language of the original and target text.
        'sourceLanguage' and 'targetLanguage' must contain ISO 639 Set 3 code of the corresponding language (e.g. 'eng', 'deu', 'rus', 'jpn', etc.).
        Maintain consistency:
            - Use the same terminology throughout the translation
            - If a word appears multiple times, analyze it consistently
            - Ensure word count matches: every word in original must have a corresponding entry
        Special cases:
            - Numbers: treat as words with 'numeral' part of speech
            - Proper nouns: mark in partOfSpeech as 'proper noun', provide transliteration if needed
            - Idioms: provide literal translation in note field, idiomatic translation in contextualTranslations
            - Honorifics: mark as such and explain their usage level in the note field
        Quality checks before submitting:
            1. Count: Does the number of word entries match the number of words in the original?
            2. Punctuation: Is all punctuation preserved and correctly marked?
            3. Grammar: Did you avoid using TARGET language grammar concepts for SOURCE language analysis?
            4. Completeness: Does every word have all required fields filled?
            5. Consistency: Are repeated words analyzed the same way?
            6. ISO codes: Are sourceLanguage and targetLanguage correct 3-letter ISO 639-3 codes?")
    }
}

pub fn get_translator(
    cache: Arc<TranslationsCache>,
    provider: TranslationProvider,
    translation_model: TranslationModel,
    api_key: String,
    from: Language,
    to: Language,
) -> anyhow::Result<Box<dyn Translator>> {
    match provider {
        TranslationProvider::Google => Ok(Box::new(GeminiTranslator::create(
            cache,
            translation_model,
            api_key,
            &from,
            &to,
        )?)),
        TranslationProvider::Openai => Ok(Box::new(OpenAITranslator::create(
            cache,
            translation_model,
            api_key,
            &from,
            &to,
        )?)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::StreamChunkAccumulator;

    #[test]
    fn first_chunk_error_is_retried() {
        let mut accumulator = StreamChunkAccumulator::new("OpenAI");

        assert!(
            accumulator
                .handle_result(Err(anyhow::anyhow!("boom")), None)
                .unwrap()
        );
        assert!(
            accumulator
                .handle_result(Ok(Some("abc".into())), None)
                .unwrap()
        );
        assert!(!accumulator.handle_result(Ok(None), None).unwrap());
        assert_eq!(accumulator.finish().unwrap(), "abc");
    }

    #[test]
    fn second_chunk_error_fails() {
        let mut accumulator = StreamChunkAccumulator::new("Gemini");

        assert!(
            accumulator
                .handle_result(Err(anyhow::anyhow!("boom-1")), None)
                .unwrap()
        );
        let err = accumulator
            .handle_result(Err(anyhow::anyhow!("boom-2")), None)
            .unwrap_err();

        assert!(err.to_string().contains("Gemini stream failed after retry"));
    }

    #[test]
    fn callback_tracks_cumulative_progress_for_non_empty_chunks() {
        let mut accumulator = StreamChunkAccumulator::new("OpenAI");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_for_cb = Arc::clone(&seen);
        let callback = move |len| seen_for_cb.lock().unwrap().push(len);

        assert!(
            accumulator
                .handle_result(Ok(Some("a".into())), Some(&callback))
                .unwrap()
        );
        assert!(
            accumulator
                .handle_result(Ok(Some(String::new())), Some(&callback))
                .unwrap()
        );
        assert!(
            accumulator
                .handle_result(Ok(Some("bc".into())), Some(&callback))
                .unwrap()
        );
        assert!(
            !accumulator
                .handle_result(Ok(None), Some(&callback))
                .unwrap()
        );

        assert_eq!(accumulator.finish().unwrap(), "abc");
        assert_eq!(*seen.lock().unwrap(), vec![1, 3]);
    }

    #[test]
    fn empty_stream_still_fails() {
        let mut accumulator = StreamChunkAccumulator::new("Gemini");

        assert!(!accumulator.handle_result(Ok(None), None).unwrap());
        assert_eq!(
            accumulator.finish().unwrap_err().to_string(),
            "Gemini returned empty content"
        );
    }

    #[test]
    fn inter_chunk_timeout_fires_on_empty_chunk_flood() {
        use tokio::time::Instant;

        let mut accumulator = StreamChunkAccumulator::new("OpenAI");
        // Send one real chunk so we're past the "empty stream" case
        assert!(
            accumulator
                .handle_result(Ok(Some("a".into())), None)
                .unwrap()
        );

        // Force last_progress_at into the past
        accumulator.last_progress_at =
            Instant::now() - super::TRANSLATION_INTER_CHUNK_TIMEOUT - std::time::Duration::from_secs(1);

        let err = accumulator
            .handle_result(Ok(Some(String::new())), None)
            .unwrap_err();
        assert!(
            err.to_string().contains("inter-chunk timeout"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn inter_chunk_timeout_resets_on_non_empty_chunk() {
        use tokio::time::Instant;

        let mut accumulator = StreamChunkAccumulator::new("Gemini");
        assert!(
            accumulator
                .handle_result(Ok(Some("a".into())), None)
                .unwrap()
        );

        // Push last_progress_at into the past
        accumulator.last_progress_at =
            Instant::now() - super::TRANSLATION_INTER_CHUNK_TIMEOUT - std::time::Duration::from_secs(1);

        // A non-empty chunk should reset the timer, not fail
        assert!(
            accumulator
                .handle_result(Ok(Some("b".into())), None)
                .unwrap()
        );
        assert_eq!(accumulator.full_content, "ab");
    }

    #[test]
    fn total_stream_timeout_scales_with_input() {
        let short = super::total_stream_timeout(100);
        let long = super::total_stream_timeout(1000);
        assert!(long > short, "longer input should have longer timeout");
        assert_eq!(
            short,
            super::TRANSLATION_TOTAL_TIMEOUT_BASE
                + super::TRANSLATION_TOTAL_TIMEOUT_PER_CHAR * 100
        );
    }
}
