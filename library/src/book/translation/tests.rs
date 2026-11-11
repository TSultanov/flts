use std::io::Cursor;

use super::*;

fn make_word(original: &str) -> translation_import::Word {
    translation_import::Word {
        original: original.to_string(),
        contextual_translations: vec![format!("{}-ct", original)],
        note: Some(String::new()),
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

fn make_paragraph(ts: u64, text: &str) -> translation_import::ParagraphTranslation {
    translation_import::ParagraphTranslation {
        timestamp: ts,
        sentences: vec![translation_import::Sentence {
            full_translation: text.to_string(),
            words: vec![make_word(text)],
        }],
        total_tokens: None,
    }
}

#[test]
fn test_translation_add_paragraph_translation() {
    let mut translation = Translation::create("en", "ru");
    let paragraph_translation = translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: 1234567890,
        sentences: vec![translation_import::Sentence {
            full_translation: "Hello, world!".to_string(),
            words: vec![
                translation_import::Word {
                    original: "Hello".to_string(),
                    contextual_translations: vec!["Привет".to_string()],
                    note: Some("A common greeting".to_string()),
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
                    note: Some("".to_string()),
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
                    note: Some("".to_string()),
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
                    note: Some("".to_string()),
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
    translation.add_paragraph_translation(0, &paragraph_translation, "models/gemini-2.5-pro");
    let paragraph_view = translation.paragraph_view(0).unwrap();
    assert_eq!(paragraph_view.timestamp, 1234567890);
    assert_eq!(paragraph_view.previous_version, None);
    assert_eq!(paragraph_view.sentence_count(), 1);
    assert_eq!(paragraph_view.model, "models/gemini-2.5-pro");
    let sentence_view = paragraph_view.sentence_view(0);
    assert_eq!(sentence_view.full_translation, "Hello, world!");
    assert_eq!(sentence_view.word_count(), 4);
    let word_view_0 = sentence_view.word_view(0);
    assert_eq!(word_view_0.original, "Hello");
    assert_eq!(word_view_0.note, "A common greeting");
    assert!(!word_view_0.is_punctuation);
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
        total_tokens: Some(1234),
        timestamp: 1,
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
    translation.add_paragraph_translation(0, &paragraph_translation, "models/gemini-2.5-flash");

    let paragraph_translation2 = translation_import::ParagraphTranslation {
        total_tokens: Some(4321),
        timestamp: 2,
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
        0,
        &paragraph_translation2,
        "models/gemini-2.5-flash-lite",
    );

    let mut buf: Vec<u8> = vec![];
    translation.serialize(&mut buf).unwrap();
    let mut cursor = Cursor::new(buf);
    let translation2 = Translation::deserialize(&mut cursor).unwrap();

    assert_eq!(translation2.source_language, "en");
    assert_eq!(translation2.target_language, "ru");
    let latest = translation2.paragraph_view(0).unwrap();
    assert_eq!(latest.sentence_count(), 1);
    assert_eq!(latest.model, "models/gemini-2.5-flash-lite");
    assert_eq!(latest.total_tokens, Some(4321));
    let sentence = latest.sentence_view(0);
    assert_eq!(sentence.full_translation, "Hi there");
    assert_eq!(sentence.word_count(), 2);
    let word0 = sentence.word_view(0);
    assert_eq!(word0.original, "Hi");
    assert_eq!(word0.contextual_translations_count(), 1);
    let word1 = sentence.word_view(1);
    assert_eq!(word1.original, "there");
    let prev = latest.get_previous_version().unwrap();
    let prev_sentence = prev.sentence_view(0);
    assert_eq!(prev_sentence.full_translation, "Hi");
}

#[test]
fn translation_serialize_v1_deserialize_round_trip() {
    let mut translation = Translation::create("en", "ru");
    let paragraph_translation = translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: 1,
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
    translation.add_paragraph_translation(0, &paragraph_translation, "models/gemini-2.5-flash");

    let paragraph_translation2 = translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: 2,
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
        0,
        &paragraph_translation2,
        "models/gemini-2.5-flash-lite",
    );

    let mut buf: Vec<u8> = vec![];
    translation.serialize_v1(&mut buf).unwrap();
    let mut cursor = Cursor::new(buf);
    let translation2 = Translation::deserialize(&mut cursor).unwrap();

    assert_eq!(translation2.source_language, "en");
    assert_eq!(translation2.target_language, "ru");
    let latest = translation2.paragraph_view(0).unwrap();
    assert_eq!(latest.sentence_count(), 1);
    assert_eq!(latest.model, "");
    let sentence = latest.sentence_view(0);
    assert_eq!(sentence.full_translation, "Hi there");
    assert_eq!(sentence.word_count(), 2);
    let word0 = sentence.word_view(0);
    assert_eq!(word0.original, "Hi");
    assert_eq!(word0.contextual_translations_count(), 1);
    let word1 = sentence.word_view(1);
    assert_eq!(word1.original, "there");
    let prev = latest.get_previous_version().unwrap();
    let prev_sentence = prev.sentence_view(0);
    assert_eq!(prev_sentence.full_translation, "Hi");
}

#[test]
fn translation_serialize_deserialize_corruption() {
    let mut translation = Translation::create("en", "ru");
    let paragraph_translation = translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: 1,
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
    translation.add_paragraph_translation(0, &paragraph_translation, "models/gemini-2.5-flash");

    let paragraph_translation2 = translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: 2,
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
    translation.add_paragraph_translation(0, &paragraph_translation2, "models/gemini-2.5-flash");

    let mut buf: Vec<u8> = vec![];
    translation.serialize(&mut buf).unwrap();

    // Byte 12 is the last byte of the metadata hash, which depends on the
    // random UUID. XOR so the mutation is never a no-op.
    buf[12] ^= 0xff;

    let mut cursor = Cursor::new(buf);
    let translation2 = Translation::deserialize(&mut cursor);
    assert!(translation2.is_err());
}

#[test]
fn to_import_empty_paragraph() {
    let mut translation = Translation::create("eng", "rus");
    let input = translation_import::ParagraphTranslation {
        timestamp: 42,
        total_tokens: Some(17),
        sentences: vec![],
    };
    translation.add_paragraph_translation(0, &input, "models/gemini-2.5-flash");

    let view = translation.latest_version_view(0).unwrap();
    let out = view.to_import();

    assert_eq!(out.timestamp, 42);
    assert_eq!(out.total_tokens, Some(17));
    assert!(out.sentences.is_empty());
}

fn make_empty_paragraph(ts: u64) -> translation_import::ParagraphTranslation {
    translation_import::ParagraphTranslation {
        timestamp: ts,
        sentences: vec![],
        total_tokens: None,
    }
}

#[test]
fn empty_latest_version_counts_as_untranslated() {
    let mut t = Translation::create("en", "ru");
    t.add_paragraph_translation(0, &make_empty_paragraph(1), "models/gemini-2.5-flash");
    assert!(t.paragraph_view(0).is_none());
    assert_eq!(t.translated_paragraphs_count(), 0);

    t.add_paragraph_translation(1, &make_paragraph(2, "p1"), "models/gemini-2.5-flash");
    assert_eq!(t.translated_paragraphs_count(), 1);
}

#[test]
fn empty_version_over_real_one_hides_it_until_retranslated() {
    let mut t = Translation::create("en", "ru");
    t.add_paragraph_translation(0, &make_paragraph(1, "v1"), "models/gemini-2.5-flash");
    t.add_paragraph_translation(0, &make_empty_paragraph(2), "models/gemini-2.5-flash");
    assert!(t.paragraph_view(0).is_none());

    t.add_paragraph_translation(0, &make_paragraph(3, "v3"), "models/gemini-2.5-flash");
    let latest = t.paragraph_view(0).expect("retranslated paragraph");
    assert_eq!(latest.timestamp, 3);
    let empty = latest.get_previous_version().expect("empty version kept");
    assert_eq!((empty.timestamp, empty.sentence_count()), (2, 0));
    let first = empty.get_previous_version().expect("original version kept");
    assert_eq!(first.sentence_view(0).full_translation, "v1");
    assert_eq!(t.translated_paragraphs_count(), 1);
}

#[test]
fn to_import_word_grammar_and_punctuation() {
    let mut translation = Translation::create("spa", "rus");
    let input = translation_import::ParagraphTranslation {
        timestamp: 100,
        total_tokens: None,
        sentences: vec![translation_import::Sentence {
            full_translation: "Я могу.".into(),
            words: vec![
                translation_import::Word {
                    original: "puedo".into(),
                    contextual_translations: vec!["могу".into(), "умею".into()],
                    note: Some("first-person singular".into()),
                    is_punctuation: false,
                    grammar: translation_import::Grammar {
                        original_initial_form: "poder".into(),
                        target_initial_form: "мочь".into(),
                        part_of_speech: "verb".into(),
                        plurality: Some("singular".into()),
                        person: Some("1".into()),
                        tense: Some("present".into()),
                        case: None,
                        other: None,
                    },
                },
                translation_import::Word {
                    original: ".".into(),
                    contextual_translations: vec![".".into()],
                    note: None,
                    is_punctuation: true,
                    grammar: translation_import::Grammar {
                        original_initial_form: ".".into(),
                        target_initial_form: ".".into(),
                        part_of_speech: "punctuation".into(),
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
    translation.add_paragraph_translation(0, &input, "models/gemini-2.5-flash");

    let view = translation.paragraph_view(0).unwrap();
    let out = view.to_import();

    assert_eq!(out.sentences.len(), 1);
    let sentence = &out.sentences[0];
    assert_eq!(sentence.full_translation, "Я могу.");
    assert_eq!(sentence.words.len(), 2);

    let verb = &sentence.words[0];
    assert_eq!(verb.original, "puedo");
    assert!(!verb.is_punctuation);
    assert_eq!(verb.contextual_translations, vec!["могу", "умею"]);
    assert_eq!(verb.note.as_deref(), Some("first-person singular"));
    assert_eq!(verb.grammar.original_initial_form, "poder");
    assert_eq!(verb.grammar.target_initial_form, "мочь");
    assert_eq!(verb.grammar.part_of_speech, "verb");
    assert_eq!(verb.grammar.plurality.as_deref(), Some("singular"));
    assert_eq!(verb.grammar.person.as_deref(), Some("1"));
    assert_eq!(verb.grammar.tense.as_deref(), Some("present"));
    assert_eq!(verb.grammar.case, None);
    assert_eq!(verb.grammar.other, None);

    let punct = &sentence.words[1];
    assert_eq!(punct.original, ".");
    assert!(punct.is_punctuation);
    assert_eq!(punct.note, None);
    assert_eq!(punct.grammar.part_of_speech, "punctuation");
}

#[test]
fn to_import_round_trip_via_add_paragraph_translation() {
    // Only round-trip-stable conventions: a note is `None` or non-empty, since
    // `Some("")` decodes back as `None`.
    let input = translation_import::ParagraphTranslation {
        timestamp: 1234567890,
        total_tokens: Some(256),
        sentences: vec![
            translation_import::Sentence {
                full_translation: "Я могу есть.".into(),
                words: vec![
                    translation_import::Word {
                        original: "puedo".into(),
                        contextual_translations: vec!["могу".into()],
                        note: Some("modal".into()),
                        is_punctuation: false,
                        grammar: translation_import::Grammar {
                            original_initial_form: "poder".into(),
                            target_initial_form: "мочь".into(),
                            part_of_speech: "verb".into(),
                            plurality: Some("singular".into()),
                            person: Some("1".into()),
                            tense: Some("present".into()),
                            case: None,
                            other: Some("aux".into()),
                        },
                    },
                    translation_import::Word {
                        original: "comer".into(),
                        contextual_translations: vec!["есть".into(), "кушать".into()],
                        note: None,
                        is_punctuation: false,
                        grammar: translation_import::Grammar {
                            original_initial_form: "comer".into(),
                            target_initial_form: "есть".into(),
                            part_of_speech: "verb".into(),
                            plurality: None,
                            person: None,
                            tense: Some("infinitive".into()),
                            case: None,
                            other: None,
                        },
                    },
                    translation_import::Word {
                        original: ".".into(),
                        contextual_translations: vec![".".into()],
                        note: None,
                        is_punctuation: true,
                        grammar: translation_import::Grammar {
                            original_initial_form: ".".into(),
                            target_initial_form: ".".into(),
                            part_of_speech: "punctuation".into(),
                            plurality: None,
                            person: None,
                            tense: None,
                            case: None,
                            other: None,
                        },
                    },
                ],
            },
            translation_import::Sentence {
                full_translation: "Хорошо.".into(),
                words: vec![translation_import::Word {
                    original: "bueno".into(),
                    contextual_translations: vec!["хорошо".into()],
                    note: None,
                    is_punctuation: false,
                    grammar: translation_import::Grammar {
                        original_initial_form: "bueno".into(),
                        target_initial_form: "хорошо".into(),
                        part_of_speech: "adjective".into(),
                        plurality: None,
                        person: None,
                        tense: None,
                        case: Some("nominative".into()),
                        other: None,
                    },
                }],
            },
        ],
    };

    let mut translation = Translation::create("spa", "rus");
    translation.add_paragraph_translation(0, &input, "models/gemini-2.5-flash");

    let view = translation.paragraph_view(0).unwrap();
    let out = view.to_import();

    assert_eq!(out, input);
}

#[test]
fn translation_model_field_round_trips_discovered_id() {
    let mut t = Translation::create("en", "ru");
    t.add_paragraph_translation(0, &make_paragraph(1, "hi"), "models/gemini-9-ultra");
    let mut buf = Vec::new();
    t.serialize(&mut buf).unwrap();
    let back = Translation::deserialize(&mut std::io::Cursor::new(&buf)).unwrap();
    assert_eq!(
        back.paragraph_view(0).unwrap().model,
        "models/gemini-9-ultra"
    );
}

#[test]
fn read_model_field_accepts_varint_only_legacy_blob() {
    let mut blob = Vec::new();
    crate::book::serialization::write_var_u64(&mut blob, 1).unwrap(); // tag
    crate::book::serialization::write_var_u64(&mut blob, 2).unwrap(); // Gemini 2.5 Pro
    assert_eq!(read_model_field(&blob).unwrap(), "models/gemini-2.5-pro");
}

#[test]
fn clone_shares_chunks_and_write_isolates() {
    let mut translation = Translation::create("en", "ru");
    translation.add_paragraph_translation(0, &make_paragraph(1, "first"), "m");
    let snapshot = translation.clone();
    let mut snapshot_bytes = Vec::new();
    snapshot.serialize(&mut snapshot_bytes).unwrap();

    translation.add_paragraph_translation(1, &make_paragraph(2, "second"), "m");
    translation.add_paragraph_translation(0, &make_paragraph(3, "first again"), "m");

    assert_eq!(snapshot.translated_paragraphs_count(), 1);
    assert_eq!(
        snapshot
            .paragraph_view(0)
            .unwrap()
            .sentence_view(0)
            .full_translation,
        "first"
    );
    assert!(snapshot.paragraph_view(1).is_none());
    let mut snapshot_bytes_after = Vec::new();
    snapshot.serialize(&mut snapshot_bytes_after).unwrap();
    assert_eq!(snapshot_bytes, snapshot_bytes_after);

    let mut fresh = Translation::create("en", "ru");
    fresh.id = translation.id;
    fresh.add_paragraph_translation(0, &make_paragraph(1, "first"), "m");
    fresh.add_paragraph_translation(1, &make_paragraph(2, "second"), "m");
    fresh.add_paragraph_translation(0, &make_paragraph(3, "first again"), "m");
    let mut fresh_bytes = Vec::new();
    fresh.serialize(&mut fresh_bytes).unwrap();
    let mut written_bytes = Vec::new();
    translation.serialize(&mut written_bytes).unwrap();
    assert_eq!(fresh_bytes, written_bytes);
}

#[test]
fn model_names_are_interned_across_versions_snapshots_and_loading() {
    let mut current = Translation::create("eng", "rus");
    for id in 0..5000 {
        current.add_paragraph_translation(id, &make_paragraph(1, "word"), "model-a");
    }
    assert_eq!(current.models.index.len(), 1);
    assert_eq!(current.models.strings.len(), "model-a".len());
    let old = current.clone();
    current.add_paragraph_translation(0, &make_paragraph(2, "new"), "model-b");
    assert_eq!(old.paragraph_view(0).unwrap().model, "model-a");
    assert_eq!(current.paragraph_view(0).unwrap().model, "model-b");
    assert_eq!(
        current
            .paragraph_view(0)
            .unwrap()
            .get_previous_version()
            .unwrap()
            .model,
        "model-a"
    );
    let mut bytes = Vec::new();
    current.serialize(&mut bytes).unwrap();
    let loaded = Translation::deserialize(&mut Cursor::new(&bytes)).unwrap();
    assert_eq!(loaded.models.index.len(), 3); // includes the legacy empty default
    assert_eq!(loaded.paragraph_view(0).unwrap().model, "model-b");
    assert_eq!(
        loaded
            .paragraph_view(0)
            .unwrap()
            .get_previous_version()
            .unwrap()
            .model,
        "model-a"
    );
    let mut roundtrip = Vec::new();
    loaded.serialize(&mut roundtrip).unwrap();
    assert_eq!(bytes, roundtrip);
}

#[test]
fn strings_spanning_chunks_round_trip_and_encode_like_a_flat_buffer() {
    let mut translation = Translation::create("eng", "rus");
    // Unique strings, so dedup cannot keep the arena inside one chunk.
    for id in 0..4000 {
        let text = format!("paragraph {id} {}", "x".repeat(60));
        translation.add_paragraph_translation(id, &make_paragraph(1, &text), "m");
    }
    assert!(translation.strings.chunks().count() > 2);
    let flat = translation.strings.chunks().collect::<Vec<_>>().concat();
    assert_eq!(
        translation.encode_strings().unwrap(),
        zstd::stream::encode_all(flat.as_slice(), -7).unwrap()
    );

    let mut bytes = Vec::new();
    translation.serialize(&mut bytes).unwrap();
    let loaded = Translation::deserialize(&mut Cursor::new(&bytes)).unwrap();
    assert_eq!(loaded.strings.chunks().collect::<Vec<_>>().concat(), flat);
    for id in [0, 1999, 3999] {
        assert_eq!(
            loaded
                .paragraph_view(id)
                .unwrap()
                .sentence_view(0)
                .full_translation,
            translation
                .paragraph_view(id)
                .unwrap()
                .sentence_view(0)
                .full_translation
        );
    }
    let mut roundtrip = Vec::new();
    loaded.serialize(&mut roundtrip).unwrap();
    assert_eq!(bytes, roundtrip);
}

#[test]
fn is_paragraph_translated_matches_paragraph_view() {
    let mut translation = Translation::create("eng", "rus");
    translation.add_paragraph_translation(0, &make_paragraph(1, "real"), "m");
    translation.add_paragraph_translation(2, &make_paragraph(1, "hidden"), "m");
    translation.add_paragraph_translation(2, &make_empty_paragraph(2), "m");
    for id in 0..5 {
        assert_eq!(
            translation.is_paragraph_translated(id),
            translation.paragraph_view(id).is_some(),
            "paragraph {id}"
        );
    }
    assert!(translation.is_paragraph_translated(0));
    assert!(!translation.is_paragraph_translated(2));
}
