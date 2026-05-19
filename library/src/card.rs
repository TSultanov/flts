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

pub const EXAMPLES_CAP: usize = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct CardUpdate {
    pub key: CardKey,
    pub translations: Vec<String>,
    pub example: Option<Example>,
}

impl Card {
    pub fn new_from_update(update: &CardUpdate) -> Self {
        let mut translations: Vec<String> = Vec::with_capacity(update.translations.len());
        for t in &update.translations {
            if !translations.contains(t) {
                translations.push(t.clone());
            }
        }
        let examples = update
            .example
            .as_ref()
            .map(|e| vec![e.clone()])
            .unwrap_or_default();
        Card {
            version: 1,
            id: update.key.id(),
            lemma: update.key.lemma.clone(),
            part_of_speech: update.key.part_of_speech.clone(),
            translations,
            examples,
            anki_data: None,
        }
    }

    pub fn apply_update(&mut self, update: &CardUpdate) {
        for t in &update.translations {
            if !self.translations.contains(t) {
                self.translations.push(t.clone());
            }
        }
        if let Some(example) = &update.example
            && !self.examples.iter().any(|e| {
                e.book_id == example.book_id
                    && e.chapter == example.chapter
                    && e.paragraph == example.paragraph
            })
            && self.examples.len() < EXAMPLES_CAP
        {
            self.examples.push(example.clone());
        }
    }
}

pub fn extract_card_updates(
    paragraph: &translation_import::ParagraphTranslation,
    src_lang: Language,
    tgt_lang: Language,
    book_id: Uuid,
    chapter: usize,
    paragraph_index: usize,
) -> Vec<CardUpdate> {
    let source_language = src_lang.to_639_3();
    let target_language = tgt_lang.to_639_3();
    let mut updates: Vec<CardUpdate> = Vec::new();

    for sentence in &paragraph.sentences {
        let source_text = sentence
            .words
            .iter()
            .map(|w| w.original.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let example = Example {
            source: source_text,
            translation: sentence.full_translation.clone(),
            book_id,
            chapter,
            paragraph: paragraph_index,
        };

        for word in &sentence.words {
            if !is_eligible(word) {
                continue;
            }
            let lemma = canonicalize_lemma(&word.grammar.original_initial_form, src_lang);
            if lemma.is_empty() {
                continue;
            }
            let slug = lemma_slug(&lemma);
            let key = CardKey {
                source_language: source_language.to_owned(),
                target_language: target_language.to_owned(),
                lemma,
                slug,
                part_of_speech: word.grammar.part_of_speech.clone(),
            };

            if let Some(existing) = updates.iter_mut().find(|u| u.key == key) {
                for t in &word.contextual_translations {
                    if !existing.translations.contains(t) {
                        existing.translations.push(t.clone());
                    }
                }
            } else {
                let mut translations: Vec<String> = Vec::new();
                for t in &word.contextual_translations {
                    if !translations.contains(t) {
                        translations.push(t.clone());
                    }
                }
                updates.push(CardUpdate {
                    key,
                    translations,
                    example: Some(example.clone()),
                });
            }
        }
    }

    updates
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

    fn full_word(
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

    fn one_sentence_paragraph(
        full_translation: &str,
        words: Vec<translation_import::Word>,
    ) -> translation_import::ParagraphTranslation {
        translation_import::ParagraphTranslation {
            timestamp: 0,
            total_tokens: None,
            source_language: "spa".into(),
            target_language: "rus".into(),
            sentences: vec![translation_import::Sentence {
                full_translation: full_translation.into(),
                words,
            }],
        }
    }

    fn poder_key() -> CardKey {
        CardKey {
            source_language: "spa".into(),
            target_language: "rus".into(),
            lemma: "poder".into(),
            slug: "poder".into(),
            part_of_speech: "verb".into(),
        }
    }

    fn example_at(book_id: Uuid, chapter: usize, paragraph: usize) -> Example {
        Example {
            source: "No puedo más.".into(),
            translation: "Я больше не могу.".into(),
            book_id,
            chapter,
            paragraph,
        }
    }

    #[test]
    fn new_card_from_update_has_version_1_anki_data_null() {
        let update = CardUpdate {
            key: poder_key(),
            translations: vec!["мочь".into()],
            example: Some(example_at(Uuid::nil(), 1, 2)),
        };
        let card = Card::new_from_update(&update);
        assert_eq!(card.version, 1);
        assert_eq!(card.id, "flts_spa_rus_poder_verb");
        assert_eq!(card.lemma, "poder");
        assert_eq!(card.part_of_speech, "verb");
        assert_eq!(card.translations, vec!["мочь"]);
        assert_eq!(card.examples.len(), 1);
        assert!(card.anki_data.is_none());
    }

    #[test]
    fn apply_update_adds_new_translation() {
        let mut card = Card::new_from_update(&CardUpdate {
            key: poder_key(),
            translations: vec!["мочь".into()],
            example: None,
        });
        card.apply_update(&CardUpdate {
            key: poder_key(),
            translations: vec!["уметь".into()],
            example: None,
        });
        assert_eq!(card.translations, vec!["мочь", "уметь"]);
    }

    #[test]
    fn apply_update_dedups_translation() {
        let mut card = Card::new_from_update(&CardUpdate {
            key: poder_key(),
            translations: vec!["мочь".into()],
            example: None,
        });
        card.apply_update(&CardUpdate {
            key: poder_key(),
            translations: vec!["мочь".into(), "уметь".into(), "мочь".into()],
            example: None,
        });
        assert_eq!(card.translations, vec!["мочь", "уметь"]);
    }

    #[test]
    fn apply_update_adds_new_example_with_distinct_provenance() {
        let book = Uuid::new_v4();
        let mut card = Card::new_from_update(&CardUpdate {
            key: poder_key(),
            translations: vec![],
            example: Some(example_at(book, 1, 1)),
        });
        card.apply_update(&CardUpdate {
            key: poder_key(),
            translations: vec![],
            example: Some(example_at(book, 1, 2)),
        });
        assert_eq!(card.examples.len(), 2);
    }

    #[test]
    fn apply_update_skips_example_with_same_provenance() {
        let book = Uuid::new_v4();
        let mut card = Card::new_from_update(&CardUpdate {
            key: poder_key(),
            translations: vec![],
            example: Some(example_at(book, 1, 1)),
        });
        card.apply_update(&CardUpdate {
            key: poder_key(),
            translations: vec![],
            example: Some(example_at(book, 1, 1)),
        });
        assert_eq!(card.examples.len(), 1);
    }

    #[test]
    fn apply_update_caps_examples_at_10() {
        let book = Uuid::new_v4();
        let mut card = Card::new_from_update(&CardUpdate {
            key: poder_key(),
            translations: vec![],
            example: Some(example_at(book, 0, 0)),
        });
        for i in 1..20 {
            card.apply_update(&CardUpdate {
                key: poder_key(),
                translations: vec![],
                example: Some(example_at(book, 0, i)),
            });
        }
        assert_eq!(card.examples.len(), EXAMPLES_CAP);
        // earliest-by-insertion retained
        assert_eq!(card.examples[0].paragraph, 0);
        assert_eq!(card.examples[9].paragraph, 9);
    }

    #[test]
    fn apply_update_handles_empty_translations_no_example() {
        let mut card = Card::new_from_update(&CardUpdate {
            key: poder_key(),
            translations: vec!["мочь".into()],
            example: None,
        });
        let before = card.clone();
        card.apply_update(&CardUpdate {
            key: poder_key(),
            translations: vec![],
            example: None,
        });
        assert_eq!(card, before);
    }

    #[test]
    fn walk_produces_one_update_per_eligible_word() {
        let p = one_sentence_paragraph(
            "Я больше не могу.",
            vec![
                full_word("No", "no", "не", "adv", &["не"], false),
                full_word("puedo", "poder", "мочь", "verb", &["могу"], false),
                full_word("más", "más", "больше", "adv", &["больше"], false),
                full_word(".", ".", ".", "punct", &[], true),
            ],
        );
        let updates = extract_card_updates(
            &p,
            Language::from_639_3("spa").unwrap(),
            Language::from_639_3("rus").unwrap(),
            Uuid::nil(),
            0,
            0,
        );
        assert_eq!(updates.len(), 3, "punctuation should be excluded");
    }

    #[test]
    fn walk_skips_punctuation() {
        let p = one_sentence_paragraph(
            ".",
            vec![full_word(".", ".", ".", "punct", &[], true)],
        );
        let updates = extract_card_updates(
            &p,
            Language::from_639_3("spa").unwrap(),
            Language::from_639_3("rus").unwrap(),
            Uuid::nil(),
            0,
            0,
        );
        assert!(updates.is_empty());
    }

    #[test]
    fn walk_skips_pure_digits() {
        let p = one_sentence_paragraph(
            "42",
            vec![
                full_word("42", "42", "42", "num", &["42"], false),
                full_word("años", "año", "год", "noun", &["лет"], false),
            ],
        );
        let updates = extract_card_updates(
            &p,
            Language::from_639_3("spa").unwrap(),
            Language::from_639_3("rus").unwrap(),
            Uuid::nil(),
            0,
            0,
        );
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].key.lemma, "año");
    }

    #[test]
    fn walk_uses_canonicalized_lemma_in_key() {
        let p = one_sentence_paragraph(
            "Mosca",
            vec![full_word("España", "España", "Испания", "propn", &["Испания"], false)],
        );
        let updates = extract_card_updates(
            &p,
            Language::from_639_3("spa").unwrap(),
            Language::from_639_3("rus").unwrap(),
            Uuid::nil(),
            0,
            0,
        );
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].key.lemma, "españa");
        assert_eq!(updates[0].key.slug, "españa");
    }

    #[test]
    fn walk_carries_provenance() {
        let book = Uuid::new_v4();
        let p = one_sentence_paragraph(
            "Я могу.",
            vec![full_word("puedo", "poder", "мочь", "verb", &["могу"], false)],
        );
        let updates = extract_card_updates(
            &p,
            Language::from_639_3("spa").unwrap(),
            Language::from_639_3("rus").unwrap(),
            book,
            3,
            12,
        );
        let example = updates[0].example.as_ref().unwrap();
        assert_eq!(example.book_id, book);
        assert_eq!(example.chapter, 3);
        assert_eq!(example.paragraph, 12);
        assert_eq!(example.source, "puedo");
        assert_eq!(example.translation, "Я могу.");
    }

    #[test]
    fn walk_dedupes_within_paragraph() {
        let p = translation_import::ParagraphTranslation {
            timestamp: 0,
            total_tokens: None,
            source_language: "spa".into(),
            target_language: "rus".into(),
            sentences: vec![
                translation_import::Sentence {
                    full_translation: "Я могу.".into(),
                    words: vec![full_word("puedo", "poder", "мочь", "verb", &["могу"], false)],
                },
                translation_import::Sentence {
                    full_translation: "Я не могу.".into(),
                    words: vec![full_word("puedo", "poder", "мочь", "verb", &["умею"], false)],
                },
            ],
        };
        let updates = extract_card_updates(
            &p,
            Language::from_639_3("spa").unwrap(),
            Language::from_639_3("rus").unwrap(),
            Uuid::nil(),
            0,
            0,
        );
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].translations, vec!["могу", "умею"]);
        // example is from the first sentence the lemma appeared in
        assert_eq!(updates[0].example.as_ref().unwrap().translation, "Я могу.");
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
