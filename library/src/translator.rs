mod gemini;

use std::sync::Arc;

use gemini_rust::Model;
use isolang::Language;
use strum::EnumIter;
use tokio::sync::Mutex;

use crate::{
    book::translation_import::ParagraphTranslation, cache::TranslationsCache,
    translator::gemini::GeminiTranslator,
};

#[derive(Debug, Clone, Copy, EnumIter)]
pub enum TranslationModel {
    GeminiFlash = 0,
    GeminiPro = 1,
    GeminiFlashLight = 2,
}

impl From<usize> for TranslationModel {
    fn from(value: usize) -> Self {
        match value {
            0 => TranslationModel::GeminiFlash,
            1 => TranslationModel::GeminiPro,
            2 => TranslationModel::GeminiFlashLight,
            _ => TranslationModel::GeminiFlash,
        }
    }
}

pub trait Translator {
    fn get_translation(
        &self,
        paragraph: &str,
        use_cache: bool,
    ) -> impl std::future::Future<Output = anyhow::Result<ParagraphTranslation>> + Send;

    fn get_prompt(from: &str, to: &str) -> String {
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
    cache: Arc<Mutex<TranslationsCache>>,
    translation_model: TranslationModel,
    api_key: String,
    from: Language,
    to: Language,
) -> anyhow::Result<impl Translator> {
    let model = match translation_model {
        TranslationModel::GeminiFlash => Model::Gemini25Flash,
        TranslationModel::GeminiPro => Model::Gemini25Pro,
        TranslationModel::GeminiFlashLight => Model::Gemini25FlashLite,
    };

    GeminiTranslator::create(cache, model, api_key, &from, &to)
}
