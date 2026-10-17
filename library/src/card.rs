use std::collections::{BTreeMap, VecDeque};

use htmlentity::entity::{ICodedDataTrait, decode};
use isolang::Language;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

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

/// Anki retention state as a `[0.0, 1.0]` familiarity scalar for the reader.
/// Unsynced means `Some(0.0)` (never studied); Suspended/Deleted mean `None`
/// (retired, so no underline or overlay). See `.specs/ANKI_UI.md`.
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

// Identity is the on-disk identity: same language pair + lemma slug = same
// card, PoS excluded. `lemma` is display metadata outside equality, since
// surface variants slug identically and would collide on disk.
impl PartialEq for CardKey {
    fn eq(&self, other: &Self) -> bool {
        self.source_language == other.source_language
            && self.target_language == other.target_language
            && self.slug == other.slug
    }
}
impl Eq for CardKey {}

/// NFC + apostrophe + whitespace collapse, keeping the emitted casing. Use for
/// the user-visible lemma; pair with [`canonicalize_lemma`] + [`lemma_slug`]
/// for the lowercased filesystem/id side.
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
    // _src_lang is reserved for locale-aware lowercase (Turkish dotless `i`).
    canonicalize_lemma_display(raw).to_lowercase()
}

pub fn canonicalize_part_of_speech(raw: &str) -> String {
    let nfc: String = raw.nfc().collect();
    let lowered = nfc.to_lowercase();
    lowered.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Filesystem-safe fragment for a lemma or PoS: unsafe and separator-ish
/// characters become `_`, runs collapse, edges trim. Replacing rather than
/// removing keeps `a/b` and `ab` distinct slugs.
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

/// Stitch a sentence's words back into a readable source string: HTML entities
/// (the prompt asks for them, see [`crate::translator`]) decode back to
/// literals, and spacing is suppressed around punctuation. The spacing rule is
/// a Western-script heuristic, not a typographic engine.
pub fn render_example_source<'a, I>(words: I) -> String
where
    I: IntoIterator<Item = &'a str>,
{
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
        let decoded = decode_entities(word);
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
    /// Updates sharing a `key` but differing here land in separate buckets.
    pub part_of_speech: String,
    pub translations: Vec<String>,
    pub example: Option<Example>,
}

/// Lexical fields needed to decide whether a word can become a card and to
/// build one. Borrowed so callers can pass views without cloning.
#[derive(Debug, Clone, Copy)]
pub struct CardWordFields<'a> {
    pub is_punctuation: bool,
    pub original_initial_form: &'a str,
    pub target_initial_form: &'a str,
    pub part_of_speech: &'a str,
}

/// Why a word cannot become a card. Mirrors the reasons the paragraph-wide
/// extractor used to skip words silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardSkipReason {
    Punctuation,
    EmptyLemma,
    PureDigits,
    EmptyTargetForm,
    EmptyPartOfSpeech,
}

impl CardSkipReason {
    pub fn message(self) -> &'static str {
        match self {
            CardSkipReason::Punctuation => "punctuation is not added to cards",
            CardSkipReason::EmptyLemma => "the word has no dictionary form",
            CardSkipReason::PureDigits => "numbers are not added to cards",
            CardSkipReason::EmptyTargetForm => "the translation has no dictionary form",
            CardSkipReason::EmptyPartOfSpeech => "the word has no part of speech",
        }
    }
}

impl std::fmt::Display for CardSkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

struct CompiledWord {
    lemma: String,
    slug: String,
    part_of_speech: String,
    target: String,
}

fn compile_word(fields: CardWordFields<'_>) -> Result<CompiledWord, CardSkipReason> {
    if fields.is_punctuation {
        return Err(CardSkipReason::Punctuation);
    }
    let raw_lemma = fields.original_initial_form.trim();
    if raw_lemma.is_empty() {
        return Err(CardSkipReason::EmptyLemma);
    }
    if is_pure_digit_lemma(raw_lemma) {
        return Err(CardSkipReason::PureDigits);
    }

    // Display form keeps the citation casing; the slug lowercases so
    // filesystem identity and cross-occurrence dedup stay stable.
    let lemma = canonicalize_lemma_display(raw_lemma);
    if lemma.is_empty() {
        return Err(CardSkipReason::EmptyLemma);
    }
    let slug = lemma_slug(&lemma.to_lowercase());
    if slug.is_empty() {
        return Err(CardSkipReason::EmptyLemma);
    }

    let part_of_speech = canonicalize_part_of_speech(fields.part_of_speech);
    if part_of_speech_slug(&part_of_speech).is_empty() {
        return Err(CardSkipReason::EmptyPartOfSpeech);
    }

    let target = fields.target_initial_form.trim();
    if target.is_empty() {
        return Err(CardSkipReason::EmptyTargetForm);
    }

    Ok(CompiledWord {
        lemma,
        slug,
        part_of_speech,
        target: target.to_owned(),
    })
}

/// Whether [`card_update_from_word`] would produce a card for these fields.
/// The reader uses this to decide whether to offer the add button.
pub fn card_word_eligibility(fields: CardWordFields<'_>) -> Result<(), CardSkipReason> {
    compile_word(fields).map(|_| ())
}

/// The on-disk identity for a lemma, used both to create a card and to answer
/// whether one already exists. `None` when the lemma slugs to nothing.
pub fn card_key_for_lemma(
    raw_lemma: &str,
    src_lang: Language,
    tgt_lang: Language,
) -> Option<CardKey> {
    let lemma = canonicalize_lemma_display(raw_lemma);
    let slug = lemma_slug(&lemma.to_lowercase());
    if lemma.is_empty() || slug.is_empty() {
        return None;
    }
    Some(CardKey {
        source_language: src_lang.to_639_3().to_owned(),
        target_language: tgt_lang.to_639_3().to_owned(),
        lemma,
        slug,
    })
}

/// Build a single card update from one word in one sentence.
pub fn card_update_from_word(
    fields: CardWordFields<'_>,
    example_source: &str,
    example_translation: &str,
    src_lang: Language,
    tgt_lang: Language,
    book_id: Uuid,
    chapter: usize,
    paragraph: usize,
) -> Result<CardUpdate, CardSkipReason> {
    let word = compile_word(fields)?;
    Ok(CardUpdate {
        key: CardKey {
            source_language: src_lang.to_639_3().to_owned(),
            target_language: tgt_lang.to_639_3().to_owned(),
            lemma: word.lemma,
            slug: word.slug,
        },
        part_of_speech: word.part_of_speech,
        translations: vec![word.target],
        example: Some(Example {
            source: example_source.to_owned(),
            translation: example_translation.to_owned(),
            book_id,
            chapter,
            paragraph,
        }),
    })
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

    /// Per-PoS buckets flattened and deduped, in PoS-key then insertion order.
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

    /// Merge `other` into `self`; the caller must have checked `self.id ==
    /// other.id`. `other.anki_data` is dropped: it's a local cache, not
    /// authoritative across instances.
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
            // Provenance order keeps the merge commutative; the round-robin
            // spreads the cap across books instead of draining the first.
            combined.sort_by(|a, b| {
                a.book_id
                    .cmp(&b.book_id)
                    .then(a.chapter.cmp(&b.chapter))
                    .then(a.paragraph.cmp(&b.paragraph))
            });
            let mut by_book: BTreeMap<Uuid, VecDeque<Example>> = BTreeMap::new();
            for e in combined {
                by_book.entry(e.book_id).or_default().push_back(e);
            }
            let mut picked: Vec<Example> = Vec::with_capacity(EXAMPLES_CAP);
            while picked.len() < EXAMPLES_CAP {
                let mut progressed = false;
                for group in by_book.values_mut() {
                    if let Some(e) = group.pop_front() {
                        picked.push(e);
                        progressed = true;
                        if picked.len() >= EXAMPLES_CAP {
                            break;
                        }
                    }
                }
                if !progressed {
                    break;
                }
            }
            combined = picked;
        }

        self.examples = combined;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::translation_import;
    use crate::test_utils::full_word;

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
        assert_eq!(canonicalize_lemma_display("Harry"), "Harry");
        assert_eq!(canonicalize_lemma_display("España"), "España");
        assert_eq!(canonicalize_lemma_display("Haus"), "Haus");
        assert_eq!(canonicalize_lemma_display("I"), "I");
        assert_eq!(canonicalize_lemma_display("  HARRY jr  "), "HARRY jr");
        assert_eq!(canonicalize_lemma_display("Espan\u{0303}a"), "España");
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
        assert_eq!(lemma_slug("a/b\\c:d*e?f\"g<h>i|j"), "a_b_c_d_e_f_g_h_i_j");
    }

    #[test]
    fn lemma_slug_handles_noisy_input_like_pos_slug() {
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
    fn card_update_from_word_canonicalizes_noisy_pos() {
        let word = full_word(
            "good",
            "good",
            "хорошо",
            "Существительное / Прилагательное",
            &["хорошо"],
            false,
        );
        let update = card_update_from_word(
            eligible_fields(&word),
            "good",
            "хорошо",
            spa(),
            Language::from_639_3("rus").unwrap(),
            Uuid::nil(),
            0,
            0,
        )
        .unwrap();
        assert_eq!(update.part_of_speech, "существительное / прилагательное");
        assert_eq!(update.key.id(), "flts_spa_rus_good");
        assert_eq!(update.translations, vec!["хорошо"]);
    }

    #[test]
    fn card_update_from_word_uses_target_initial_form() {
        // Contextual translations serve reader annotations only; cards carry
        // target_initial_form.
        let word = full_word(
            "witnessed",
            "witness",
            "быть свидетелем",
            "verb",
            &["свидетель"],
            false,
        );
        let update = card_update_from_word(
            eligible_fields(&word),
            "witnessed",
            "стал свидетелем",
            Language::from_639_3("eng").unwrap(),
            Language::from_639_3("rus").unwrap(),
            Uuid::nil(),
            0,
            0,
        )
        .unwrap();
        assert_eq!(update.translations, vec!["быть свидетелем"]);
    }

    #[test]
    fn card_word_eligibility_keeps_word_form_numeral() {
        for lemma in ["cinco", "пять", "fünf", "five"] {
            let word = full_word(lemma, lemma, "translation", "numeral", &[], false);
            assert_eq!(
                card_word_eligibility(eligible_fields(&word)),
                Ok(()),
                "expected {lemma} eligible"
            );
        }
    }

    fn eligible_fields<'a>(word: &'a translation_import::Word) -> CardWordFields<'a> {
        CardWordFields {
            is_punctuation: word.is_punctuation,
            original_initial_form: &word.grammar.original_initial_form,
            target_initial_form: &word.grammar.target_initial_form,
            part_of_speech: &word.grammar.part_of_speech,
        }
    }

    #[test]
    fn card_update_from_word_builds_one_card_with_citation_forms() {
        let word = full_word("puedo", "poder", "мочь", "verb", &["могу"], false);
        let update = card_update_from_word(
            eligible_fields(&word),
            "Puedo entrar.",
            "Я могу войти.",
            spa(),
            Language::from_639_3("rus").unwrap(),
            Uuid::nil(),
            3,
            7,
        )
        .unwrap();

        assert_eq!(update.key.id(), "flts_spa_rus_poder");
        assert_eq!(update.key.lemma, "poder");
        assert_eq!(update.part_of_speech, "verb");
        assert_eq!(update.translations, vec!["мочь"]);
        let example = update.example.unwrap();
        assert_eq!(example.source, "Puedo entrar.");
        assert_eq!(example.translation, "Я могу войти.");
        assert_eq!(example.book_id, Uuid::nil());
        assert_eq!(example.chapter, 3);
        assert_eq!(example.paragraph, 7);
    }

    #[test]
    fn card_word_eligibility_reports_each_skip_reason() {
        let base = CardWordFields {
            is_punctuation: false,
            original_initial_form: "poder",
            target_initial_form: "мочь",
            part_of_speech: "verb",
        };
        assert_eq!(card_word_eligibility(base), Ok(()));

        assert_eq!(
            card_word_eligibility(CardWordFields {
                is_punctuation: true,
                ..base
            }),
            Err(CardSkipReason::Punctuation)
        );
        assert_eq!(
            card_word_eligibility(CardWordFields {
                original_initial_form: "   ",
                ..base
            }),
            Err(CardSkipReason::EmptyLemma)
        );
        assert_eq!(
            card_word_eligibility(CardWordFields {
                original_initial_form: "42",
                ..base
            }),
            Err(CardSkipReason::PureDigits)
        );
        assert_eq!(
            card_word_eligibility(CardWordFields {
                target_initial_form: "   ",
                ..base
            }),
            Err(CardSkipReason::EmptyTargetForm)
        );
        assert_eq!(
            card_word_eligibility(CardWordFields {
                part_of_speech: "///",
                ..base
            }),
            Err(CardSkipReason::EmptyPartOfSpeech)
        );
    }

    #[test]
    fn card_key_for_lemma_matches_creation_identity() {
        let word = full_word("España", "España", "Испания", "propn", &[], false);
        let update = card_update_from_word(
            eligible_fields(&word),
            "España",
            "Испания",
            Language::from_639_3("spa").unwrap(),
            Language::from_639_3("rus").unwrap(),
            Uuid::nil(),
            0,
            0,
        )
        .unwrap();
        let key = card_key_for_lemma(
            "España",
            Language::from_639_3("spa").unwrap(),
            Language::from_639_3("rus").unwrap(),
        )
        .unwrap();

        assert_eq!(key, update.key);
        assert_eq!(key.lemma, "España");
        assert_eq!(key.slug, "españa");
    }

    #[test]
    fn card_key_for_lemma_rejects_unsluggable_lemma() {
        assert!(
            card_key_for_lemma(
                "///",
                Language::from_639_3("spa").unwrap(),
                Language::from_639_3("rus").unwrap(),
            )
            .is_none()
        );
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
    fn render_example_source_decodes_entities_and_strips_pre_punctuation_space() {
        // Entity punctuation tokens must not join naively with spaces.
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
            render_example_source(words.iter().map(|w| w.original.as_str())),
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
        assert_eq!(
            render_example_source(words.iter().map(|w| w.original.as_str())),
            "He said (loudly)."
        );
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
    fn merge_spreads_retained_examples_across_books() {
        // The retained set must draw from both books, not just the first.
        let book_a = Uuid::new_v4();
        let book_b = Uuid::new_v4();
        let a_examples: Vec<Example> = (0..8)
            .map(|p| example_with(book_a, 0, p, &format!("a{p}"), &format!("ta{p}")))
            .collect();
        let b_examples: Vec<Example> = (0..8)
            .map(|p| example_with(book_b, 0, p, &format!("b{p}"), &format!("tb{p}")))
            .collect();
        let mut base = make_card_with(vec![], a_examples, None);
        base.merge(make_card_with(vec![], b_examples, None));

        assert_eq!(base.examples.len(), EXAMPLES_CAP);
        let from_a = base.examples.iter().filter(|e| e.book_id == book_a).count();
        let from_b = base.examples.iter().filter(|e| e.book_id == book_b).count();
        assert_eq!(
            from_a, 5,
            "expected an even spread across books, got {from_a} from A"
        );
        assert_eq!(
            from_b, 5,
            "expected an even spread across books, got {from_b} from B"
        );
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

        let ab_t: std::collections::HashSet<_> = ab.translations_flat().into_iter().collect();
        let ba_t: std::collections::HashSet<_> = ba.translations_flat().into_iter().collect();
        assert_eq!(ab_t, ba_t);

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
