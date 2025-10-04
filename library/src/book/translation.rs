use uuid::Uuid;

use crate::book::{
    serialization::{
        ChecksumedWriter, Magic, Serializable, Version, read_exact_array, read_len_prefixed_string,
        read_len_prefixed_vec, read_opt, read_u8, read_u64, read_var_u64, read_vec_slice,
        validate_hash, write_len_prefixed_bytes, write_opt, write_u64, write_var_u64,
        write_vec_slice,
    },
    translation_import,
};
use std::{borrow::Cow, iter};
use std::{
    collections::HashSet,
    io::{self, Read, Write},
};

use super::soa_helpers::*;

pub struct Translation {
    pub id: Uuid,
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
    idx: usize,
    translation: &'a Translation,
    pub timestamp: usize,
    previous_version: Option<usize>,
    sentences: &'a [Sentence],
}

#[derive(Clone)]
struct Sentence {
    full_translation: VecSlice<u8>,
    words: VecSlice<Word>,
}

pub struct SentenceView<'a> {
    translation: &'a Translation,
    pub full_translation: Cow<'a, str>,
    words: &'a [Word],
}

#[derive(Clone)]
struct Word {
    original: VecSlice<u8>,
    contextual_translations: VecSlice<WordContextualTranslation>,
    is_punctuation: bool,
    note: VecSlice<u8>,
    grammar: Grammar,
}

#[derive(Clone)]
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
    pub is_punctuation: bool,
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

#[derive(Clone)]
struct WordContextualTranslation {
    translation: VecSlice<u8>,
}

pub struct WordContextualTranslationView<'a> {
    pub translation: Cow<'a, str>,
}

impl Translation {
    pub fn create(source_language: &str, target_language: &str) -> Self {
        Translation {
            id: Uuid::new_v4(),
            source_language: source_language.to_owned(),
            target_language: target_language.to_owned(),
            strings: vec![],
            paragraphs: vec![],
            paragraph_translations: vec![],
            sentences: vec![],
            words: vec![],
            word_contextual_translations: vec![],
        }
    }

    pub fn paragraph_view(&'_ self, paragraph: usize) -> Option<ParagraphTranslationView<'_>> {
        if paragraph >= self.paragraphs.len() {
            return None;
        }
        let paragraph = self.paragraphs[paragraph];
        let paragraph = paragraph.map(|p| (p, &self.paragraph_translations[p]));
        paragraph.map(|(idx, p)| ParagraphTranslationView {
            idx,
            translation: self,
            timestamp: p.timestamp,
            previous_version: p.previous_version,
            sentences: p.sentences.slice(&self.sentences),
        })
    }

    pub fn translated_paragraphs_count(&self) -> usize {
        self.paragraphs.iter().filter(|p| p.is_some()).count()
    }

    pub fn add_paragraph_translation(
        &mut self,
        paragraph_index: usize,
        translation: &translation_import::ParagraphTranslation,
    ) {
        if paragraph_index >= self.paragraphs.len() {
            self.paragraphs
                .extend(iter::repeat(None).take(paragraph_index - self.paragraphs.len() + 1));
        }

        let new_prev_version = self.paragraphs[paragraph_index];

        let new_paragraph = ParagraphTranslation {
            timestamp: translation.timestamp,
            previous_version: new_prev_version,
            sentences: VecSlice::empty(),
        };
        let new_index = self.paragraph_translations.len();
        self.paragraph_translations.push(new_paragraph);
        self.paragraphs[paragraph_index] = Some(new_index);

        let mut sentences = VecSlice::empty();
        for sentence in &translation.sentences {
            let full_translation = push_string(&mut self.strings, &sentence.full_translation);
            let mut words = VecSlice::empty();
            for word in &sentence.words {
                let original = push_string(&mut self.strings, &word.original);
                let note = push_string(&mut self.strings, &word.note);
                let grammar = Grammar {
                    original_initial_form: push_string(
                        &mut self.strings,
                        &word.grammar.original_initial_form,
                    ),
                    target_initial_form: push_string(
                        &mut self.strings,
                        &word.grammar.target_initial_form,
                    ),
                    part_of_speech: push_string(&mut self.strings, &word.grammar.part_of_speech),
                    plurality: word
                        .grammar
                        .plurality
                        .as_ref()
                        .map(|s| push_string(&mut self.strings, s)),
                    person: word
                        .grammar
                        .person
                        .as_ref()
                        .map(|s| push_string(&mut self.strings, s)),
                    tense: word
                        .grammar
                        .tense
                        .as_ref()
                        .map(|s| push_string(&mut self.strings, s)),
                    case: word
                        .grammar
                        .case
                        .as_ref()
                        .map(|s| push_string(&mut self.strings, s)),
                    other: word
                        .grammar
                        .other
                        .as_ref()
                        .map(|s| push_string(&mut self.strings, s)),
                };
                let mut contextual_translations = VecSlice::empty();
                for contextual_translation in &word.contextual_translations {
                    let contextual_translation = WordContextualTranslation {
                        translation: push_string(&mut self.strings, contextual_translation),
                    };
                    contextual_translations = push(
                        &mut self.word_contextual_translations,
                        &contextual_translations,
                        contextual_translation,
                    )
                    .unwrap();
                }
                let new_word = Word {
                    original,
                    contextual_translations,
                    is_punctuation: word.is_punctuation,
                    note,
                    grammar,
                };
                words = push(&mut self.words, &words, new_word).unwrap();
            }
            let new_sentence = Sentence {
                full_translation,
                words,
            };
            sentences = push(&mut self.sentences, &sentences, new_sentence).unwrap();
        }

        self.paragraph_translations[new_index].sentences = sentences;
    }

    fn add_paragraph_translation_from_view(
        &mut self,
        paragraph_index: usize,
        translation: &ParagraphTranslationView,
    ) {
        if paragraph_index >= self.paragraphs.len() {
            self.paragraphs
                .extend(iter::repeat(None).take(paragraph_index - self.paragraphs.len() + 1));
        }

        let new_prev_version = self.paragraphs[paragraph_index];

        let new_paragraph = ParagraphTranslation {
            timestamp: translation.timestamp,
            previous_version: new_prev_version,
            sentences: VecSlice::empty(),
        };

        let new_index = self.paragraph_translations.len();
        self.paragraph_translations.push(new_paragraph);
        self.paragraphs[paragraph_index] = Some(new_index);

        let mut sentences = VecSlice::empty();
        for sentence in translation.sentences() {
            let full_translation = push_string(&mut self.strings, &sentence.full_translation);
            let mut words = VecSlice::empty();
            for word in sentence.words() {
                let original = push_string(&mut self.strings, &word.original);
                let note = push_string(&mut self.strings, &word.note);
                let grammar = Grammar {
                    original_initial_form: push_string(
                        &mut self.strings,
                        &word.grammar.original_initial_form,
                    ),
                    target_initial_form: push_string(
                        &mut self.strings,
                        &word.grammar.target_initial_form,
                    ),
                    part_of_speech: push_string(&mut self.strings, &word.grammar.part_of_speech),
                    plurality: word
                        .grammar
                        .plurality
                        .as_ref()
                        .map(|s| push_string(&mut self.strings, &s)),
                    person: word
                        .grammar
                        .person
                        .as_ref()
                        .map(|s| push_string(&mut self.strings, &s)),
                    tense: word
                        .grammar
                        .tense
                        .as_ref()
                        .map(|s| push_string(&mut self.strings, &s)),
                    case: word
                        .grammar
                        .case
                        .as_ref()
                        .map(|s| push_string(&mut self.strings, &s)),
                    other: word
                        .grammar
                        .other
                        .as_ref()
                        .map(|s| push_string(&mut self.strings, &s)),
                };
                let mut contextual_translations = VecSlice::empty();
                for contextual_translation in word.contextual_translations() {
                    let contextual_translation = WordContextualTranslation {
                        translation: push_string(
                            &mut self.strings,
                            &contextual_translation.translation,
                        ),
                    };
                    contextual_translations = push(
                        &mut self.word_contextual_translations,
                        &contextual_translations,
                        contextual_translation,
                    )
                    .unwrap();
                }
                let new_word = Word {
                    original,
                    contextual_translations,
                    is_punctuation: word.is_punctuation,
                    note,
                    grammar,
                };
                words = push(&mut self.words, &words, new_word).unwrap();
            }
            let new_sentence = Sentence {
                full_translation,
                words,
            };
            sentences = push(&mut self.sentences, &sentences, new_sentence).unwrap();
        }

        self.paragraph_translations[new_index].sentences = sentences;
    }

    pub fn merge(self, other: Self) -> Self {
        let mut merged_translation = Self::create(&self.source_language, &self.target_language);
        for paragraph_idx in 0..self.paragraphs.len().max(other.paragraphs.len()) {
            if let Some(paragarph) = self.paragraph_view(paragraph_idx)
                && let Some(other_paragraph) = other.paragraph_view(paragraph_idx)
            {
                let mut versions = Vec::new();
                let mut curr_paragraph = paragarph;
                loop {
                    let prev_paragraph = curr_paragraph.get_previous_version();
                    versions.push((curr_paragraph.timestamp, curr_paragraph));
                    match prev_paragraph {
                        Some(prev) => curr_paragraph = prev,
                        None => break,
                    }
                }

                let existing_versions = versions
                    .iter()
                    .map(|(timestamp, _)| *timestamp)
                    .collect::<HashSet<_>>();

                curr_paragraph = other_paragraph;

                loop {
                    let prev_paragraph = curr_paragraph.get_previous_version();
                    if !existing_versions.contains(&curr_paragraph.timestamp) {
                        versions.push((curr_paragraph.timestamp, curr_paragraph));
                    }
                    match prev_paragraph {
                        Some(prev) => curr_paragraph = prev,
                        None => break,
                    }
                }

                versions.sort_by_key(|(timestamp, _)| *timestamp);

                for (_, translation) in versions {
                    merged_translation
                        .add_paragraph_translation_from_view(paragraph_idx, &translation);
                }
            } else if let Some(paragarph) = self.paragraph_view(paragraph_idx)
                && other.paragraph_view(paragraph_idx).is_none()
            {
                // Copy entire history from self
                let mut versions = Vec::new();
                let mut curr = Some(paragarph);
                while let Some(p) = curr {
                    let prev = p.get_previous_version();
                    versions.push((p.timestamp, p));
                    curr = prev;
                }
                versions.sort_by_key(|(ts, _)| *ts);
                for (_, v) in versions {
                    merged_translation.add_paragraph_translation_from_view(paragraph_idx, &v);
                }
            } else if self.paragraph_view(paragraph_idx).is_none()
                && let Some(other_paragraph) = other.paragraph_view(paragraph_idx)
            {
                // Copy entire history from other
                let mut versions = Vec::new();
                let mut curr = Some(other_paragraph);
                while let Some(p) = curr {
                    let prev = p.get_previous_version();
                    versions.push((p.timestamp, p));
                    curr = prev;
                }
                versions.sort_by_key(|(ts, _)| *ts);
                for (_, v) in versions {
                    merged_translation.add_paragraph_translation_from_view(paragraph_idx, &v);
                }
            }
        }
        merged_translation
    }
}

impl Serializable for Translation {
    fn serialize<TWriter: io::Write>(&self, output_stream: &mut TWriter) -> std::io::Result<()> {
        // Binary format TR01 v1 (little endian):
        // magic[4] = TR01
        // u8 version = 1
        // Metadata section
        // u8[16] id
        // u64 metadata hash
        // u64 metadata_length
        // u64 source_lang_len, [u8]*
        // u64 target_lang_len, [u8]*
        // u64 translated_paragraphs_count
        // Data section
        // u64 strings_len (compressed), [u8]* (strings blob (zstd compressed))
        // u64 contextual_translations_count, then each: u64 translation.start, u64 translation.len
        // u64 words_count, then each:
        //   u64 original.start,len
        //   u64 note.start,len
        //   u8 is_punctuation
        //   grammar block:
        //     u64 original_initial_form.start,len
        //     u64 target_initial_form.start,len
        //     u64 part_of_speech.start,len
        //     optionals (plurality, person, tense, case, other): for each u8 has + if 1 then u64 start,len
        //   u64 contextual_translations.start,len
        // u64 sentences_count, then each: u64 full_translation.start,len u64 words.start,len
        // u64 paragraph_translations_count, then each:
        //   u64 timestamp
        //   u8 has_previous (if 1 then u64 previous_index)
        //   u64 sentences.start,len
        // u64 paragraphs_count, then each: u8 has_translation (if 1 then u64 paragraph_translation_index)
        // u64 fnv1 hash of the entire file except the hash itself

        let mut hashing_stream = ChecksumedWriter::create(output_stream);

        Magic::Translation.write(&mut hashing_stream)?;
        Version::V1.write_version(&mut hashing_stream)?;

        let mut metadata_buf = Vec::new();
        let mut metadata_buf_hasher = ChecksumedWriter::create(&mut metadata_buf);

        metadata_buf_hasher.write_all(self.id.as_bytes())?;

        write_var_u64(&mut metadata_buf_hasher, self.source_language.len() as u64)?;
        metadata_buf_hasher.write_all(self.source_language.as_bytes())?;
        write_var_u64(&mut metadata_buf_hasher, self.target_language.len() as u64)?;
        metadata_buf_hasher.write_all(self.target_language.as_bytes())?;

        write_var_u64(
            &mut metadata_buf_hasher,
            self.translated_paragraphs_count() as u64,
        )?;

        let metadata_hash = metadata_buf_hasher.current_hash();
        write_u64(&mut hashing_stream, metadata_hash)?;
        write_len_prefixed_bytes(&mut hashing_stream, &metadata_buf)?;

        let encoded = zstd::stream::encode_all(self.strings.as_slice(), 5)?;
        write_var_u64(&mut hashing_stream, encoded.len() as u64)?;
        hashing_stream.write_all(&encoded)?;

        // Contextual translations
        write_var_u64(
            &mut hashing_stream,
            self.word_contextual_translations.len() as u64,
        )?;
        for ct in &self.word_contextual_translations {
            write_vec_slice(&mut hashing_stream, &ct.translation)?;
        }

        // Words
        write_var_u64(&mut hashing_stream, self.words.len() as u64)?;
        for w in &self.words {
            write_vec_slice(&mut hashing_stream, &w.original)?;
            write_vec_slice(&mut hashing_stream, &w.note)?;
            hashing_stream.write_all(&[if w.is_punctuation { 1 } else { 0 }])?;

            // Grammar required fields
            write_vec_slice(&mut hashing_stream, &w.grammar.original_initial_form)?;
            write_vec_slice(&mut hashing_stream, &w.grammar.target_initial_form)?;
            write_vec_slice(&mut hashing_stream, &w.grammar.part_of_speech)?;

            write_opt(&mut hashing_stream, &w.grammar.plurality)?;
            write_opt(&mut hashing_stream, &w.grammar.person)?;
            write_opt(&mut hashing_stream, &w.grammar.tense)?;
            write_opt(&mut hashing_stream, &w.grammar.case)?;
            write_opt(&mut hashing_stream, &w.grammar.other)?;

            write_vec_slice(&mut hashing_stream, &w.contextual_translations)?;
        }

        // Sentences
        write_var_u64(&mut hashing_stream, self.sentences.len() as u64)?;
        for s in &self.sentences {
            write_vec_slice(&mut hashing_stream, &s.full_translation)?;
            write_vec_slice(&mut hashing_stream, &s.words)?;
        }

        // Paragraph translations
        write_var_u64(
            &mut hashing_stream,
            self.paragraph_translations.len() as u64,
        )?;
        for pt in &self.paragraph_translations {
            write_var_u64(&mut hashing_stream, pt.timestamp as u64)?;
            match pt.previous_version {
                Some(idx) => {
                    hashing_stream.write_all(&[1])?;
                    write_var_u64(&mut hashing_stream, idx as u64)?;
                }
                None => hashing_stream.write_all(&[0])?,
            };
            write_vec_slice(&mut hashing_stream, &pt.sentences)?;
        }

        // Paragraphs (Option indices)
        write_var_u64(&mut hashing_stream, self.paragraphs.len() as u64)?;
        for p in &self.paragraphs {
            match p {
                Some(idx) => {
                    hashing_stream.write_all(&[1])?;
                    write_var_u64(&mut hashing_stream, *idx as u64)?;
                }
                None => hashing_stream.write_all(&[0])?,
            }
        }

        // Hash
        let hash = hashing_stream.current_hash();
        write_u64(output_stream, hash)?;

        output_stream.flush()?;

        Ok(())
    }

    fn deserialize<TReader: io::Seek + io::Read>(
        input_stream: &mut TReader,
    ) -> std::io::Result<Self>
    where
        Self: Sized,
    {
        let hash_valid = validate_hash(input_stream)?;
        if !hash_valid {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid hash"));
        }

        let mut magic = [0u8; 4];
        input_stream.read_exact(&mut magic)?;
        if &magic != Magic::Translation.as_bytes() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid magic"));
        }
        Version::read_version(input_stream)?;

        // Skip metadata hash
        _ = read_u64(input_stream)?;

        // Skip metadata length
        _ = read_var_u64(input_stream)?;

        let id = Uuid::from_bytes(read_exact_array::<16>(input_stream)?);

        let source_language = read_len_prefixed_string(input_stream)?;
        let target_language = read_len_prefixed_string(input_stream)?;

        // Skip translated_paragraphs_count
        _ = read_var_u64(input_stream)?;

        let encoded_data = read_len_prefixed_vec(input_stream)?;
        let strings = zstd::stream::decode_all(encoded_data.as_slice())?;

        // Contextual translations
        let ct_len = read_var_u64(input_stream)? as usize;
        let mut word_contextual_translations = Vec::with_capacity(ct_len);
        for _ in 0..ct_len {
            let slice = read_vec_slice::<u8>(input_stream)?;
            word_contextual_translations.push(WordContextualTranslation { translation: slice });
        }

        // Words
        let words_len = read_var_u64(input_stream)? as usize;
        let mut words = Vec::with_capacity(words_len);
        for _ in 0..words_len {
            let original = read_vec_slice::<u8>(input_stream)?;
            let note = read_vec_slice::<u8>(input_stream)?;
            let is_punctuation = read_u8(input_stream)? == 1;
            let original_initial_form = read_vec_slice::<u8>(input_stream)?;
            let target_initial_form = read_vec_slice::<u8>(input_stream)?;
            let part_of_speech = read_vec_slice::<u8>(input_stream)?;
            let plurality = read_opt(input_stream)?;
            let person = read_opt(input_stream)?;
            let tense = read_opt(input_stream)?;
            let case = read_opt(input_stream)?;
            let other = read_opt(input_stream)?;
            let contextual_translations =
                read_vec_slice::<WordContextualTranslation>(input_stream)?;
            let grammar = Grammar {
                original_initial_form,
                target_initial_form,
                part_of_speech,
                plurality,
                person,
                tense,
                case,
                other,
            };
            words.push(Word {
                original,
                contextual_translations,
                is_punctuation,
                note,
                grammar,
            });
        }

        // Sentences
        let sentences_len = read_var_u64(input_stream)? as usize;
        let mut sentences = Vec::with_capacity(sentences_len);
        for _ in 0..sentences_len {
            let full_translation = read_vec_slice::<u8>(input_stream)?;
            let words_slice = read_vec_slice::<Word>(input_stream)?;
            sentences.push(Sentence {
                full_translation,
                words: words_slice,
            });
        }

        // Paragraph translations
        let pt_len = read_var_u64(input_stream)? as usize;
        let mut paragraph_translations = Vec::with_capacity(pt_len);
        for _ in 0..pt_len {
            let timestamp = read_var_u64(input_stream)? as usize;
            let has_prev = read_u8(input_stream)?;
            let previous_version = if has_prev == 1 {
                Some(read_var_u64(input_stream)? as usize)
            } else {
                None
            };
            let sentences_slice = read_vec_slice::<Sentence>(input_stream)?;
            paragraph_translations.push(ParagraphTranslation {
                timestamp,
                previous_version,
                sentences: sentences_slice,
            });
        }

        // Paragraphs (Option indices)
        let paragraphs_len = read_var_u64(input_stream)? as usize;
        let mut paragraphs = Vec::with_capacity(paragraphs_len);
        for _ in 0..paragraphs_len {
            let has = read_u8(input_stream)?;
            let val = if has == 1 {
                Some(read_var_u64(input_stream)? as usize)
            } else {
                None
            };
            paragraphs.push(val);
        }

        Ok(Translation {
            id,
            source_language,
            target_language,
            strings,
            paragraphs,
            paragraph_translations,
            sentences,
            words,
            word_contextual_translations,
        })
    }
}

impl<'a> ParagraphTranslationView<'a> {
    pub fn get_previous_version(&self) -> Option<ParagraphTranslationView<'a>> {
        let paragraph = self
            .previous_version
            .map(|p| (p, &self.translation.paragraph_translations[p]));
        paragraph.map(|(idx, p)| ParagraphTranslationView {
            idx,
            translation: self.translation,
            timestamp: p.timestamp,
            previous_version: p.previous_version,
            sentences: p.sentences.slice(&self.translation.sentences),
        })
    }

    pub fn sentence_count(&self) -> usize {
        self.sentences.len()
    }

    pub fn sentence_view(&self, sentence: usize) -> SentenceView<'a> {
        let sentence = &self.sentences[sentence];
        SentenceView {
            translation: self.translation,
            full_translation: String::from_utf8_lossy(
                sentence.full_translation.slice(&self.translation.strings),
            ),
            words: sentence.words.slice(&self.translation.words),
        }
    }

    pub fn sentences(&'_ self) -> impl Iterator<Item = SentenceView<'_>> {
        (0..self.sentence_count()).map(|s| self.sentence_view(s))
    }
}

impl<'a> SentenceView<'a> {
    pub fn word_count(&self) -> usize {
        self.words.len()
    }

    pub fn word_view(&self, word: usize) -> WordView<'a> {
        let word = &self.words[word];
        WordView {
            translation: self.translation,
            original: String::from_utf8_lossy(word.original.slice(&self.translation.strings)),
            note: String::from_utf8_lossy(word.note.slice(&self.translation.strings)),
            grammar: GrammarView {
                original_initial_form: String::from_utf8_lossy(
                    word.grammar
                        .original_initial_form
                        .slice(&self.translation.strings),
                ),
                target_initial_form: String::from_utf8_lossy(
                    word.grammar
                        .target_initial_form
                        .slice(&self.translation.strings),
                ),
                part_of_speech: String::from_utf8_lossy(
                    word.grammar.part_of_speech.slice(&self.translation.strings),
                ),
                plurality: word
                    .grammar
                    .plurality
                    .map(|s| String::from_utf8_lossy(s.slice(&self.translation.strings))),
                person: word
                    .grammar
                    .person
                    .map(|s| String::from_utf8_lossy(s.slice(&self.translation.strings))),
                tense: word
                    .grammar
                    .tense
                    .map(|s| String::from_utf8_lossy(s.slice(&self.translation.strings))),
                case: word
                    .grammar
                    .case
                    .map(|s| String::from_utf8_lossy(s.slice(&self.translation.strings))),
                other: word
                    .grammar
                    .other
                    .map(|s| String::from_utf8_lossy(s.slice(&self.translation.strings))),
            },
            is_punctuation: word.is_punctuation,
            contextual_translations: word
                .contextual_translations
                .slice(&self.translation.word_contextual_translations),
        }
    }

    pub fn words(&'_ self) -> impl Iterator<Item = WordView<'_>> {
        (0..self.word_count()).map(|w| self.word_view(w))
    }
}

impl<'a> WordView<'a> {
    pub fn contextual_translations_count(&self) -> usize {
        self.contextual_translations.len()
    }

    pub fn contextual_translations_view(&self, index: usize) -> WordContextualTranslationView<'a> {
        let contextual_translation = &self.contextual_translations[index];
        WordContextualTranslationView {
            translation: String::from_utf8_lossy(
                contextual_translation
                    .translation
                    .slice(&self.translation.strings),
            ),
        }
    }

    pub fn contextual_translations(
        &self,
    ) -> impl Iterator<Item = WordContextualTranslationView<'_>> {
        (0..self.contextual_translations_count()).map(|t| self.contextual_translations_view(t))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn make_word(original: &str) -> translation_import::Word {
        translation_import::Word {
            original: original.to_string(),
            contextual_translations: vec![format!("{}-ct", original)],
            note: String::new(),
            is_punctuation: false,
            grammar: translation_import::Grammar {
                original_initial_form: original.to_string(),
                target_initial_form: original.to_string(),
                part_of_speech: "n".into(),
                plurality: None,
                person: None,
                tense: None,
                case: None,
                other: None,
            },
        }
    }

    fn make_paragraph(ts: usize, text: &str) -> translation_import::ParagraphTranslation {
        translation_import::ParagraphTranslation {
            timestamp: ts,
            sentences: vec![translation_import::Sentence {
                full_translation: text.to_string(),
                words: vec![make_word(text)],
            }],
        }
    }

    #[test]
    fn test_translation_add_paragraph_translation() {
        let mut translation = Translation::create("en", "ru");
        let paragraph_translation = translation_import::ParagraphTranslation {
            timestamp: 1234567890,
            sentences: vec![translation_import::Sentence {
                full_translation: "Hello, world!".to_string(),
                words: vec![
                    translation_import::Word {
                        original: "Hello".to_string(),
                        contextual_translations: vec!["Привет".to_string()],
                        note: "A common greeting".to_string(),
                        is_punctuation: false,
                        grammar: translation_import::Grammar {
                            original_initial_form: "hello".to_string(),
                            target_initial_form: "привет".to_string(),
                            part_of_speech: "interjection".to_string(),
                            plurality: None,
                            person: None,
                            tense: None,
                            case: None,
                            other: None,
                        },
                    },
                    translation_import::Word {
                        original: ",".to_string(),
                        contextual_translations: vec![",".to_string()],
                        note: "".to_string(),
                        is_punctuation: true,
                        grammar: translation_import::Grammar {
                            original_initial_form: ",".to_string(),
                            target_initial_form: ",".to_string(),
                            part_of_speech: "punctuation".to_string(),
                            plurality: None,
                            person: None,
                            tense: None,
                            case: None,
                            other: None,
                        },
                    },
                    translation_import::Word {
                        original: "world".to_string(),
                        contextual_translations: vec!["мир".to_string()],
                        note: "".to_string(),
                        is_punctuation: false,
                        grammar: translation_import::Grammar {
                            original_initial_form: "world".to_string(),
                            target_initial_form: "мир".to_string(),
                            part_of_speech: "noun".to_string(),
                            plurality: Some("singular".to_string()),
                            person: None,
                            tense: None,
                            case: Some("nominative".to_string()),
                            other: None,
                        },
                    },
                    translation_import::Word {
                        original: "!".to_string(),
                        contextual_translations: vec!["!".to_string()],
                        note: "".to_string(),
                        is_punctuation: true,
                        grammar: translation_import::Grammar {
                            original_initial_form: "!".to_string(),
                            target_initial_form: "!".to_string(),
                            part_of_speech: "punctuation".to_string(),
                            plurality: None,
                            person: None,
                            tense: None,
                            case: None,
                            other: None,
                        },
                    },
                ],
            }],
        };
        translation.add_paragraph_translation(0, &paragraph_translation);
        let paragraph_view = translation.paragraph_view(0).unwrap();
        assert_eq!(paragraph_view.timestamp, 1234567890);
        assert_eq!(paragraph_view.previous_version, None);
        assert_eq!(paragraph_view.sentence_count(), 1);
        let sentence_view = paragraph_view.sentence_view(0);
        assert_eq!(sentence_view.full_translation, "Hello, world!");
        assert_eq!(sentence_view.word_count(), 4);
        let word_view_0 = sentence_view.word_view(0);
        assert_eq!(word_view_0.original, "Hello");
        assert_eq!(word_view_0.note, "A common greeting");
        assert_eq!(word_view_0.is_punctuation, false);
        assert_eq!(word_view_0.grammar.original_initial_form, "hello");
        assert_eq!(word_view_0.grammar.target_initial_form, "привет");
        assert_eq!(word_view_0.grammar.part_of_speech, "interjection");
        assert_eq!(word_view_0.grammar.plurality, None);
        assert_eq!(word_view_0.grammar.person, None);
        assert_eq!(word_view_0.grammar.tense, None);
        assert_eq!(word_view_0.grammar.case, None);
        assert_eq!(word_view_0.grammar.other, None);
        assert_eq!(word_view_0.contextual_translations_count(), 1);
        let contextual_translation_view_0 = word_view_0.contextual_translations_view(0);
        assert_eq!(contextual_translation_view_0.translation, "Привет");
    }

    #[test]
    fn translation_serialize_deserialize_round_trip() {
        let mut translation = Translation::create("en", "ru");
        let paragraph_translation = translation_import::ParagraphTranslation {
            timestamp: 1,
            sentences: vec![translation_import::Sentence {
                full_translation: "Hi".into(),
                words: vec![translation_import::Word {
                    original: "Hi".into(),
                    contextual_translations: vec!["Привет".into()],
                    note: "greet".into(),
                    is_punctuation: false,
                    grammar: translation_import::Grammar {
                        original_initial_form: "hi".into(),
                        target_initial_form: "привет".into(),
                        part_of_speech: "interj".into(),
                        plurality: None,
                        person: None,
                        tense: None,
                        case: None,
                        other: None,
                    },
                }],
            }],
        };
        translation.add_paragraph_translation(0, &paragraph_translation);

        // second version
        let paragraph_translation2 = translation_import::ParagraphTranslation {
            timestamp: 2,
            sentences: vec![translation_import::Sentence {
                full_translation: "Hi there".into(),
                words: vec![
                    translation_import::Word {
                        original: "Hi".into(),
                        contextual_translations: vec!["Привет".into()],
                        note: "greet".into(),
                        is_punctuation: false,
                        grammar: translation_import::Grammar {
                            original_initial_form: "hi".into(),
                            target_initial_form: "привет".into(),
                            part_of_speech: "interj".into(),
                            plurality: None,
                            person: None,
                            tense: None,
                            case: None,
                            other: None,
                        },
                    },
                    translation_import::Word {
                        original: "there".into(),
                        contextual_translations: vec!["там".into()],
                        note: "".into(),
                        is_punctuation: false,
                        grammar: translation_import::Grammar {
                            original_initial_form: "there".into(),
                            target_initial_form: "там".into(),
                            part_of_speech: "adv".into(),
                            plurality: None,
                            person: None,
                            tense: None,
                            case: None,
                            other: None,
                        },
                    },
                ],
            }],
        };
        translation.add_paragraph_translation(0, &paragraph_translation2);

        let mut buf: Vec<u8> = vec![];
        translation.serialize(&mut buf).unwrap();
        let mut cursor = Cursor::new(buf);
        let translation2 = Translation::deserialize(&mut cursor).unwrap();

        assert_eq!(translation2.source_language, "en");
        assert_eq!(translation2.target_language, "ru");
        // Latest paragraph view
        let latest = translation2.paragraph_view(0).unwrap();
        assert_eq!(latest.sentence_count(), 1);
        let sentence = latest.sentence_view(0);
        assert_eq!(sentence.full_translation, "Hi there");
        assert_eq!(sentence.word_count(), 2);
        let word0 = sentence.word_view(0);
        assert_eq!(word0.original, "Hi");
        assert_eq!(word0.contextual_translations_count(), 1);
        let word1 = sentence.word_view(1);
        assert_eq!(word1.original, "there");
        // Previous version chain
        let prev = latest.get_previous_version().unwrap();
        let prev_sentence = prev.sentence_view(0);
        assert_eq!(prev_sentence.full_translation, "Hi");
    }

    #[test]
    fn translation_serialize_deserialize_corruption() {
        let mut translation = Translation::create("en", "ru");
        let paragraph_translation = translation_import::ParagraphTranslation {
            timestamp: 1,
            sentences: vec![translation_import::Sentence {
                full_translation: "Hi".into(),
                words: vec![translation_import::Word {
                    original: "Hi".into(),
                    contextual_translations: vec!["Привет".into()],
                    note: "greet".into(),
                    is_punctuation: false,
                    grammar: translation_import::Grammar {
                        original_initial_form: "hi".into(),
                        target_initial_form: "привет".into(),
                        part_of_speech: "interj".into(),
                        plurality: None,
                        person: None,
                        tense: None,
                        case: None,
                        other: None,
                    },
                }],
            }],
        };
        translation.add_paragraph_translation(0, &paragraph_translation);

        // second version
        let paragraph_translation2 = translation_import::ParagraphTranslation {
            timestamp: 2,
            sentences: vec![translation_import::Sentence {
                full_translation: "Hi there".into(),
                words: vec![
                    translation_import::Word {
                        original: "Hi".into(),
                        contextual_translations: vec!["Привет".into()],
                        note: "greet".into(),
                        is_punctuation: false,
                        grammar: translation_import::Grammar {
                            original_initial_form: "hi".into(),
                            target_initial_form: "привет".into(),
                            part_of_speech: "interj".into(),
                            plurality: None,
                            person: None,
                            tense: None,
                            case: None,
                            other: None,
                        },
                    },
                    translation_import::Word {
                        original: "there".into(),
                        contextual_translations: vec!["там".into()],
                        note: "".into(),
                        is_punctuation: false,
                        grammar: translation_import::Grammar {
                            original_initial_form: "there".into(),
                            target_initial_form: "там".into(),
                            part_of_speech: "adv".into(),
                            plurality: None,
                            person: None,
                            tense: None,
                            case: None,
                            other: None,
                        },
                    },
                ],
            }],
        };
        translation.add_paragraph_translation(0, &paragraph_translation2);

        let mut buf: Vec<u8> = vec![];
        translation.serialize(&mut buf).unwrap();

        // Corrupt
        buf[12] = 0xae;

        let mut cursor = Cursor::new(buf);
        let translation2 = Translation::deserialize(&mut cursor);
        assert!(translation2.is_err());
    }

    #[test]
    fn merge_same_history() {
        let mut a = Translation::create("en", "ru");
        a.add_paragraph_translation(0, &make_paragraph(1, "v1"));
        a.add_paragraph_translation(0, &make_paragraph(2, "v2"));

        let mut b = Translation::create("en", "ru");
        b.add_paragraph_translation(0, &make_paragraph(1, "v1"));
        b.add_paragraph_translation(0, &make_paragraph(2, "v2"));

        let merged = a.merge(b);

        let latest = merged.paragraph_view(0).expect("merged paragraph");
        assert_eq!(latest.timestamp, 2);
        assert_eq!(latest.sentence_view(0).full_translation, "v2");
        let prev = latest.get_previous_version().expect("prev exists");
        assert_eq!(prev.timestamp, 1);
        assert_eq!(prev.sentence_view(0).full_translation, "v1");
        assert!(prev.get_previous_version().is_none());
    }

    #[test]
    fn merge_diverged_common_root() {
        // a: 1 -> 2 -> 4
        let mut a = Translation::create("en", "ru");
        a.add_paragraph_translation(0, &make_paragraph(1, "a1"));
        a.add_paragraph_translation(0, &make_paragraph(2, "a2"));
        a.add_paragraph_translation(0, &make_paragraph(4, "a4"));

        // b: 1 -> 3 -> 5
        let mut b = Translation::create("en", "ru");
        b.add_paragraph_translation(0, &make_paragraph(1, "a1")); // same ts as a1 (dedup)
        b.add_paragraph_translation(0, &make_paragraph(3, "a3"));
        b.add_paragraph_translation(0, &make_paragraph(5, "a5"));

        let merged = a.merge(b);

        // Expect order by ts: 1,2,3,4,5 (latest=5)
        let mut ts = Vec::new();
        let mut v = merged.paragraph_view(0).unwrap();
        ts.push(v.timestamp);
        while let Some(prev) = v.get_previous_version() {
            ts.push(prev.timestamp);
            v = prev;
        }
        assert_eq!(ts, vec![5, 4, 3, 2, 1]);
        // Verify content for unique timestamps
        assert_eq!(merged.paragraph_view(0).unwrap().sentence_view(0).full_translation, "a5");
        let v4 = merged.paragraph_view(0).unwrap().get_previous_version().unwrap();
        assert_eq!(v4.sentence_view(0).full_translation, "a4");
    }

    #[test]
    fn merge_no_common_root() {
        // a: 10 -> 20
        let mut a = Translation::create("en", "ru");
        a.add_paragraph_translation(0, &make_paragraph(10, "a10"));
        a.add_paragraph_translation(0, &make_paragraph(20, "a20"));

        // b: 5 -> 15 -> 25
        let mut b = Translation::create("en", "ru");
        b.add_paragraph_translation(0, &make_paragraph(5, "b5"));
        b.add_paragraph_translation(0, &make_paragraph(15, "b15"));
        b.add_paragraph_translation(0, &make_paragraph(25, "b25"));

        let merged = a.merge(b);
        let mut ts = Vec::new();
        let mut v = merged.paragraph_view(0).unwrap();
        ts.push(v.timestamp);
        while let Some(prev) = v.get_previous_version() {
            ts.push(prev.timestamp);
            v = prev;
        }
        assert_eq!(ts, vec![25, 20, 15, 10, 5]);
    }

    #[test]
    fn merge_present_only_in_one_side() {
        // Paragraph 0 only in left, with history 1 -> 2
        let mut a = Translation::create("en", "ru");
        a.add_paragraph_translation(0, &make_paragraph(1, "a1"));
        a.add_paragraph_translation(0, &make_paragraph(2, "a2"));
        // Paragraph 1 only in right, with single version 3
        let b = {
            let mut t = Translation::create("en", "ru");
            t.add_paragraph_translation(1, &make_paragraph(3, "b3"));
            t
        };

        let merged = a.merge(b);

        // Paragraph 0 preserved history
        let mut ts0 = Vec::new();
        let mut v0 = merged.paragraph_view(0).unwrap();
        ts0.push(v0.timestamp);
        while let Some(prev) = v0.get_previous_version() {
            ts0.push(prev.timestamp);
            v0 = prev;
        }
        assert_eq!(ts0, vec![2, 1]);

        // Paragraph 1 from right present
        let v1 = merged.paragraph_view(1).unwrap();
        assert_eq!(v1.timestamp, 3);
        assert!(v1.get_previous_version().is_none());
    }
}
