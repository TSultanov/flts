use std::path::PathBuf;
use uuid::Uuid;

use crate::book::translation_import;

pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    pub fn new(prefix: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!("{}_{}", prefix, Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub fn full_word(
    original: &str,
    lemma_src: &str,
    lemma_tgt: &str,
    part_of_speech: &str,
    translations: &[&str],
    is_punctuation: bool,
) -> translation_import::Word {
    translation_import::Word {
        original: original.into(),
        contextual_translations: translations.iter().map(|s| (*s).into()).collect(),
        note: None,
        is_punctuation,
        grammar: translation_import::Grammar {
            original_initial_form: lemma_src.into(),
            target_initial_form: lemma_tgt.into(),
            part_of_speech: part_of_speech.into(),
            plurality: None,
            person: None,
            tense: None,
            case: None,
            other: None,
        },
    }
}

pub fn one_sentence_paragraph(
    full_translation: &str,
    words: Vec<translation_import::Word>,
) -> translation_import::ParagraphTranslation {
    translation_import::ParagraphTranslation {
        timestamp: 0,
        total_tokens: None,
        sentences: vec![translation_import::Sentence {
            full_translation: full_translation.into(),
            words,
        }],
    }
}
