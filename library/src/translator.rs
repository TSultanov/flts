pub(crate) mod gemini;
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

pub const TRANSLATION_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
pub const TRANSLATION_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const TRANSLATION_INTER_CHUNK_TIMEOUT: Duration = Duration::from_secs(30);
const TRANSLATION_TOTAL_TIMEOUT_BASE: Duration = Duration::from_secs(30);
const TRANSLATION_TOTAL_TIMEOUT_PER_CHAR: Duration = Duration::from_millis(100);

pub type ProgressCallback = dyn Fn(usize) + Send + Sync;

pub fn total_stream_timeout(input_len: usize) -> Duration {
    TRANSLATION_TOTAL_TIMEOUT_BASE + TRANSLATION_TOTAL_TIMEOUT_PER_CHAR * (input_len as u32)
}

/// Closed set of values the LLM may return in `grammar.partOfSpeech`.
/// The schema's `enum` keyword is built from these tags, and the prompt
/// renders each tag with its description so the LLM has guidance on
/// which to pick.
pub(crate) const PART_OF_SPEECH_VOCABULARY: &[(&str, &str)] = &[
    ("common_noun", "regular common nouns (book, idea, страна, libro, kitap, წიგნი, 书)"),
    ("proper_noun", "names of people, places, brands (Harry, Munich, Москва, Madrid, İstanbul, თბილისი, 北京)"),
    ("pronoun_personal", "personal pronouns including Spanish clitics (I, you, he, she, me/te/se/lo, я, ты, он, 我, 你)"),
    ("pronoun_possessive", "possessive pronouns standing alone as the noun phrase (mine, yours, мой as predicate, mío)"),
    ("pronoun_demonstrative", "demonstrative pronouns standing alone (this, that, этот as predicate, esto, bu, ეს, 这个 in isolation)"),
    ("pronoun_reflexive", "reflexive pronouns (myself, себя, kendi, თავი, 自己)"),
    ("pronoun_relative", "relative pronouns in relative clauses (which, that, who as relativizer; который, que)"),
    ("pronoun_interrogative", "interrogative pronouns in questions (who, what; кто, что; qué; kim, ne; ვინ, რა; 谁, 什么)"),
    ("pronoun_indefinite", "indefinite pronouns (someone, anyone, none; кто-то, никто; alguien; biri; ვინმე; 有人)"),
    ("pronoun_other", "catch-all for pronoun-like words that fit none of the above (everyone/everything, intensive сам/himself, dummy/expletive it/there, impersonal subjects)"),
    ("verb", "main lexical verb in finite or non-finite use that is NOT a participle, gerund, copula, auxiliary, or modal. Imperatives, subjunctives, infinitives, converbs collapse here. Inflection goes in tense/plurality/person/case/other, not in this tag"),
    ("verb_auxiliary", "auxiliary verbs when helping another verb (have eaten, is going, был as past-tense aux, Spanish haber, Turkish ol- as auxiliary, Chinese 在/着 as aspect host when verbal)"),
    ("verb_modal", "modal verbs (can, must, will, should; мочь as modal; deber; -ebil- in Turkish potentialis; 能, 会, 可以)"),
    ("verb_copula", "linking copula (am tired, was hungry; ser/estar in Spanish; есть as copula; dır in Turkish; არის in Georgian; 是 in Chinese)"),
    ("predicative", "Russian-style state words used alone as predicate (можно, нельзя, пора, надо, жаль; Korean/Hungarian have analogs). Use only when the word does not inflect like a verb and functions as the predicate by itself"),
    ("participle_present", "present participle: -ing as adjectival in English; Russian active present идущий / passive present читаемый; Turkish -an; Georgian მ-...-ელ-ი; Spanish gerundio when adjectival"),
    ("participle_past", "past participle: -ed as adjectival in English; Russian active past прочитавший / passive past прочитанный; Turkish -mış/-dik; Georgian -ულ-ი; Spanish participio cantado; deverbal adjectives once lexicalized"),
    ("gerund", "verb form used as a noun: English -ing as nominal subject/object; Russian verbal nouns and деепричастия when nominal; Spanish gerundio when nominal; Turkish -mek/-me nominals; Georgian masdar -ი; converbs when functioning adverbial-nominal. For an -ing word in English: noun-position → gerund, modifier-position → participle_present"),
    ("adjective", "attributive and predicative adjectives, Japanese i-adj / na-adj (record the type in grammar.other), Russian short/long forms (form in grammar.other)"),
    ("adverb", "adverbs (modify verbs, adjectives, or other adverbs)"),
    ("determiner_article", "articles: a, an, the; el/la; der/die/das"),
    ("determiner_demonstrative", "demonstrative determiners: this/that BEFORE a noun (this car); этот/эта before a noun; este libro; bu kitap; ეს წიგნი; 这本书"),
    ("determiner_possessive", "possessive determiners: my/your BEFORE a noun (my book); мой before a noun; mi libro"),
    ("determiner_quantifier", "quantifier determiners (some, many, several, few, all; несколько, mucho, çok, ბევრი, 很多, 一些)"),
    ("preposition", "prepositions: relational markers placed BEFORE their complement (in, on, of, against; под, для, из; en, de)"),
    ("postposition", "postpositions: relational markers placed AFTER their complement. Turkish ile/için/gibi; Georgian -ში/-ზე/-თვის; Japanese case-marking particles に/で/へ when functioning as postpositions"),
    ("conjunction_coordinating", "coordinating conjunctions (and, but, or; и, а, но; y, o, pero; ve, ama, veya; და, მაგრამ; 和, 但是, 或者)"),
    ("conjunction_subordinating", "subordinating conjunctions (because, although, when; если, что as conj., потому что; porque, aunque, cuando; çünkü, eğer; რომ, თუ; 因为, 虽然, 如果)"),
    ("particle", "particle: broad function-word bucket. Infinitive marker English to; phrasal-verb particle up/down/out; negation не/ни, not, değil, არ, 不/没; Japanese binding/topic/case particles は/が/を when not used as postpositions; question markers Turkish mi/mı, Japanese か, Chinese 吗; aspect markers Chinese 了/着/过, Japanese た/て; sentence-final particles Chinese 吧/呢, Japanese よ/ね. Use this when a function word is neither preposition nor postposition nor conjunction"),
    ("classifier", "classifier / measure word: Chinese 个, 只, 本, 张; Japanese counters 個, 本, 枚, 匹; Korean numerative"),
    ("interjection", "interjection or onomatopoeia (oh, wow, ой, ах, ay; boom, мяу, わんわん, 喵)"),
    ("numeral_cardinal", "cardinal numerals (one, two; один, два; uno, dos; bir, iki; ერთი, ორი; 一, 二)"),
    ("numeral_ordinal", "ordinal numerals (first, second; первый, второй; primero, segundo; birinci, ikinci; პირველი, მეორე; 第一, 第二)"),
    ("affix", "bound morphemes the LLM occasionally returns as separate words (-ed, -ing, English 's; Russian -сь/-ся; Turkish suffix chains when split; Georgian preverb მი-/მო-)"),
    ("other", "last-resort escape ONLY for the rare case nothing else fits (acronyms used as words, untranslatable transliterations, gibberish in the source). Do not default to this"),
];

#[derive(Debug)]
pub struct StreamChunkAccumulator {
    provider: &'static str,
    full_content: String,
    saw_chunk_error: bool,
    last_progress_at: Instant,
}

impl StreamChunkAccumulator {
    pub fn new(provider: &'static str) -> Self {
        Self {
            provider,
            full_content: String::new(),
            saw_chunk_error: false,
            last_progress_at: Instant::now(),
        }
    }

    pub fn handle_result(
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
                log::warn!(
                    "Error in {} stream chunk, retrying once: {err}",
                    self.provider
                );
                Ok(true)
            }
            Err(err) => anyhow::bail!("{} stream failed after retry: {err}", self.provider),
        }
    }

    pub fn finish(self) -> anyhow::Result<String> {
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

#[derive(Debug, Clone, Copy, EnumIter, PartialEq, Eq, Hash)]
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
        let mut pos_block = String::from(
            "Part-of-speech vocabulary. The 'partOfSpeech' field is restricted by the JSON schema to one of the following English tags; pick the one whose scope best fits the word. Do NOT translate these tags or invent new ones. Inflection information (tense, plurality, person, case) belongs in the dedicated grammar fields, NOT in partOfSpeech.\n",
        );
        for (tag, description) in PART_OF_SPEECH_VOCABULARY {
            pos_block.push_str("            - ");
            pos_block.push_str(tag);
            pos_block.push_str(": ");
            pos_block.push_str(description);
            pos_block.push('\n');
        }

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
            - All the information given must be in {to} language except for the 'originalInitialForm', 'sourceLanguage', 'targetLanguage' and 'partOfSpeech' fields. 'originalInitialForm' is in {from}; 'sourceLanguage' and 'targetLanguage' are ISO 639-3 codes; 'partOfSpeech' is one of the canonical English tags listed below.
            - Example: For Japanese, use concepts like 'て-form', 'potential form', '連体形'
            - Example: For German, use concepts like 'dative case', 'strong declension'
            - Example: For Russian, use concepts like 'perfective aspect', 'genitive case'
            - Explain these concepts in the TARGET language for the learner
            - In the 'other' field, include any language-specific grammatical features not covered by standard fields
        Initial forms in the grammar section must be contain the form as it appears in the dictionaries in the language of the original and target text.
        'sourceLanguage' and 'targetLanguage' must contain ISO 639 Set 3 code of the corresponding language (e.g. 'eng', 'deu', 'rus', 'jpn', etc.).
        {pos_block}
        Maintain consistency:
            - Use the same terminology throughout the translation
            - If a word appears multiple times, analyze it consistently
            - Ensure word count matches: every word in original must have a corresponding entry
        Special cases:
            - Idioms: provide literal translation in note field, idiomatic translation in contextualTranslations
            - Honorifics: mark as such and explain their usage level in the note field
        Quality checks before submitting:
            1. Count: Does the number of word entries match the number of words in the original?
            2. Punctuation: Is all punctuation preserved and correctly marked?
            3. Grammar: Did you avoid using TARGET language grammar concepts for SOURCE language analysis?
            4. Completeness: Does every word have all required fields filled?
            5. Consistency: Are repeated words analyzed the same way?
            6. ISO codes: Are sourceLanguage and targetLanguage correct 3-letter ISO 639-3 codes?
            7. partOfSpeech: Is every word's partOfSpeech one of the canonical English tags listed above?")
    }
}

/// Single source of truth for the paragraph-translation response schema.
///
/// Written in OpenAI Structured Outputs strict form (every object is closed and
/// every property is required) so OpenAI can use it with `strict: true`. Gemini
/// reads the same shape via JSON Schema embedded in the system prompt and
/// produces extra empty-string fields for the optional grammar slots; those
/// deserialize cleanly into `Option<String>`.
pub(crate) fn paragraph_translation_schema() -> serde_json::Value {
    let pos_enum: Vec<&str> = PART_OF_SPEECH_VOCABULARY.iter().map(|(tag, _)| *tag).collect();
    serde_json::json!({
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
                                    "original": { "type": "string", "description": "Original word" },
                                    "contextualTranslations": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "Translation variants which are suitable for the current context"
                                    },
                                    "note": { "type": "string", "description": "Note about the translation, if necessary for understanding" },
                                    "isPunctuation": { "type": "boolean" },
                                    "grammar": {
                                        "type": "object",
                                        "additionalProperties": false,
                                        "properties": {
                                            "originalInitialForm": { "type": "string", "description": "Original word in its initial (dictionary) form" },
                                            "targetInitialForm": { "type": "string", "description": "Translated word in its initial (dictionary) form" },
                                            "partOfSpeech": {
                                                "type": "string",
                                                "enum": pos_enum,
                                                "description": "Part of speech of the original word. Must be one of the enumerated tags; see prompt for the scope of each."
                                            },
                                            "plurality": { "type": "string", "description": "Plurality of the original word, if applicable" },
                                            "person": { "type": "string", "description": "Person of the original word, if applicable" },
                                            "tense": { "type": "string", "description": "Tense of the original word, if applicable" },
                                            "case": { "type": "string", "description": "What case the original word is in, if applicable" },
                                            "other": { "type": "string", "description": "Other grammatical information about the original word, if not described by other fields" }
                                        },
                                        "required": [
                                            "partOfSpeech", "originalInitialForm", "targetInitialForm",
                                            "plurality", "person", "tense", "case", "other"
                                        ]
                                    }
                                },
                                "required": ["original", "contextualTranslations", "note", "grammar", "isPunctuation"]
                            }
                        },
                        "fullTranslation": { "type": "string", "description": "Full translation of the sentence" }
                    },
                    "required": ["words", "fullTranslation"]
                }
            },
            "sourceLanguage": { "type": "string" },
            "targetLanguage": { "type": "string" }
        },
        "required": ["sentences", "sourceLanguage", "targetLanguage"]
    })
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
        accumulator.last_progress_at = Instant::now()
            - super::TRANSLATION_INTER_CHUNK_TIMEOUT
            - std::time::Duration::from_secs(1);

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
        accumulator.last_progress_at = Instant::now()
            - super::TRANSLATION_INTER_CHUNK_TIMEOUT
            - std::time::Duration::from_secs(1);

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
            super::TRANSLATION_TOTAL_TIMEOUT_BASE + super::TRANSLATION_TOTAL_TIMEOUT_PER_CHAR * 100
        );
    }
}
