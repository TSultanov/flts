use isolang::Language;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::book::translation_import;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    #[serde(default = "default_version")]
    pub version: u32,
    pub id: String,
    pub lemma: String,
    pub part_of_speech: String,
    pub translations: Vec<String>,
    pub examples: Vec<Example>,
    pub anki_data: Option<AnkiData>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Example {
    pub source: String,
    pub translation: String,
    pub book_id: Uuid,
    pub chapter: usize,
    pub paragraph: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnkiData {
    pub state: AnkiState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_days: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease_factor: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fsrs_difficulty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fsrs_stability: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnkiState {
    Active,
    Suspended,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardKey {
    pub source_language: String,
    pub target_language: String,
    pub lemma: String,
    pub slug: String,
    pub part_of_speech: String,
}

impl CardKey {
    pub fn id(&self) -> String {
        card_id(&self.source_language, &self.target_language, &self.slug, &self.part_of_speech)
    }
}

pub fn canonicalize_lemma(raw: &str, _src_lang: Language) -> String {
    // _src_lang is reserved for locale-aware lowercase (Turkish dotted/dotless `i`).
    // Stage 2 uses Unicode default. See .specs/ANKI_PLAN.md "Known follow-ups".
    let nfc: String = raw.nfc().collect();
    let lowered = nfc.to_lowercase();
    let apostrophe_normalized: String = lowered
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{02BC}' => '\'',
            other => other,
        })
        .collect();
    apostrophe_normalized
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn lemma_slug(canonical: &str) -> String {
    canonical
        .chars()
        .filter_map(|c| {
            if c.is_whitespace() {
                Some('_')
            } else if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                None
            } else {
                Some(c)
            }
        })
        .collect()
}

pub fn card_id(source_language: &str, target_language: &str, slug: &str, part_of_speech: &str) -> String {
    format!("flts_{source_language}_{target_language}_{slug}_{part_of_speech}")
}

pub fn is_eligible(word: &translation_import::Word) -> bool {
    if word.is_punctuation {
        return false;
    }
    let lemma = word.grammar.original_initial_form.trim();
    if lemma.is_empty() {
        return false;
    }
    if is_pure_digit_lemma(lemma) {
        return false;
    }
    true
}

fn is_pure_digit_lemma(lemma: &str) -> bool {
    let mut has_digit = false;
    for c in lemma.chars() {
        if c.is_ascii_digit() {
            has_digit = true;
        } else if !matches!(c, '.' | ',' | '_') {
            return false;
        }
    }
    has_digit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(lemma: &str, is_punctuation: bool) -> translation_import::Word {
        translation_import::Word {
            original: lemma.into(),
            contextual_translations: vec![],
            note: None,
            is_punctuation,
            grammar: translation_import::Grammar {
                original_initial_form: lemma.into(),
                target_initial_form: String::new(),
                part_of_speech: "noun".into(),
                plurality: None,
                person: None,
                tense: None,
                case: None,
                other: None,
            },
        }
    }

    fn spa() -> Language {
        Language::from_639_3("spa").unwrap()
    }

    #[test]
    fn canonicalize_nfc_roundtrip() {
        let nfd = "e\u{301}";
        assert_eq!(canonicalize_lemma(nfd, spa()), "\u{00e9}");
    }

    #[test]
    fn canonicalize_lowercase() {
        assert_eq!(canonicalize_lemma("España", spa()), "españa");
    }

    #[test]
    fn canonicalize_curly_apostrophe() {
        assert_eq!(canonicalize_lemma("l\u{2019}amour", spa()), "l'amour");
        assert_eq!(canonicalize_lemma("L\u{2018}AMOUR", spa()), "l'amour");
    }

    #[test]
    fn canonicalize_whitespace_collapse() {
        assert_eq!(canonicalize_lemma("darse  cuenta", spa()), "darse cuenta");
        assert_eq!(canonicalize_lemma("  darse\tcuenta  ", spa()), "darse cuenta");
    }

    #[test]
    fn canonicalize_composes_all() {
        let messy = "  DARSE\u{00A0}\u{00A0}CUENTA\u{2019}s  ";
        let canonical = canonicalize_lemma(messy, spa());
        assert_eq!(canonical, "darse cuenta's");
    }

    #[test]
    fn slug_replaces_internal_space() {
        assert_eq!(lemma_slug("darse cuenta"), "darse_cuenta");
    }

    #[test]
    fn slug_drops_unsafe_chars() {
        assert_eq!(lemma_slug("a/b\\c:d*e?f\"g<h>i|j"), "abcdefghij");
    }

    #[test]
    fn slug_preserves_unicode() {
        assert_eq!(lemma_slug("café"), "café");
        assert_eq!(lemma_slug("мочь"), "мочь");
    }

    #[test]
    fn card_id_format() {
        assert_eq!(
            card_id("spa", "rus", "poder", "verb"),
            "flts_spa_rus_poder_verb"
        );
    }

    #[test]
    fn eligible_keeps_normal_word() {
        assert!(is_eligible(&word("hola", false)));
    }

    #[test]
    fn eligible_rejects_punctuation() {
        assert!(!is_eligible(&word(".", true)));
        assert!(!is_eligible(&word("hola", true)));
    }

    #[test]
    fn eligible_rejects_pure_digits() {
        for lemma in ["42", "2026", "1,000", "3.14", "1_000", "5"] {
            assert!(
                !is_eligible(&word(lemma, false)),
                "expected {lemma} to be ineligible"
            );
        }
    }

    #[test]
    fn eligible_keeps_word_form_numeral() {
        for lemma in ["cinco", "пять", "fünf", "five"] {
            assert!(is_eligible(&word(lemma, false)), "expected {lemma} eligible");
        }
    }

    #[test]
    fn card_round_trips_through_json() {
        let card = Card {
            version: 1,
            id: "flts_spa_rus_poder_verb".into(),
            lemma: "poder".into(),
            part_of_speech: "verb".into(),
            translations: vec!["мочь".into()],
            examples: vec![Example {
                source: "No puedo más.".into(),
                translation: "Я больше не могу.".into(),
                book_id: Uuid::nil(),
                chapter: 3,
                paragraph: 12,
            }],
            anki_data: None,
        };
        let json = serde_json::to_string(&card).unwrap();
        let back: Card = serde_json::from_str(&json).unwrap();
        assert_eq!(card, back);
        assert!(json.contains("\"version\":1"));
        assert!(json.contains("\"anki_data\":null"));
    }
}
