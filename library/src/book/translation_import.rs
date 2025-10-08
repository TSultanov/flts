use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct ParagraphTranslation {
    #[serde(skip)]
    pub timestamp: u64,
    pub sentences: Vec<Sentence>,
    #[serde(alias = "sourceLanguage")]
    pub source_language: String,
    #[serde(alias = "targetLanguage")]
    pub target_language: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Sentence {
    #[serde(alias = "fullTranslation")]
    pub full_translation: String,
    pub words: Vec<Word>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Word {
    pub original: String,
    #[serde(alias = "contextualTranslations")]
    pub contextual_translations: Vec<String>,
    pub note: String,
    #[serde(alias = "isPunctuation")]
    pub is_punctuation: bool,
    pub grammar: Grammar,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Grammar {
    #[serde(alias = "originalInitialForm")]
    pub original_initial_form: String,
    #[serde(alias = "targetInitialForm")]
    pub target_initial_form: String,
    #[serde(alias = "partOfSpeech")]
    pub part_of_speech: String,
    pub plurality: Option<String>,
    pub person: Option<String>,
    pub tense: Option<String>,
    pub case: Option<String>,
    pub other: Option<String>,
}
