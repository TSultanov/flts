use std::{borrow::Cow, iter};

use super::soa_helpers::*;

pub struct Translation {
    pub source_language: String,
    pub target_language: String,

    strings: Vec<u8>,

    paragraphs: Vec<Option<usize>>,
    paragraph_translations: Vec<ParagraphTranslation>,
    sentences: Vec<Sentence>,
    words: Vec<Word>,
    word_contextual_translations: Vec<WordContextualTranslation>,
}

struct ParagraphTranslation {
    timestamp: usize,
    previous_version: Option<usize>,
    sentences: VecSlice<Sentence>,
}

pub struct ParagraphTranslationView<'a> {
    translation: &'a Translation,
    timestamp: usize,
    previous_version: Option<usize>,
    sentences: &'a [Sentence],
}

struct Sentence {
    full_translation: VecSlice<u8>,
    words: VecSlice<Word>,
}

pub struct SentenceView<'a> {
    translation: &'a Translation,
    pub full_translation: Cow<'a, str>,
    words: &'a [Word],
}

struct Word {
    original: VecSlice<u8>,
    contextual_translations: VecSlice<WordContextualTranslation>,
    note: VecSlice<u8>,
    grammar: Grammar,
}

struct Grammar {
    original_initial_form: VecSlice<u8>,
    target_initial_form: VecSlice<u8>,
    part_of_speech: VecSlice<u8>,
    plurality: Option<VecSlice<u8>>,
    person: Option<VecSlice<u8>>,
    tense: Option<VecSlice<u8>>,
    case: Option<VecSlice<u8>>,
    other: Option<VecSlice<u8>>,
}

pub struct WordView<'a> {
    translation: &'a Translation,
    pub original: Cow<'a, str>,
    pub note: Cow<'a, str>,
    pub grammar: GrammarView<'a>,
    contextual_translations: &'a [WordContextualTranslation],
}

pub struct GrammarView<'a> {
    original_initial_form: Cow<'a, str>,
    target_initial_form: Cow<'a, str>,
    part_of_speech: Cow<'a, str>,
    plurality: Option<Cow<'a, str>>,
    person: Option<Cow<'a, str>>,
    tense: Option<Cow<'a, str>>,
    case: Option<Cow<'a, str>>,
    other: Option<Cow<'a, str>>,
}

struct WordContextualTranslation {
    translation: VecSlice<u8>,
}

pub struct WordContextualTranslationView<'a> {
    pub translation: Cow<'a, str>,
}

impl Translation {
    pub fn paragraph_view(&self, paragraph: usize) -> Option<ParagraphTranslationView> {
        let paragraph = self.paragraphs[paragraph];
        let paragraph = paragraph.map(|p| &self.paragraph_translations[p]);
        paragraph.map(|p| ParagraphTranslationView {
            translation: self,
            timestamp: p.timestamp,
            previous_version: p.previous_version,
            sentences: p.sentences.slice(&self.sentences),
        })
    }

    pub fn add_paragraph_translation(&mut self, paragraph_index: usize, timestamp: usize) {
        if paragraph_index >= self.paragraphs.len() {
            self.paragraphs.extend(iter::repeat(None).take(paragraph_index - self.paragraphs.len() + 1));
        }

        let new_prev_version = self.paragraphs[paragraph_index];

        let new_paragraph = ParagraphTranslation {
            timestamp: timestamp,
            previous_version: new_prev_version,
            sentences: VecSlice::empty(),
        };
        let new_index = self.paragraph_translations.len();
        self.paragraph_translations.push(new_paragraph);
        self.paragraphs[paragraph_index] = Some(new_index);
    }
}

impl<'a> ParagraphTranslationView<'a> {
    pub fn get_previous_version(&self) -> Option<ParagraphTranslationView> {
        let paragraph = self
            .previous_version
            .map(|p| &self.translation.paragraph_translations[p]);
        paragraph.map(|p| ParagraphTranslationView {
            translation: self.translation,
            timestamp: p.timestamp,
            previous_version: p.previous_version,
            sentences: p.sentences.slice(&self.translation.sentences),
        })
    }

    pub fn sentence_count(&self) -> usize {
        self.sentences.len()
    }

    pub fn sentence_view(&self, sentence: usize) -> SentenceView {
        let sentence = &self.sentences[sentence];
        SentenceView {
            translation: self.translation,
            full_translation: String::from_utf8_lossy(
                sentence.full_translation.slice(&self.translation.strings),
            ),
            words: sentence.words.slice(&self.translation.words),
        }
    }
}

impl<'a> SentenceView<'a> {
    pub fn word_count(&self) -> usize {
        self.words.len()
    }

    pub fn word_view(&self, word: usize) -> WordView {
        let word = &self.words[word];
        WordView {
            translation: self.translation,
            original: String::from_utf8_lossy(word.original.slice(&self.translation.strings)),
            note: String::from_utf8_lossy(word.note.slice(&self.translation.strings)),
            grammar: GrammarView {
                original_initial_form: String::from_utf8_lossy(word.grammar.original_initial_form.slice(&self.translation.strings)),
                target_initial_form: String::from_utf8_lossy(word.grammar.target_initial_form.slice(&self.translation.strings)),
                part_of_speech: String::from_utf8_lossy(word.grammar.part_of_speech.slice(&self.translation.strings)),
                plurality: word.grammar.plurality.map(|s| String::from_utf8_lossy(s.slice(&self.translation.strings))),
                person: word.grammar.person.map(|s| String::from_utf8_lossy(s.slice(&self.translation.strings))),
                tense: word.grammar.tense.map(|s| String::from_utf8_lossy(s.slice(&self.translation.strings))),
                case: word.grammar.case.map(|s| String::from_utf8_lossy(s.slice(&self.translation.strings))),
                other: word.grammar.other.map(|s| String::from_utf8_lossy(s.slice(&self.translation.strings))),
            },
            contextual_translations: word
                .contextual_translations
                .slice(&self.translation.word_contextual_translations),
        }
    }
}

impl<'a> WordView<'a> {
    pub fn contextual_translations_count(&self) -> usize {
        self.contextual_translations.len()
    }

    pub fn contextual_translations_view(&self, index: usize) -> WordContextualTranslationView {
        let contextual_translation = &self.contextual_translations[index];
        WordContextualTranslationView {
            translation: String::from_utf8_lossy(
                contextual_translation
                    .translation
                    .slice(&self.translation.strings),
            ),
        }
    }
}
