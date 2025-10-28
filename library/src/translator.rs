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
    GeminiFlash,
    GeminiPro,
}

pub trait Translator {
    fn get_translation(
        &self,
        paragraph: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<ParagraphTranslation>> + Send;

    fn get_prompt(from: &str, to: &str) -> String {
        format!(
        "You are given a paragraph in a foreign language. The goal is to construct a translation which can be used by somebody who speaks the {to} language to learn the original language.
        For each sentence provide a good, but close to the original, translation from {from} into the {to} language.
        For each word in the sentence, provide a full translation from {from} into {to} language. Give several translation variants if necessary.
        Add a note on the use of the word if it's not clear how translation maps to the original.
        Preserve all punctuation, including all quotation marks and various kinds of parenthesis or braces.
        Put HTML-encoded values for punctuation signs in the 'original' field, e.g. comma turns into &comma;.
        For punctuation signs which are meant to be written separately from words (e.g. em- and en-dashes) put 'true' in the 'isStandalonePunctuation' field. For punctuation signs which are written without space before it put 'false' into the 'isStandalonePunctuation' field.
        If you see an HTML line break (<br>) treat it as a standalone punctuation and preserve it in the output correspondingly.
        Provide grammatical information for each word. Grammatical information should ONLY be about the original word and how it's used in the original language. Do NOT use concepts from the {to} language when decribing the grammar. Use ONLY concepts which make sense and exist in the {from} language of the original text, but use the {to} language to describe it.
        All the information given must be in {to} language except for the 'originalInitialForm', 'sourceLanguage' and 'targetLanguage' fields, which should be in the {from} language.
        Initial forms in the grammar section must be contain the form as it appears in the dictionaries in the language of the original and target text.
        'sourceLanguage' and 'targetLanguage' must contain ISO 639 Set 3 code of the corresponding language (e.g. 'eng', 'deu', 'rus', 'jpn', etc.).
        Before giving the final answer to the user, re-read it and fix mistakes. Double-check that you correctly carried over the punctuation. Make sure that you don't accidentally use concepts which only exist in the {to} language to describe word in the source text.
        Triple-check that you didn't miss any words!")
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
    };

    GeminiTranslator::create(cache, model, api_key, &from, &to)
}
