use htmlentity::entity::{ICodedDataTrait, decode};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ParagraphTranslation {
    #[serde(skip)]
    pub timestamp: u64,
    #[serde(rename = "s", alias = "sentences")]
    pub sentences: Vec<Sentence>,
    #[serde(skip)]
    pub total_tokens: Option<u64>,
}

impl ParagraphTranslation {
    /// Decode HTML entities in fields where they are not intentional.
    ///
    /// The prompt instructs the LLM to entity-encode punctuation only in
    /// `Word.original`, but some providers (notably DeepSeek in loose JSON
    /// mode) leak entities into other fields too. Left untreated, this
    /// produces duplicate cards (e.g. `qu&eacute;` and `qué` slugify
    /// differently). Run this once at the translator boundary.
    pub fn normalize_html_entities(&mut self) {
        for sentence in &mut self.sentences {
            decode_in_place(&mut sentence.full_translation);
            for word in &mut sentence.words {
                for ct in &mut word.contextual_translations {
                    decode_in_place(ct);
                }
                if let Some(note) = word.note.as_mut() {
                    decode_in_place(note);
                }
                let g = &mut word.grammar;
                decode_in_place(&mut g.original_initial_form);
                decode_in_place(&mut g.target_initial_form);
                decode_in_place(&mut g.part_of_speech);
                for opt in [
                    &mut g.plurality,
                    &mut g.person,
                    &mut g.tense,
                    &mut g.case,
                    &mut g.other,
                ] {
                    if let Some(s) = opt.as_mut() {
                        decode_in_place(s);
                    }
                }
            }
        }
    }
}

fn decode_in_place(s: &mut String) {
    if let Ok(decoded) = decode(s.as_bytes()).to_string()
        && decoded != *s
    {
        *s = decoded;
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Sentence {
    #[serde(rename = "ft", alias = "fullTranslation", alias = "full_translation")]
    pub full_translation: String,
    #[serde(rename = "wl", alias = "words")]
    pub words: Vec<Word>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Word {
    #[serde(rename = "o", alias = "original")]
    pub original: String,
    #[serde(
        rename = "t",
        alias = "contextualTranslations",
        alias = "contextual_translations",
        default
    )]
    pub contextual_translations: Vec<String>,
    #[serde(rename = "n", alias = "note", default)]
    pub note: Option<String>,
    // `default`: Gemini's relaxed schema lets non-punctuation words omit
    // `p` entirely (absent == false), saving ~4 output tokens per word.
    #[serde(
        rename = "p",
        alias = "isPunctuation",
        alias = "is_punctuation",
        default
    )]
    pub is_punctuation: bool,
    #[serde(rename = "g", alias = "grammar", default)]
    pub grammar: Grammar,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Grammar {
    #[serde(rename = "lf", alias = "originalInitialForm", alias = "original_initial_form")]
    pub original_initial_form: String,
    #[serde(rename = "lt", alias = "targetInitialForm", alias = "target_initial_form")]
    pub target_initial_form: String,
    #[serde(rename = "pos", alias = "partOfSpeech", alias = "part_of_speech")]
    pub part_of_speech: String,
    #[serde(rename = "pl", alias = "plurality", default)]
    pub plurality: Option<String>,
    #[serde(rename = "pe", alias = "person", default)]
    pub person: Option<String>,
    #[serde(rename = "te", alias = "tense", default)]
    pub tense: Option<String>,
    #[serde(rename = "ca", alias = "case", default)]
    pub case: Option<String>,
    #[serde(rename = "ot", alias = "other", default)]
    pub other: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grammar(original_initial: &str, target_initial: &str, pos: &str) -> Grammar {
        Grammar {
            original_initial_form: original_initial.to_owned(),
            target_initial_form: target_initial.to_owned(),
            part_of_speech: pos.to_owned(),
            plurality: None,
            person: None,
            tense: None,
            case: None,
            other: None,
        }
    }

    #[test]
    fn normalize_html_entities_decodes_lemma_and_translations() {
        let mut p = ParagraphTranslation {
            timestamp: 0,
            sentences: vec![Sentence {
                full_translation: "What&quest;".to_owned(),
                words: vec![Word {
                    original: "hello&comma;".to_owned(),
                    contextual_translations: vec!["what&quest;".to_owned()],
                    note: Some("see &sect;1".to_owned()),
                    is_punctuation: false,
                    grammar: grammar("qu&eacute;", "what&quest;", "noun"),
                }],
            }],
            total_tokens: None,
        };

        p.normalize_html_entities();

        let w = &p.sentences[0].words[0];
        assert_eq!(p.sentences[0].full_translation, "What?");
        assert_eq!(w.grammar.original_initial_form, "qué");
        assert_eq!(w.grammar.target_initial_form, "what?");
        assert_eq!(w.contextual_translations, vec!["what?".to_owned()]);
        assert_eq!(w.note.as_deref(), Some("see §1"));
        // `original` is intentionally left encoded; the prompt asks the LLM to
        // entity-encode punctuation there and `render_example_source` decodes
        // it at render time.
        assert_eq!(w.original, "hello&comma;");
    }

    #[test]
    fn serializes_with_short_keys() {
        let p = ParagraphTranslation {
            timestamp: 0,
            sentences: vec![Sentence {
                full_translation: "Hola".to_owned(),
                words: vec![Word {
                    original: "hi".to_owned(),
                    contextual_translations: vec!["hola".to_owned()],
                    note: Some("greeting".to_owned()),
                    is_punctuation: false,
                    grammar: grammar("hi", "hola", "interjection"),
                }],
            }],
            total_tokens: None,
        };

        let json = serde_json::to_string(&p).unwrap();
        // Short keys present.
        for key in [
            "\"s\"", "\"wl\"", "\"ft\"", "\"o\"", "\"t\"", "\"n\"", "\"p\"", "\"g\"", "\"lf\"",
            "\"lt\"", "\"pos\"",
        ] {
            assert!(json.contains(key), "expected short key {key} in {json}");
        }
        // Long names gone.
        for key in [
            "originalInitialForm",
            "targetInitialForm",
            "partOfSpeech",
            "contextualTranslations",
            "isPunctuation",
            "fullTranslation",
            "sentences",
        ] {
            assert!(!json.contains(key), "unexpected long key {key} in {json}");
        }
    }

    #[test]
    fn deserializes_word_without_punctuation_flag() {
        // Gemini's relaxed schema omits `p` for normal words; absence means
        // "not punctuation". Punctuation tokens still emit `p: true`.
        let json = r#"{
            "s": [{
                "ft": "Hola.",
                "wl": [
                    { "o": "hi", "t": ["hola"], "g": { "lf": "hi", "lt": "hola", "pos": "interjection" } },
                    { "o": "&period;", "p": true }
                ]
            }]
        }"#;
        let p: ParagraphTranslation = serde_json::from_str(json).unwrap();
        let words = &p.sentences[0].words;
        assert!(!words[0].is_punctuation);
        assert!(words[1].is_punctuation);
    }

    #[test]
    fn deserializes_legacy_camel_case() {
        // Output emitted by the LLM before the short-key migration.
        let legacy = r#"{
            "sentences": [{
                "fullTranslation": "Hola",
                "words": [{
                    "original": "hi",
                    "contextualTranslations": ["hola"],
                    "note": "greeting",
                    "isPunctuation": false,
                    "grammar": {
                        "originalInitialForm": "hi",
                        "targetInitialForm": "hola",
                        "partOfSpeech": "interjection",
                        "plurality": "", "person": "", "tense": "", "case": "", "other": ""
                    }
                }]
            }]
        }"#;
        let p: ParagraphTranslation = serde_json::from_str(legacy).unwrap();
        let w = &p.sentences[0].words[0];
        assert_eq!(w.original, "hi");
        assert_eq!(w.grammar.part_of_speech, "interjection");
        assert_eq!(w.contextual_translations, vec!["hola".to_owned()]);
    }

    #[test]
    fn deserializes_legacy_snake_case_cache_entry() {
        // Shape written to the on-disk cache before the migration.
        let legacy = r#"{
            "sentences": [{
                "full_translation": "Hola",
                "words": [{
                    "original": "hi",
                    "contextual_translations": ["hola"],
                    "note": null,
                    "is_punctuation": false,
                    "grammar": {
                        "original_initial_form": "hi",
                        "target_initial_form": "hola",
                        "part_of_speech": "interjection",
                        "plurality": null, "person": null, "tense": null, "case": null, "other": null
                    }
                }]
            }]
        }"#;
        let p: ParagraphTranslation = serde_json::from_str(legacy).unwrap();
        assert_eq!(p.sentences[0].full_translation, "Hola");
        assert_eq!(
            p.sentences[0].words[0].grammar.original_initial_form,
            "hi"
        );
    }

    #[test]
    fn deserializes_compact_payload_with_omitted_fields() {
        // Gemini-style compact output: a content word carrying only the
        // required fields, followed by a punctuation token that omits the
        // whole grammar block, translations, and note.
        let compact = r#"{
            "s": [{
                "ft": "Hola.",
                "wl": [
                    { "o": "hi", "t": ["hola"], "p": false,
                      "g": { "lf": "hi", "lt": "hola", "pos": "interjection" } },
                    { "o": "&period;", "p": true }
                ]
            }]
        }"#;
        let p: ParagraphTranslation = serde_json::from_str(compact).unwrap();
        let words = &p.sentences[0].words;
        // Content word: omitted grammar inflection fields default to None.
        assert_eq!(words[0].grammar.part_of_speech, "interjection");
        assert_eq!(words[0].grammar.plurality, None);
        assert_eq!(words[0].note, None);
        // Punctuation token: grammar/translations/note all defaulted.
        assert!(words[1].is_punctuation);
        assert_eq!(words[1].original, "&period;");
        assert!(words[1].contextual_translations.is_empty());
        assert_eq!(words[1].note, None);
        assert_eq!(words[1].grammar, Grammar::default());
    }

    #[test]
    fn normalize_html_entities_is_idempotent_on_clean_text() {
        let mut p = ParagraphTranslation {
            timestamp: 0,
            sentences: vec![Sentence {
                full_translation: "Already clean".to_owned(),
                words: vec![Word {
                    original: "hola".to_owned(),
                    contextual_translations: vec!["hello".to_owned()],
                    note: None,
                    is_punctuation: false,
                    grammar: grammar("qué", "what", "noun"),
                }],
            }],
            total_tokens: None,
        };
        let before = p.clone();
        p.normalize_html_entities();
        assert_eq!(p, before);
    }
}
