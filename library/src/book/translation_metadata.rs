use std::{
    hash::Hasher,
    io::{self, Cursor},
};

use uuid::Uuid;

use crate::book::serialization::{
    Magic, Version, read_exact_array, read_len_prefixed_string, read_len_prefixed_vec, read_u64,
    read_var_u64,
};

#[derive(Debug)]
pub struct TranslationMetadata {
    pub id: Uuid,
    pub source_language: String,
    pub target_language: String,
    pub translated_paragraphs_count: usize,
}

impl TranslationMetadata {
    pub fn read_metadata<TReader: io::Read>(input_stream: &mut TReader) -> io::Result<Self>
    where
        Self: Sized,
    {
        // Magic
        let magic = read_exact_array::<4>(input_stream)?;
        if &magic != Magic::Translation.as_bytes() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid magic"));
        }
        Version::read_version(input_stream)?; // ensure supported

        // hash
        let metadata_hash = read_u64(input_stream)?;

        // Read metadata
        let metadata_buf = read_len_prefixed_vec(input_stream)?;

        let mut hasher = fnv::FnvHasher::default();
        hasher.write(&metadata_buf);
        if hasher.finish() != metadata_hash {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid metadata hash",
            ));
        }

        let mut cursor = Cursor::new(metadata_buf);

        let id = Uuid::from_bytes(read_exact_array(&mut cursor)?);

        let source_language = read_len_prefixed_string(&mut cursor)?;
        let target_language = read_len_prefixed_string(&mut cursor)?;

        let translated_paragraphs_count = read_var_u64(&mut cursor)? as usize;

        Ok(TranslationMetadata {
            id,
            source_language,
            target_language,
            translated_paragraphs_count,
        })
    }
}

#[cfg(test)]
mod translation_metadata_test {
    use std::io::Cursor;

    use crate::{
        book::{
            serialization::Serializable, translation::Translation, translation_import,
            translation_metadata::TranslationMetadata,
        },
        dictionary::Dictionary,
        translator::TranslationModel,
    };

    #[test]
    fn test_metadata_roundtrip() {
        let mut translation = Translation::create("en", "ru");
        let paragraph_translation = translation_import::ParagraphTranslation {
            total_tokens: None,
            timestamp: 1,
            source_language: "en".to_owned(),
            target_language: "ru".to_owned(),
            sentences: vec![translation_import::Sentence {
                full_translation: "Hi".into(),
                words: vec![translation_import::Word {
                    original: "Hi".into(),
                    contextual_translations: vec!["Привет".into()],
                    note: Some("greet".into()),
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

        let mut dict = Dictionary::create("en".to_owned(), "ru".to_owned());

        translation.add_paragraph_translation(
            0,
            &paragraph_translation,
            TranslationModel::Gemini25Flash,
            &mut dict,
        );

        // another paragraph
        let paragraph_translation2 = translation_import::ParagraphTranslation {
            total_tokens: None,
            timestamp: 2,
            source_language: "en".to_owned(),
            target_language: "ru".to_owned(),
            sentences: vec![translation_import::Sentence {
                full_translation: "Hi there".into(),
                words: vec![
                    translation_import::Word {
                        original: "Hi".into(),
                        contextual_translations: vec!["Привет".into()],
                        note: Some("greet".into()),
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
                        note: Some("".into()),
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
        translation.add_paragraph_translation(
            3,
            &paragraph_translation2,
            TranslationModel::Gemini25Flash,
            &mut dict,
        );

        let mut buf: Vec<u8> = vec![];
        translation.serialize(&mut buf).unwrap();

        let mut cursor = Cursor::new(buf);
        let metadata = TranslationMetadata::read_metadata(&mut cursor).unwrap();

        assert_eq!(metadata.source_language, "en");
        assert_eq!(metadata.target_language, "ru");
        assert_eq!(metadata.translated_paragraphs_count, 2);
    }

    #[test]
    fn test_metadata_corruption() {
        let mut translation = Translation::create("en", "ru");
        let paragraph_translation = translation_import::ParagraphTranslation {
            total_tokens: None,
            timestamp: 1,
            source_language: "en".to_owned(),
            target_language: "ru".to_owned(),
            sentences: vec![translation_import::Sentence {
                full_translation: "Hi".into(),
                words: vec![translation_import::Word {
                    original: "Hi".into(),
                    contextual_translations: vec!["Привет".into()],
                    note: Some("greet".into()),
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
        let mut dict = Dictionary::create("en".to_owned(), "ru".to_owned());
        translation.add_paragraph_translation(
            0,
            &paragraph_translation,
            TranslationModel::Gemini25Flash,
            &mut dict,
        );

        // another paragraph
        let paragraph_translation2 = translation_import::ParagraphTranslation {
            total_tokens: None,
            timestamp: 2,
            source_language: "en".to_owned(),
            target_language: "ru".to_owned(),
            sentences: vec![translation_import::Sentence {
                full_translation: "Hi there".into(),
                words: vec![
                    translation_import::Word {
                        original: "Hi".into(),
                        contextual_translations: vec!["Привет".into()],
                        note: Some("greet".into()),
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
                        note: Some("".into()),
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
        translation.add_paragraph_translation(
            3,
            &paragraph_translation2,
            TranslationModel::Gemini25Flash,
            &mut dict,
        );

        let mut buf: Vec<u8> = vec![];
        translation.serialize(&mut buf).unwrap();

        buf[15] = 0xae;

        let mut cursor = Cursor::new(buf);
        let metadata = TranslationMetadata::read_metadata(&mut cursor);

        assert!(metadata.is_err());
    }
}
