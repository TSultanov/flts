//! A paragraph translation as it is stored: what drawing the paragraph takes,
//! and apart from it what only a look at one word takes.

use serde::{Deserialize, Serialize};

use super::translation_import::{Grammar, ParagraphTranslation, Sentence, Word};

/// What drawing a translated paragraph takes.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ParagraphWords {
    #[serde(rename = "s")]
    pub sentences: Vec<SentenceWords>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct SentenceWords {
    #[serde(rename = "w")]
    pub words: Vec<WordGloss>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct WordGloss {
    #[serde(rename = "o")]
    pub original: String,
    #[serde(rename = "l", default, skip_serializing_if = "String::is_empty")]
    pub lemma: String,
    /// The first contextual translation.
    #[serde(rename = "t", default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<String>,
    #[serde(rename = "p", default, skip_serializing_if = "std::ops::Not::not")]
    pub is_punctuation: bool,
}

impl ParagraphWords {
    pub fn of(translation: &ParagraphTranslation) -> Self {
        let sentences = translation.sentences.iter().map(|sentence| SentenceWords {
            words: sentence
                .words
                .iter()
                .map(|word| WordGloss {
                    original: word.original.clone(),
                    lemma: word.grammar.original_initial_form.clone(),
                    translation: word.contextual_translations.first().cloned(),
                    is_punctuation: word.is_punctuation,
                })
                .collect(),
        });
        Self {
            sentences: sentences.collect(),
        }
    }
}

impl SentenceWords {
    pub fn source_text(&self) -> String {
        crate::card::render_example_source(self.words.iter().map(|w| w.original.as_str()))
    }
}

/// The rest of the translation, in step with `ParagraphWords`.
#[derive(Serialize, Deserialize, Default)]
struct ParagraphDetails {
    #[serde(rename = "s")]
    sentences: Vec<SentenceDetails>,
}

#[derive(Serialize, Deserialize, Default)]
struct SentenceDetails {
    #[serde(rename = "ft")]
    full_translation: String,
    #[serde(rename = "w")]
    words: Vec<WordDetails>,
}

#[derive(Serialize, Deserialize, Default)]
struct WordDetails {
    /// Contextual translations after the first.
    #[serde(rename = "t", default, skip_serializing_if = "Vec::is_empty")]
    more_translations: Vec<String>,
    #[serde(rename = "n", default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(rename = "lt", default, skip_serializing_if = "String::is_empty")]
    target_initial_form: String,
    #[serde(rename = "pos", default, skip_serializing_if = "String::is_empty")]
    part_of_speech: String,
    #[serde(rename = "pl", default, skip_serializing_if = "Option::is_none")]
    plurality: Option<String>,
    #[serde(rename = "pe", default, skip_serializing_if = "Option::is_none")]
    person: Option<String>,
    #[serde(rename = "te", default, skip_serializing_if = "Option::is_none")]
    tense: Option<String>,
    #[serde(rename = "ca", default, skip_serializing_if = "Option::is_none")]
    case: Option<String>,
    #[serde(rename = "ot", default, skip_serializing_if = "Option::is_none")]
    other: Option<String>,
}

/// The two stored halves of `translation`: words, then details.
pub fn split(translation: &ParagraphTranslation) -> serde_json::Result<(Vec<u8>, Vec<u8>)> {
    let words = ParagraphWords::of(translation);
    let mut details = ParagraphDetails::default();
    for sentence in &translation.sentences {
        let rest = sentence.words.iter().map(|word| {
            let grammar = &word.grammar;
            WordDetails {
                more_translations: word
                    .contextual_translations
                    .iter()
                    .skip(1)
                    .cloned()
                    .collect(),
                note: word.note.clone(),
                target_initial_form: grammar.target_initial_form.clone(),
                part_of_speech: grammar.part_of_speech.clone(),
                plurality: grammar.plurality.clone(),
                person: grammar.person.clone(),
                tense: grammar.tense.clone(),
                case: grammar.case.clone(),
                other: grammar.other.clone(),
            }
        });
        details.sentences.push(SentenceDetails {
            full_translation: sentence.full_translation.clone(),
            words: rest.collect(),
        });
    }
    Ok((serde_json::to_vec(&words)?, serde_json::to_vec(&details)?))
}

pub fn read_words(words: &[u8]) -> serde_json::Result<ParagraphWords> {
    serde_json::from_slice(words)
}

/// The translation `split` took apart; its stamp and token count are the caller's to set.
pub fn join(words: &[u8], details: &[u8]) -> anyhow::Result<ParagraphTranslation> {
    let words = read_words(words)?;
    let details: ParagraphDetails = serde_json::from_slice(details)?;
    anyhow::ensure!(
        words.sentences.len() == details.sentences.len(),
        "the halves of a translation disagree"
    );
    let mut sentences = Vec::with_capacity(words.sentences.len());
    for (glosses, rest) in words.sentences.into_iter().zip(details.sentences) {
        anyhow::ensure!(
            glosses.words.len() == rest.words.len(),
            "the halves of a translation disagree"
        );
        let words = glosses
            .words
            .into_iter()
            .zip(rest.words)
            .map(|(gloss, rest)| Word {
                original: gloss.original,
                contextual_translations: gloss
                    .translation
                    .into_iter()
                    .chain(rest.more_translations)
                    .collect(),
                note: rest.note,
                is_punctuation: gloss.is_punctuation,
                grammar: Grammar {
                    original_initial_form: gloss.lemma,
                    target_initial_form: rest.target_initial_form,
                    part_of_speech: rest.part_of_speech,
                    plurality: rest.plurality,
                    person: rest.person,
                    tense: rest.tense,
                    case: rest.case,
                    other: rest.other,
                },
            })
            .collect();
        sentences.push(Sentence {
            full_translation: rest.full_translation,
            words,
        });
    }
    Ok(ParagraphTranslation {
        timestamp: 0,
        sentences,
        total_tokens: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn text() -> impl Strategy<Value = String> {
        "[a-zé\"<& ]{0,6}"
    }

    fn maybe() -> impl Strategy<Value = Option<String>> {
        proptest::option::of(text())
    }

    fn word() -> impl Strategy<Value = Word> {
        (
            (text(), proptest::collection::vec(text(), 0..4), maybe()),
            any::<bool>(),
            (text(), text(), text()),
            (maybe(), maybe(), maybe(), maybe(), maybe()),
        )
            .prop_map(
                |((original, translations, note), punctuation, forms, rest)| Word {
                    original,
                    contextual_translations: translations,
                    note,
                    is_punctuation: punctuation,
                    grammar: Grammar {
                        original_initial_form: forms.0,
                        target_initial_form: forms.1,
                        part_of_speech: forms.2,
                        plurality: rest.0,
                        person: rest.1,
                        tense: rest.2,
                        case: rest.3,
                        other: rest.4,
                    },
                },
            )
    }

    fn translation() -> impl Strategy<Value = ParagraphTranslation> {
        proptest::collection::vec(
            (text(), proptest::collection::vec(word(), 0..5)).prop_map(|(full, words)| Sentence {
                full_translation: full,
                words,
            }),
            0..4,
        )
        .prop_map(|sentences| ParagraphTranslation {
            timestamp: 0,
            sentences,
            total_tokens: None,
        })
    }

    proptest! {
        #[test]
        fn splitting_loses_nothing(translation in translation()) {
            let (words, details) = split(&translation).unwrap();
            prop_assert_eq!(join(&words, &details).unwrap(), translation);
        }

        #[test]
        fn the_words_half_draws_the_paragraph(translation in translation()) {
            let (words, _) = split(&translation).unwrap();
            let words = read_words(&words).unwrap();
            for (drawn, sentence) in words.sentences.iter().zip(&translation.sentences) {
                for (gloss, word) in drawn.words.iter().zip(&sentence.words) {
                    prop_assert_eq!(&gloss.original, &word.original);
                    prop_assert_eq!(&gloss.lemma, &word.grammar.original_initial_form);
                    prop_assert_eq!(gloss.translation.as_ref(), word.contextual_translations.first());
                    prop_assert_eq!(gloss.is_punctuation, word.is_punctuation);
                }
            }
        }
    }

    #[test]
    fn halves_that_disagree_are_refused() {
        let one = ParagraphTranslation {
            timestamp: 0,
            sentences: vec![Sentence {
                full_translation: "a".into(),
                words: vec![],
            }],
            total_tokens: None,
        };
        let none = ParagraphTranslation {
            timestamp: 0,
            sentences: vec![],
            total_tokens: None,
        };
        let (words, _) = split(&one).unwrap();
        let (_, details) = split(&none).unwrap();
        assert!(join(&words, &details).is_err());
    }
}
