use htmlentity::entity::{ICodedDataTrait, decode};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ParagraphTranslation {
    #[serde(skip)]
    pub timestamp: u64,
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
    #[serde(alias = "fullTranslation")]
    pub full_translation: String,
    pub words: Vec<Word>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Word {
    pub original: String,
    #[serde(alias = "contextualTranslations")]
    pub contextual_translations: Vec<String>,
    pub note: Option<String>,
    #[serde(alias = "isPunctuation")]
    pub is_punctuation: bool,
    pub grammar: Grammar,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
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
