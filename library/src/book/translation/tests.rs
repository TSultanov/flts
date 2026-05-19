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
        source_language: "en".to_owned(),
        target_language: "ru".to_owned(),
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
        source_language: "en".to_owned(),
        target_language: "ru".to_owned(),
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
    let mut dict = Dictionary::create("en".to_owned(), "ru".to_owned());
    translation.add_paragraph_translation(
        0,
        &paragraph_translation,
        TranslationModel::Gemini25Pro,
        &mut dict,
    );
    let paragraph_view = translation.paragraph_view(0).unwrap();
    assert_eq!(paragraph_view.timestamp, 1234567890);
    assert_eq!(paragraph_view.previous_version, None);
    assert_eq!(paragraph_view.sentence_count(), 1);
    assert_eq!(paragraph_view.model, TranslationModel::Gemini25Pro);
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

    // second version
    let paragraph_translation2 = translation_import::ParagraphTranslation {
        total_tokens: Some(4321),
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
        0,
        &paragraph_translation2,
        TranslationModel::Gemini25FlashLight,
        &mut dict,
    );

    let mut buf: Vec<u8> = vec![];
    translation.serialize(&mut buf).unwrap();
    let mut cursor = Cursor::new(buf);
    let translation2 = Translation::deserialize(&mut cursor).unwrap();

    assert_eq!(translation2.source_language, "en");
    assert_eq!(translation2.target_language, "ru");
    // Latest paragraph view
    let latest = translation2.paragraph_view(0).unwrap();
    assert_eq!(latest.sentence_count(), 1);
    assert_eq!(latest.model, TranslationModel::Gemini25FlashLight);
    assert_eq!(latest.total_tokens, Some(4321));
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
fn translation_serialize_v1_deserialize_round_trip() {
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

    // second version
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
        0,
        &paragraph_translation2,
        TranslationModel::Gemini25FlashLight,
        &mut dict,
    );

    let mut buf: Vec<u8> = vec![];
    translation.serialize_v1(&mut buf).unwrap();
    let mut cursor = Cursor::new(buf);
    let translation2 = Translation::deserialize(&mut cursor).unwrap();

    assert_eq!(translation2.source_language, "en");
    assert_eq!(translation2.target_language, "ru");
    // Latest paragraph view
    let latest = translation2.paragraph_view(0).unwrap();
    assert_eq!(latest.sentence_count(), 1);
    assert_eq!(latest.model, TranslationModel::Unknown);
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

    // second version
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
        0,
        &paragraph_translation2,
        TranslationModel::Gemini25Flash,
        &mut dict,
    );

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
    let mut dict = Dictionary::create("en".to_owned(), "ru".to_owned());
    let mut a = Translation::create("en", "ru");
    a.add_paragraph_translation(
        0,
        &make_paragraph(1, "v1"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );
    a.add_paragraph_translation(
        0,
        &make_paragraph(2, "v2"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );

    let mut b = Translation::create("en", "ru");
    b.add_paragraph_translation(
        0,
        &make_paragraph(1, "v1"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );
    b.add_paragraph_translation(
        0,
        &make_paragraph(2, "v2"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );

    let merged = a.merge(&b);

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
    let mut dict = Dictionary::create("en".to_owned(), "ru".to_owned());
    // a: 1 -> 2 -> 4
    let mut a = Translation::create("en", "ru");
    a.add_paragraph_translation(
        0,
        &make_paragraph(1, "a1"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );
    a.add_paragraph_translation(
        0,
        &make_paragraph(2, "a2"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );
    a.add_paragraph_translation(
        0,
        &make_paragraph(4, "a4"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );

    // b: 1 -> 3 -> 5
    let mut b = Translation::create("en", "ru");
    b.add_paragraph_translation(
        0,
        &make_paragraph(1, "a1"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    ); // same ts as a1 (dedup)
    b.add_paragraph_translation(
        0,
        &make_paragraph(3, "a3"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );
    b.add_paragraph_translation(
        0,
        &make_paragraph(5, "a5"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );

    let merged = a.merge(&b);

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
    assert_eq!(
        merged
            .paragraph_view(0)
            .unwrap()
            .sentence_view(0)
            .full_translation,
        "a5"
    );
    let v4 = merged
        .paragraph_view(0)
        .unwrap()
        .get_previous_version()
        .unwrap();
    assert_eq!(v4.sentence_view(0).full_translation, "a4");
}

#[test]
fn merge_no_common_root() {
    let mut dict = Dictionary::create("en".to_owned(), "ru".to_owned());
    // a: 10 -> 20
    let mut a = Translation::create("en", "ru");
    a.add_paragraph_translation(
        0,
        &make_paragraph(10, "a10"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );
    a.add_paragraph_translation(
        0,
        &make_paragraph(20, "a20"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );

    // b: 5 -> 15 -> 25
    let mut b = Translation::create("en", "ru");
    b.add_paragraph_translation(
        0,
        &make_paragraph(5, "b5"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );
    b.add_paragraph_translation(
        0,
        &make_paragraph(15, "b15"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );
    b.add_paragraph_translation(
        0,
        &make_paragraph(25, "b25"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );

    let merged = a.merge(&b);
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
    let mut dict = Dictionary::create("en".to_owned(), "ru".to_owned());
    // Paragraph 0 only in left, with history 1 -> 2
    let mut a = Translation::create("en", "ru");
    a.add_paragraph_translation(
        0,
        &make_paragraph(1, "a1"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );
    a.add_paragraph_translation(
        0,
        &make_paragraph(2, "a2"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );
    // Paragraph 1 only in right, with single version 3
    let b = {
        let mut t = Translation::create("en", "ru");
        t.add_paragraph_translation(
            1,
            &make_paragraph(3, "b3"),
            TranslationModel::Gemini25Flash,
            &mut dict,
        );
        t
    };

    let merged = a.merge(&b);

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

#[test]
fn visible_words_serialize_deserialize_roundtrip() {
    let mut dict = Dictionary::create("en".to_owned(), "ru".to_owned());
    let mut translation = Translation::create("en", "ru");
    translation.add_paragraph_translation(
        0,
        &make_paragraph(1, "test"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );

    // Mark some words as visible
    assert!(translation.mark_word_visible(0, 2));
    assert!(translation.mark_word_visible(0, 5));
    assert!(translation.mark_word_visible(0, 3));
    // Marking same word again should return false
    assert!(!translation.mark_word_visible(0, 2));

    // Verify visible_words before serialization
    let view = translation.paragraph_view(0).unwrap();
    let mut words: Vec<_> = view.visible_words().iter().copied().collect();
    words.sort();
    assert_eq!(words, vec![2, 3, 5]);

    // Serialize and deserialize
    let mut buf: Vec<u8> = vec![];
    translation.serialize(&mut buf).unwrap();
    let mut cursor = Cursor::new(buf);
    let deserialized = Translation::deserialize(&mut cursor).unwrap();

    // Verify visible_words after deserialization
    let view2 = deserialized.paragraph_view(0).unwrap();
    let mut words2: Vec<_> = view2.visible_words().iter().copied().collect();
    words2.sort();
    assert_eq!(words2, vec![2, 3, 5]);
}

#[test]
fn merge_visible_words_union() {
    let mut dict = Dictionary::create("en".to_owned(), "ru".to_owned());

    // Create two translations with same timestamp but different visible_words
    let mut a = Translation::create("en", "ru");
    a.add_paragraph_translation(
        0,
        &make_paragraph(1, "shared"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );
    a.mark_word_visible(0, 1);
    a.mark_word_visible(0, 3);

    let mut b = Translation::create("en", "ru");
    b.add_paragraph_translation(
        0,
        &make_paragraph(1, "shared"),
        TranslationModel::Gemini25Flash,
        &mut dict,
    );
    b.mark_word_visible(0, 2);
    b.mark_word_visible(0, 3); // Overlaps with a

    // Merge
    let merged = a.merge(&b);

    // Verify visible_words is the union of both sources
    let view = merged.paragraph_view(0).unwrap();
    let mut visible: Vec<usize> = view.visible_words().iter().copied().collect();
    visible.sort();
    assert_eq!(visible, vec![1, 2, 3]); // Union of [1, 3] and [2, 3]
}
