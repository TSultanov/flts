use std::collections::BTreeMap;

use htmlentity::entity::{ICodedDataTrait, decode};
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
    pub translations: BTreeMap<String, Vec<String>>,
    pub examples: Vec<Example>,
    pub anki_data: Option<AnkiData>,
}

fn default_version() -> u32 {
    2
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

pub const MATURE_DAYS: f32 = 90.0;

/// Map a card's Anki retention state into a `[0.0, 1.0]` familiarity scalar
/// for the reader-side UI.
///
/// A missing `anki_data` (card not yet synced to Anki) collapses to `Some(0.0)`
/// — the reader treats a never-studied word as the lowest knowledge level, so
/// the underline renders at full opacity and the translation auto-overlays.
/// Suspended/Deleted return `None` (dormant) — the user actively retired the
/// card from study, so no underline and no overlay.
///
/// See `.specs/ANKI_UI.md` § Familiarity scalar for the contract.
pub fn familiarity_from(anki_data: Option<&AnkiData>) -> Option<f32> {
    let Some(data) = anki_data else {
        return Some(0.0);
    };
    match data.state {
        AnkiState::Suspended | AnkiState::Deleted => None,
        AnkiState::Active => {
            let days = data.fsrs_stability.or(data.interval_days).unwrap_or(0.0) as f32;
            let raw = (1.0 + days.max(0.0)).log10() / (1.0 + MATURE_DAYS).log10();
            Some(raw.clamp(0.0, 1.0))
        }
    }
}

#[derive(Debug, Clone)]
pub struct CardKey {
    pub source_language: String,
    pub target_language: String,
    pub lemma: String,
    pub slug: String,
}

impl CardKey {
    pub fn id(&self) -> String {
        card_id(&self.source_language, &self.target_language, &self.slug)
    }
}

// Card identity is the on-disk identity: same language pair, same lemma
// slug => same card. PoS is no longer part of identity — a single lemma
// card holds translations across all its grammatical roles, grouped by
// PoS inside the translations map. The `lemma` display string is
// first-write-wins metadata and does not participate in equality, since
// different surface variants slug identically and would otherwise
// collide on disk.
impl PartialEq for CardKey {
    fn eq(&self, other: &Self) -> bool {
        self.source_language == other.source_language
            && self.target_language == other.target_language
            && self.slug == other.slug
    }
}
impl Eq for CardKey {}

/// Display-form normalization: NFC + apostrophe + whitespace collapse,
/// preserving the LLM-emitted casing. Use for the lemma stored on
/// CardKey / Card (rendered to the user, sent to Anki Source field).
/// Pair with [`canonicalize_lemma`] + [`lemma_slug`] for the lowercased
/// filesystem/id side.
pub fn canonicalize_lemma_display(raw: &str) -> String {
    let nfc: String = raw.nfc().collect();
    let apostrophe_normalized: String = nfc
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

pub fn canonicalize_lemma(raw: &str, _src_lang: Language) -> String {
    // _src_lang is reserved for locale-aware lowercase (Turkish dotted/dotless `i`).
    // Stage 2 uses Unicode default. See .specs/ANKI_PLAN.md "Known follow-ups".
    canonicalize_lemma_display(raw).to_lowercase()
}

pub fn canonicalize_part_of_speech(raw: &str) -> String {
    let nfc: String = raw.nfc().collect();
    let lowered = nfc.to_lowercase();
    lowered.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Convert a canonicalized lemma or PoS string into a filesystem-safe
/// fragment. Lemmas and PoS values both flow into filenames and IDs and
/// both can carry surprises (multi-word lemmas like "darse cuenta", LLM
/// PoS noise like "noun (gerund/participle)"), so they share the same
/// rules: replace every whitespace and every separator-ish or
/// filesystem-unsafe character with `_`, collapse consecutive `_`, trim
/// leading/trailing `_`. Replacement (not removal) prevents two distinct
/// inputs from collapsing into the same slug (`a/b` and `ab` stay
/// distinct as `a_b` and `ab`).
fn fs_safe_slug(canonical: &str) -> String {
    let mapped: String = canonical
        .chars()
        .map(|c| {
            if c.is_whitespace()
                || matches!(
                    c,
                    '/' | '\\'
                        | ':'
                        | '*'
                        | '?'
                        | '"'
                        | '<'
                        | '>'
                        | '|'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | ','
                        | ';'
                )
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut out = String::with_capacity(mapped.len());
    let mut prev_underscore = true;
    for c in mapped.chars() {
        if c == '_' {
            if !prev_underscore {
                out.push('_');
                prev_underscore = true;
            }
        } else {
            out.push(c);
            prev_underscore = false;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

pub fn lemma_slug(canonical: &str) -> String {
    fs_safe_slug(canonical)
}

pub fn part_of_speech_slug(canonical: &str) -> String {
    fs_safe_slug(canonical)
}

pub fn card_id(source_language: &str, target_language: &str, lemma_slug: &str) -> String {
    format!("flts_{source_language}_{target_language}_{lemma_slug}")
}

/// Stitch a sentence's word list back into a human-readable source string.
///
/// Two transforms run together:
/// 1. **HTML entity decoding.** The translator prompt asks the LLM to encode
///    punctuation as entities (`comma` → `&comma;`, etc., see
///    [`crate::translator`]). Decode them back to literal characters so the
///    stored example matches what a learner reads in the book.
/// 2. **Punctuation-aware spacing.** Words come in as separate tokens; a
///    naive `join(" ")` produces `"towns , cities ."`. We suppress the
///    leading space when a token starts with closing/sentence punctuation,
///    and suppress the trailing space after a token that ends with opening
///    punctuation. Heuristic, not a full typographic engine — handles the
///    common Western-script cases.
pub fn render_example_source(words: &[translation_import::Word]) -> String {
    fn decode_entities(input: &str) -> String {
        decode(input.as_bytes())
            .to_string()
            .unwrap_or_else(|_| input.to_owned())
    }

    fn eats_leading_space(c: char) -> bool {
        matches!(
            c,
            ',' | '.'
                | ';'
                | ':'
                | '?'
                | '!'
                | ')'
                | ']'
                | '}'
                | '\''
                | '\u{2019}'
                | '\u{201D}'
                | '\u{2026}'
        )
    }

    fn ends_with_open_bracket(c: char) -> bool {
        matches!(c, '(' | '[' | '{' | '\u{2018}' | '\u{201C}')
    }

    let mut out = String::new();
    let mut suppress_next_space = false;
    for word in words {
        let decoded = decode_entities(&word.original);
        let starts_with_eat = decoded.chars().next().is_some_and(eats_leading_space);
        let want_space = !out.is_empty() && !starts_with_eat && !suppress_next_space;
        if want_space {
            out.push(' ');
        }
        out.push_str(&decoded);
        suppress_next_space = decoded.chars().last().is_some_and(ends_with_open_bracket);
    }
    out
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
    /// The grammatical-role bucket this update lands in. Multiple updates
    /// with the same `key` but different `part_of_speech` accumulate into
    /// the card's `translations` map as separate POS buckets.
    pub part_of_speech: String,
    pub translations: Vec<String>,
    pub example: Option<Example>,
}

impl Card {
    pub fn new_from_update(update: &CardUpdate) -> Self {
        let mut bucket: Vec<String> = Vec::with_capacity(update.translations.len());
        for t in &update.translations {
            if !bucket.contains(t) {
                bucket.push(t.clone());
            }
        }
        let mut translations: BTreeMap<String, Vec<String>> = BTreeMap::new();
        translations.insert(update.part_of_speech.clone(), bucket);
        let examples = update
            .example
            .as_ref()
            .map(|e| vec![e.clone()])
            .unwrap_or_default();
        Card {
            version: 2,
            id: update.key.id(),
            lemma: update.key.lemma.clone(),
            translations,
            examples,
            anki_data: None,
        }
    }

    pub fn apply_update(&mut self, update: &CardUpdate) {
        let bucket = self
            .translations
            .entry(update.part_of_speech.clone())
            .or_default();
        for t in &update.translations {
            if !bucket.contains(t) {
                bucket.push(t.clone());
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

    /// Flatten the per-PoS translation buckets into a single ordered, deduped
    /// list. Iteration order is the BTreeMap's PoS-key order; within each
    /// bucket, insertion order is preserved.
    pub fn translations_flat(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for bucket in self.translations.values() {
            for t in bucket {
                if !out.iter().any(|existing| existing == t) {
                    out.push(t.clone());
                }
            }
        }
        out
    }

    /// Merge `other` into `self` for cross-instance conflict reconciliation.
    /// Caller has already verified that `self.id == other.id` (i.e. both files
    /// address the same `(src, tgt, slug)` key). `self.anki_data` is kept;
    /// `other.anki_data` is discarded — it's a local cache, not authoritative
    /// across instances.
    pub fn merge(&mut self, other: Card) {
        for (pos, translations) in other.translations {
            let bucket = self.translations.entry(pos).or_default();
            for t in translations {
                if !bucket.contains(&t) {
                    bucket.push(t);
                }
            }
        }

        let mut combined: Vec<Example> = std::mem::take(&mut self.examples);
        for e in other.examples {
            if !combined.iter().any(|existing| {
                existing.book_id == e.book_id
                    && existing.chapter == e.chapter
                    && existing.paragraph == e.paragraph
                    && existing.source == e.source
                    && existing.translation == e.translation
            }) {
                combined.push(e);
            }
        }

        if combined.len() > EXAMPLES_CAP {
            combined.sort_by(|a, b| {
                a.book_id
                    .cmp(&b.book_id)
                    .then(a.chapter.cmp(&b.chapter))
                    .then(a.paragraph.cmp(&b.paragraph))
            });
            combined.truncate(EXAMPLES_CAP);
        }

        self.examples = combined;
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
        let source_text = render_example_source(&sentence.words);
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
            // Display form preserves the LLM-emitted casing (e.g. "Harry",
            // "Haus", "I") so the rendered card matches the citation form.
            // Slug uses the lowercased pipeline so the filesystem identity
            // and across-occurrence dedup stay stable.
            let lemma = canonicalize_lemma_display(&word.grammar.original_initial_form);
            if lemma.is_empty() {
                continue;
            }
            let slug = lemma_slug(&lemma.to_lowercase());
            if slug.is_empty() {
                continue;
            }
            let part_of_speech = canonicalize_part_of_speech(&word.grammar.part_of_speech);
            let pos_slug = part_of_speech_slug(&part_of_speech);
            if pos_slug.is_empty() {
                continue;
            }
            let key = CardKey {
                source_language: source_language.to_owned(),
                target_language: target_language.to_owned(),
                lemma,
                slug,
            };

            let target_dictionary = word.grammar.target_initial_form.trim();
            if target_dictionary.is_empty() {
                continue;
            }

            if let Some(existing) = updates
                .iter_mut()
                .find(|u| u.key == key && part_of_speech_slug(&u.part_of_speech) == pos_slug)
            {
                if !existing.translations.iter().any(|t| t == target_dictionary) {
                    existing.translations.push(target_dictionary.to_owned());
                }
            } else {
                updates.push(CardUpdate {
                    key,
                    part_of_speech,
                    translations: vec![target_dictionary.to_owned()],
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
    use crate::test_utils::{full_word, one_sentence_paragraph};

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
    fn canonicalize_lemma_display_preserves_case() {
        // Display normalization keeps the LLM-emitted casing while still
        // applying NFC, apostrophe normalization, and whitespace collapse.
        assert_eq!(canonicalize_lemma_display("Harry"), "Harry");
        assert_eq!(canonicalize_lemma_display("España"), "España");
        assert_eq!(canonicalize_lemma_display("Haus"), "Haus");
        assert_eq!(canonicalize_lemma_display("I"), "I");
        assert_eq!(canonicalize_lemma_display("  HARRY jr  "), "HARRY jr");
        // NFC composition still runs.
        assert_eq!(canonicalize_lemma_display("Espan\u{0303}a"), "España");
        // Curly apostrophe still normalized.
        assert_eq!(canonicalize_lemma_display("L\u{2019}Amour"), "L'Amour");
    }

    #[test]
    fn canonicalize_curly_apostrophe() {
        assert_eq!(canonicalize_lemma("l\u{2019}amour", spa()), "l'amour");
        assert_eq!(canonicalize_lemma("L\u{2018}AMOUR", spa()), "l'amour");
    }

    #[test]
    fn canonicalize_whitespace_collapse() {
        assert_eq!(canonicalize_lemma("darse  cuenta", spa()), "darse cuenta");
        assert_eq!(
            canonicalize_lemma("  darse\tcuenta  ", spa()),
            "darse cuenta"
        );
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
    fn slug_replaces_unsafe_chars_with_underscore() {
        // Unsafe chars become `_` (not dropped) so distinct inputs stay
        // distinct on disk and the resulting filename is readable.
        assert_eq!(lemma_slug("a/b\\c:d*e?f\"g<h>i|j"), "a_b_c_d_e_f_g_h_i_j");
    }

    #[test]
    fn lemma_slug_handles_noisy_input_like_pos_slug() {
        // Lemma and PoS share the same fs-safety helper, so the same noisy
        // inputs slug identically — no surprising filesystem failures on
        // either field.
        assert_eq!(lemma_slug("(foo)"), "foo");
        assert_eq!(lemma_slug("a / b"), "a_b");
        assert_eq!(lemma_slug("  spaced  out  "), "spaced_out");
    }

    #[test]
    fn slug_preserves_unicode() {
        assert_eq!(lemma_slug("café"), "café");
        assert_eq!(lemma_slug("мочь"), "мочь");
    }

    #[test]
    fn card_id_format() {
        assert_eq!(card_id("spa", "rus", "poder"), "flts_spa_rus_poder");
    }

    #[test]
    fn canonicalize_pos_lowercases_and_collapses_whitespace() {
        assert_eq!(canonicalize_part_of_speech("  VERB  "), "verb");
        assert_eq!(
            canonicalize_part_of_speech("Noun  /\tAdjective"),
            "noun / adjective"
        );
    }

    #[test]
    fn canonicalize_pos_nfc_roundtrip() {
        // "é" composed vs decomposed
        let composed = "verbe\u{00E9}";
        let decomposed = "verbe\u{0065}\u{0301}";
        assert_eq!(
            canonicalize_part_of_speech(composed),
            canonicalize_part_of_speech(decomposed)
        );
    }

    #[test]
    fn pos_slug_passthrough_on_clean_input() {
        assert_eq!(part_of_speech_slug("verb"), "verb");
        assert_eq!(part_of_speech_slug("noun"), "noun");
        assert_eq!(part_of_speech_slug("punct"), "punct");
    }

    #[test]
    fn pos_slug_replaces_slashes_and_parens() {
        // The exact noisy values the LLM emitted on the user's library.
        assert_eq!(
            part_of_speech_slug("существительное / прилагательное"),
            "существительное_прилагательное"
        );
        assert_eq!(
            part_of_speech_slug("герундий/причастие настоящего времени"),
            "герундий_причастие_настоящего_времени"
        );
        assert_eq!(
            part_of_speech_slug("глагол (герундий/причастие настоящего времени)"),
            "глагол_герундий_причастие_настоящего_времени"
        );
        assert_eq!(part_of_speech_slug("наречие/предлог"), "наречие_предлог");
        assert_eq!(
            part_of_speech_slug("предлог/частица инфинитива"),
            "предлог_частица_инфинитива"
        );
    }

    #[test]
    fn pos_slug_trims_leading_and_trailing_separators() {
        assert_eq!(part_of_speech_slug("/verb/"), "verb");
        assert_eq!(part_of_speech_slug("  verb  "), "verb");
        assert_eq!(part_of_speech_slug("___verb___"), "verb");
    }

    #[test]
    fn pos_slug_empty_on_pure_separators() {
        assert_eq!(part_of_speech_slug(""), "");
        assert_eq!(part_of_speech_slug("   "), "");
        assert_eq!(part_of_speech_slug("///"), "");
    }

    #[test]
    fn extract_card_updates_canonicalizes_noisy_pos() {
        // Two words with the same lemma but PoS strings that differ only
        // in noise: one canonicalizes the same as the other. The extract
        // pass dedups them under the same key.
        let paragraph = one_sentence_paragraph(
            "Хорошо.",
            vec![
                full_word(
                    "good",
                    "good",
                    "хорошо",
                    "Существительное / Прилагательное",
                    &["хорошо"],
                    false,
                ),
                full_word(
                    "good",
                    "good",
                    "хорошо",
                    "существительное /прилагательное ",
                    &["добро"],
                    false,
                ),
            ],
        );
        let updates = extract_card_updates(
            &paragraph,
            spa(),
            Language::from_639_3("rus").unwrap(),
            Uuid::new_v4(),
            0,
            0,
        );
        assert_eq!(updates.len(), 1);
        let update = &updates[0];
        assert_eq!(update.part_of_speech, "существительное / прилагательное");
        assert_eq!(update.key.id(), "flts_spa_rus_good");
        // Both source words share the same target_initial_form ("хорошо"),
        // so the deduped CardUpdate carries that single dictionary translation.
        assert_eq!(update.translations, vec!["хорошо"]);
    }

    #[test]
    fn extract_card_updates_skips_pos_that_slugs_to_empty() {
        let paragraph = one_sentence_paragraph(
            "Хорошо.",
            vec![full_word(
                "good",
                "good",
                "хорошо",
                "///",
                &["хорошо"],
                false,
            )],
        );
        let updates = extract_card_updates(
            &paragraph,
            spa(),
            Language::from_639_3("rus").unwrap(),
            Uuid::new_v4(),
            0,
            0,
        );
        assert!(
            updates.is_empty(),
            "expected pure-separator PoS to be skipped"
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
            assert!(
                is_eligible(&word(lemma, false)),
                "expected {lemma} eligible"
            );
        }
    }

    fn poder_key() -> CardKey {
        CardKey {
            source_language: "spa".into(),
            target_language: "rus".into(),
            lemma: "poder".into(),
            slug: "poder".into(),
        }
    }

    fn verb_update(translations: Vec<&str>, example: Option<Example>) -> CardUpdate {
        CardUpdate {
            key: poder_key(),
            part_of_speech: "verb".into(),
            translations: translations.into_iter().map(String::from).collect(),
            example,
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
    fn new_card_from_update_has_version_2_anki_data_null() {
        let update = verb_update(vec!["мочь"], Some(example_at(Uuid::nil(), 1, 2)));
        let card = Card::new_from_update(&update);
        assert_eq!(card.version, 2);
        assert_eq!(card.id, "flts_spa_rus_poder");
        assert_eq!(card.lemma, "poder");
        assert_eq!(card.translations_flat(), vec!["мочь"]);
        assert_eq!(card.translations.get("verb").unwrap().as_slice(), ["мочь"]);
        assert_eq!(card.examples.len(), 1);
        assert!(card.anki_data.is_none());
    }

    #[test]
    fn apply_update_adds_new_translation() {
        let mut card = Card::new_from_update(&verb_update(vec!["мочь"], None));
        card.apply_update(&verb_update(vec!["уметь"], None));
        assert_eq!(card.translations_flat(), vec!["мочь", "уметь"]);
    }

    #[test]
    fn apply_update_dedups_translation() {
        let mut card = Card::new_from_update(&verb_update(vec!["мочь"], None));
        card.apply_update(&verb_update(vec!["мочь", "уметь", "мочь"], None));
        assert_eq!(card.translations_flat(), vec!["мочь", "уметь"]);
    }

    #[test]
    fn apply_update_groups_translations_by_pos() {
        // Same lemma surfacing under two PoS values lands in two map
        // buckets and translations_flat() returns the deduped union.
        let mut card = Card::new_from_update(&verb_update(vec!["мочь"], None));
        card.apply_update(&CardUpdate {
            key: poder_key(),
            part_of_speech: "verb_auxiliary".into(),
            translations: vec!["мочь".into()],
            example: None,
        });
        assert_eq!(card.translations.len(), 2);
        assert_eq!(card.translations.get("verb").unwrap().as_slice(), ["мочь"]);
        assert_eq!(
            card.translations.get("verb_auxiliary").unwrap().as_slice(),
            ["мочь"]
        );
        // Dedup across buckets in the flat view.
        assert_eq!(card.translations_flat(), vec!["мочь"]);
    }

    #[test]
    fn apply_update_adds_new_example_with_distinct_provenance() {
        let book = Uuid::new_v4();
        let mut card = Card::new_from_update(&verb_update(vec![], Some(example_at(book, 1, 1))));
        card.apply_update(&verb_update(vec![], Some(example_at(book, 1, 2))));
        assert_eq!(card.examples.len(), 2);
    }

    #[test]
    fn apply_update_skips_example_with_same_provenance() {
        let book = Uuid::new_v4();
        let mut card = Card::new_from_update(&verb_update(vec![], Some(example_at(book, 1, 1))));
        card.apply_update(&verb_update(vec![], Some(example_at(book, 1, 1))));
        assert_eq!(card.examples.len(), 1);
    }

    #[test]
    fn apply_update_caps_examples_at_10() {
        let book = Uuid::new_v4();
        let mut card = Card::new_from_update(&verb_update(vec![], Some(example_at(book, 0, 0))));
        for i in 1..20 {
            card.apply_update(&verb_update(vec![], Some(example_at(book, 0, i))));
        }
        assert_eq!(card.examples.len(), EXAMPLES_CAP);
        // earliest-by-insertion retained
        assert_eq!(card.examples[0].paragraph, 0);
        assert_eq!(card.examples[9].paragraph, 9);
    }

    #[test]
    fn apply_update_handles_empty_translations_no_example() {
        let mut card = Card::new_from_update(&verb_update(vec!["мочь"], None));
        let before = card.clone();
        card.apply_update(&verb_update(vec![], None));
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
        let p = one_sentence_paragraph(".", vec![full_word(".", ".", ".", "punct", &[], true)]);
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
    fn walk_preserves_llm_casing_in_key_lemma() {
        let p = one_sentence_paragraph(
            "Mosca",
            vec![full_word(
                "España",
                "España",
                "Испания",
                "propn",
                &["Испания"],
                false,
            )],
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
        // Display lemma keeps the LLM-emitted casing.
        assert_eq!(updates[0].key.lemma, "España");
        // Slug is lowercased so the filesystem identity is stable.
        assert_eq!(updates[0].key.slug, "españa");
        assert_eq!(updates[0].key.id(), "flts_spa_rus_españa");
    }

    #[test]
    fn walk_preserves_proper_noun_casing_into_card_lemma() {
        // Round-trip: a `proper_noun` like "Harry" lands on Card.lemma as
        // "Harry" but the on-disk identity (slug, id) lowercases.
        let p = one_sentence_paragraph(
            "Harry caught the Snitch.",
            vec![full_word(
                "Harry",
                "Harry",
                "Гарри",
                "proper_noun",
                &["Гарри"],
                false,
            )],
        );
        let updates = extract_card_updates(
            &p,
            Language::from_639_3("eng").unwrap(),
            Language::from_639_3("rus").unwrap(),
            Uuid::nil(),
            0,
            0,
        );
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].key.lemma, "Harry");
        assert_eq!(updates[0].key.slug, "harry");

        let card = Card::new_from_update(&updates[0]);
        assert_eq!(card.lemma, "Harry");
        assert_eq!(card.id, "flts_eng_rus_harry");
    }

    #[test]
    fn walk_preserves_german_common_noun_casing() {
        // German common nouns are capitalized by orthographic rule; the
        // LLM emits them that way and we must preserve it in the display
        // lemma. Slug still lowercases.
        let p = one_sentence_paragraph(
            "Das Haus ist groß.",
            vec![
                full_word("Das", "der", "дом", "determiner_article", &["дом"], false),
                full_word("Haus", "Haus", "дом", "common_noun", &["дом"], false),
            ],
        );
        let updates = extract_card_updates(
            &p,
            Language::from_639_3("deu").unwrap(),
            Language::from_639_3("rus").unwrap(),
            Uuid::nil(),
            0,
            0,
        );
        let haus = updates
            .iter()
            .find(|u| u.key.slug == "haus")
            .expect("Haus update emitted");
        assert_eq!(haus.key.lemma, "Haus");
        assert_eq!(haus.key.slug, "haus");
    }

    #[test]
    fn render_example_source_decodes_entities_and_strips_pre_punctuation_space() {
        // Reproduces the bug reported on cards/eng-rus/across_preposition.json
        // where the LLM's "&comma;" / "&period;" punctuation tokens got joined
        // naively with spaces, producing "towns &comma; cities &comma; ... Midlands ."
        // instead of "towns, cities, ... Midlands."
        let words = vec![
            full_word("Militia", "Militia", "", "proper_noun", &[], false),
            full_word("and", "and", "", "coordinating_conjunction", &[], false),
            full_word("infantrymen", "infantryman", "", "common_noun", &[], false),
            full_word("sat", "sit", "", "verb", &[], false),
            full_word("menacingly", "menacingly", "", "adverb", &[], false),
            full_word("in", "in", "", "preposition", &[], false),
            full_word("towns", "town", "", "common_noun", &[], false),
            full_word("&comma;", ",", "", "punct", &[], true),
            full_word("cities", "city", "", "common_noun", &[], false),
            full_word("&comma;", ",", "", "punct", &[], true),
            full_word("and", "and", "", "coordinating_conjunction", &[], false),
            full_word("pubs", "pub", "", "common_noun", &[], false),
            full_word("across", "across", "", "preposition", &[], false),
            full_word("the", "the", "", "determiner", &[], false),
            full_word("Midlands", "Midlands", "", "proper_noun", &[], false),
            full_word("&period;", ".", "", "punct", &[], true),
        ];
        assert_eq!(
            render_example_source(&words),
            "Militia and infantrymen sat menacingly in towns, cities, and pubs across the Midlands."
        );
    }

    #[test]
    fn render_example_source_handles_opening_brackets_and_quotes() {
        let words = vec![
            full_word("He", "he", "", "pronoun", &[], false),
            full_word("said", "say", "", "verb", &[], false),
            full_word("&lpar;", "(", "", "punct", &[], true),
            full_word("loudly", "loudly", "", "adverb", &[], false),
            full_word("&rpar;", ")", "", "punct", &[], true),
            full_word("&period;", ".", "", "punct", &[], true),
        ];
        assert_eq!(render_example_source(&words), "He said (loudly).");
    }

    #[test]
    fn walk_carries_provenance() {
        let book = Uuid::new_v4();
        let p = one_sentence_paragraph(
            "Я могу.",
            vec![full_word(
                "puedo",
                "poder",
                "мочь",
                "verb",
                &["могу"],
                false,
            )],
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
            sentences: vec![
                translation_import::Sentence {
                    full_translation: "Я могу.".into(),
                    words: vec![full_word(
                        "puedo",
                        "poder",
                        "мочь",
                        "verb",
                        &["могу"],
                        false,
                    )],
                },
                translation_import::Sentence {
                    full_translation: "Я не могу.".into(),
                    words: vec![full_word(
                        "puedo",
                        "poder",
                        "мочь",
                        "verb",
                        &["умею"],
                        false,
                    )],
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
        // Both encounters share the same target_initial_form ("мочь"), so
        // the dedup keeps a single dictionary-form translation.
        assert_eq!(updates[0].translations, vec!["мочь"]);
        // example is from the first sentence the lemma appeared in
        assert_eq!(updates[0].example.as_ref().unwrap().translation, "Я могу.");
    }

    #[test]
    fn extract_card_updates_uses_target_initial_form_for_card_translation() {
        let p = one_sentence_paragraph(
            "Байрон стал свидетелем последствий оккупации.",
            vec![full_word(
                "witnessed",
                "witness",
                "быть свидетелем",
                "verb",
                &["свидетелем"],
                false,
            )],
        );
        let updates = extract_card_updates(
            &p,
            Language::from_639_3("eng").unwrap(),
            Language::from_639_3("rus").unwrap(),
            Uuid::nil(),
            0,
            0,
        );
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].translations, vec!["быть свидетелем"]);
    }

    #[test]
    fn extract_card_updates_ignores_contextual_translations() {
        // contextualTranslations are kept for in-text reader annotations
        // but must not bleed into card translations — those are
        // target_initial_form only.
        let p = one_sentence_paragraph(
            "Я могу.",
            vec![full_word(
                "puedo",
                "poder",
                "мочь",
                "verb",
                &["могу", "умею"],
                false,
            )],
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
        assert_eq!(updates[0].translations, vec!["мочь"]);
    }

    #[test]
    fn extract_card_updates_skips_word_with_empty_target_initial_form() {
        // No targetInitialForm → no useful dictionary translation, so no
        // card is emitted even if contextual_translations are present.
        let mut word = full_word("puedo", "poder", "", "verb", &["могу"], false);
        word.grammar.target_initial_form = "   ".into();
        let p = one_sentence_paragraph("Я могу.", vec![word]);
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

    fn make_card_with(
        translations: Vec<&str>,
        examples: Vec<Example>,
        anki_data: Option<AnkiData>,
    ) -> Card {
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        map.insert(
            "verb".into(),
            translations.into_iter().map(String::from).collect(),
        );
        Card {
            version: 2,
            id: "flts_spa_rus_poder".into(),
            lemma: "poder".into(),
            translations: map,
            examples,
            anki_data,
        }
    }

    fn example_with(
        book_id: Uuid,
        chapter: usize,
        paragraph: usize,
        source: &str,
        translation: &str,
    ) -> Example {
        Example {
            source: source.into(),
            translation: translation.into(),
            book_id,
            chapter,
            paragraph,
        }
    }

    #[test]
    fn merge_unions_translations() {
        let mut base = make_card_with(vec!["мочь"], vec![], None);
        let other = make_card_with(vec!["уметь"], vec![], None);
        base.merge(other);
        assert_eq!(base.translations_flat(), vec!["мочь", "уметь"]);
    }

    #[test]
    fn merge_dedups_translations() {
        let mut base = make_card_with(vec!["мочь", "уметь"], vec![], None);
        let other = make_card_with(vec!["мочь"], vec![], None);
        base.merge(other);
        assert_eq!(base.translations_flat(), vec!["мочь", "уметь"]);
    }

    #[test]
    fn merge_translations_order_base_first() {
        let mut base = make_card_with(vec!["a", "b"], vec![], None);
        let other = make_card_with(vec!["b", "c", "a"], vec![], None);
        base.merge(other);
        assert_eq!(base.translations_flat(), vec!["a", "b", "c"]);
    }

    #[test]
    fn merge_unions_examples_by_provenance() {
        let book = Uuid::new_v4();
        let base_ex = example_with(book, 1, 1, "s1", "t1");
        let other_ex = example_with(book, 1, 2, "s2", "t2");
        let mut base = make_card_with(vec![], vec![base_ex.clone()], None);
        let other = make_card_with(vec![], vec![other_ex.clone()], None);
        base.merge(other);
        assert_eq!(base.examples.len(), 2);
        assert!(base.examples.contains(&base_ex));
        assert!(base.examples.contains(&other_ex));
    }

    #[test]
    fn merge_dedups_examples_by_provenance_tuple() {
        let book = Uuid::new_v4();
        let ex_a = example_with(book, 1, 1, "s", "t");
        let ex_dup = example_with(book, 1, 1, "s", "t");
        let mut base = make_card_with(vec![], vec![ex_a], None);
        let other = make_card_with(vec![], vec![ex_dup], None);
        base.merge(other);
        assert_eq!(base.examples.len(), 1);
    }

    #[test]
    fn merge_preserves_examples_cap_at_10_via_sort_by_provenance() {
        let book = Uuid::new_v4();
        let base_examples: Vec<Example> = (10..16)
            .map(|p| example_with(book, 0, p, &format!("s{p}"), &format!("t{p}")))
            .collect();
        let other_examples: Vec<Example> = (0..10)
            .map(|p| example_with(book, 0, p, &format!("s{p}"), &format!("t{p}")))
            .collect();
        let mut base = make_card_with(vec![], base_examples, None);
        let other = make_card_with(vec![], other_examples, None);
        base.merge(other);
        assert_eq!(base.examples.len(), EXAMPLES_CAP);
        let paragraphs: Vec<usize> = base.examples.iter().map(|e| e.paragraph).collect();
        assert_eq!(paragraphs, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn merge_commutative_on_translations_and_examples() {
        let book = Uuid::new_v4();
        let a_examples: Vec<Example> = (10..16)
            .map(|p| example_with(book, 0, p, &format!("s{p}"), &format!("t{p}")))
            .collect();
        let b_examples: Vec<Example> = (0..10)
            .map(|p| example_with(book, 0, p, &format!("s{p}"), &format!("t{p}")))
            .collect();

        let mut ab = make_card_with(vec!["x", "y"], a_examples.clone(), None);
        ab.merge(make_card_with(vec!["y", "z"], b_examples.clone(), None));

        let mut ba = make_card_with(vec!["y", "z"], b_examples, None);
        ba.merge(make_card_with(vec!["x", "y"], a_examples, None));

        // Translations differ in order (base-first), so compare as sets.
        let ab_t: std::collections::HashSet<_> = ab.translations_flat().into_iter().collect();
        let ba_t: std::collections::HashSet<_> = ba.translations_flat().into_iter().collect();
        assert_eq!(ab_t, ba_t);

        // Examples are sort-and-truncated when over cap, so order is deterministic.
        assert_eq!(ab.examples, ba.examples);
    }

    #[test]
    fn merge_keeps_base_anki_data_discards_other() {
        let base_anki = AnkiData {
            state: AnkiState::Active,
            interval_days: Some(30.0),
            ease_factor: Some(2.5),
            fsrs_difficulty: None,
            fsrs_stability: None,
        };
        let other_anki = AnkiData {
            state: AnkiState::Suspended,
            interval_days: None,
            ease_factor: None,
            fsrs_difficulty: None,
            fsrs_stability: None,
        };
        let mut base = make_card_with(vec![], vec![], Some(base_anki.clone()));
        let other = make_card_with(vec![], vec![], Some(other_anki));
        base.merge(other);
        assert_eq!(base.anki_data, Some(base_anki));
    }

    #[test]
    fn card_round_trips_through_json() {
        let mut translations: BTreeMap<String, Vec<String>> = BTreeMap::new();
        translations.insert("verb".into(), vec!["мочь".into()]);
        let card = Card {
            version: 2,
            id: "flts_spa_rus_poder".into(),
            lemma: "poder".into(),
            translations,
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
        assert!(json.contains("\"version\":2"));
        assert!(json.contains("\"anki_data\":null"));
    }

    fn active(
        interval_days: Option<f64>,
        ease: Option<f64>,
        fsrs_d: Option<f64>,
        fsrs_s: Option<f64>,
    ) -> AnkiData {
        AnkiData {
            state: AnkiState::Active,
            interval_days,
            ease_factor: ease,
            fsrs_difficulty: fsrs_d,
            fsrs_stability: fsrs_s,
        }
    }

    fn approx(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.01,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn familiarity_zero_when_no_anki_data() {
        // Never-synced cards collapse to the lowest knowledge level so the
        // reader shows the underline + auto-overlay by default.
        approx(familiarity_from(None).unwrap(), 0.0);
    }

    #[test]
    fn familiarity_none_when_suspended() {
        let d = AnkiData {
            state: AnkiState::Suspended,
            ..active(Some(30.0), Some(2.5), None, None)
        };
        assert_eq!(familiarity_from(Some(&d)), None);
    }

    #[test]
    fn familiarity_none_when_deleted() {
        let d = AnkiData {
            state: AnkiState::Deleted,
            ..active(Some(30.0), Some(2.5), None, None)
        };
        assert_eq!(familiarity_from(Some(&d)), None);
    }

    #[test]
    fn familiarity_zero_for_active_with_no_retention() {
        let d = active(None, None, None, None);
        approx(familiarity_from(Some(&d)).unwrap(), 0.0);
    }

    #[test]
    fn familiarity_uses_interval_days_when_fsrs_absent() {
        let d = active(Some(7.0), Some(2.5), None, None);
        approx(familiarity_from(Some(&d)).unwrap(), 0.46);
    }

    #[test]
    fn familiarity_prefers_fsrs_stability_over_interval_days() {
        let d = active(Some(99.0), Some(2.5), None, Some(7.0));
        approx(familiarity_from(Some(&d)).unwrap(), 0.46);
    }

    #[test]
    fn familiarity_clamped_to_one_above_mature_days() {
        let d = active(Some(500.0), Some(2.5), None, None);
        approx(familiarity_from(Some(&d)).unwrap(), 1.0);
    }

    #[test]
    fn familiarity_clamped_to_zero_for_negative_interval() {
        let d = active(Some(-5.0), Some(2.5), None, None);
        approx(familiarity_from(Some(&d)).unwrap(), 0.0);
    }
}
