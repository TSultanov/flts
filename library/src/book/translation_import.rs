use crate::book::translation::{ParagraphTranslationView, SentenceView, WordView};

pub struct ParagraphTranslation {
    pub timestamp: usize,
    pub sentences: Vec<Sentence>,
}

pub struct Sentence {
    pub full_translation: String,
    pub words: Vec<Word>,
}

pub struct Word {
    pub original: String,
    pub contextual_translations: Vec<String>,
    pub note: String,
    pub is_punctuation: bool,
    pub grammar: Grammar,
}

pub struct Grammar {
    pub original_initial_form: String,
    pub target_initial_form: String,
    pub part_of_speech: String,
    pub plurality: Option<String>,
    pub person: Option<String>,
    pub tense: Option<String>,
    pub case: Option<String>,
    pub other: Option<String>,
}
