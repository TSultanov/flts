use criterion::{Criterion, criterion_group, criterion_main};
use library::book::serialization::Serializable;
use library::book::translation::Translation;
use library::book::translation_import::{Grammar, ParagraphTranslation, Sentence, Word};
use rand::distr::Alphanumeric;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::io::Cursor;

fn random_string(rng: &mut StdRng, len: usize) -> String {
    rng.sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn generate_random_word(rng: &mut StdRng) -> Word {
    Word {
        original: random_string(rng, 5),
        contextual_translations: vec![random_string(rng, 5)],
        note: if rng.random_bool(0.2) {
            Some(random_string(rng, 10))
        } else {
            None
        },
        is_punctuation: false,
        grammar: Grammar {
            original_initial_form: random_string(rng, 5),
            target_initial_form: random_string(rng, 5),
            part_of_speech: random_string(rng, 4),
            plurality: if rng.random_bool(0.1) {
                Some(random_string(rng, 4))
            } else {
                None
            },
            person: if rng.random_bool(0.1) {
                Some(random_string(rng, 3))
            } else {
                None
            },
            tense: if rng.random_bool(0.1) {
                Some(random_string(rng, 4))
            } else {
                None
            },
            case: if rng.random_bool(0.1) {
                Some(random_string(rng, 3))
            } else {
                None
            },
            other: if rng.random_bool(0.05) {
                Some(random_string(rng, 10))
            } else {
                None
            },
        },
    }
}

fn generate_translation(paragraphs_count: usize) -> Translation {
    let mut translation = Translation::create("eng", "spa");
    let mut rng = StdRng::seed_from_u64(42);

    for i in 0..paragraphs_count {
        let sentences_count = rng.random_range(1..=3);
        let mut sentences = Vec::with_capacity(sentences_count);

        for _ in 0..sentences_count {
            let words_count = rng.random_range(5..=15);
            let mut words = Vec::with_capacity(words_count);
            for _ in 0..words_count {
                words.push(generate_random_word(&mut rng));
            }

            sentences.push(Sentence {
                full_translation: random_string(&mut rng, 50),
                words,
            });
        }

        let paragraph_translation = ParagraphTranslation {
            timestamp: 1234567890,
            sentences,
            total_tokens: Some(rng.random_range(50..200)),
        };

        translation.add_paragraph_translation(i, &paragraph_translation, "models/gemini-2.5-flash");
    }
    translation
}

fn bench_translation_serialization(c: &mut Criterion) {
    // 5000 paragraphs, ~2 sentences each, ~10 words each -> ~100,000 words.
    // With random strings, this should be substantial.
    let translation = generate_translation(5000);

    c.bench_function("serialize translation (5000 paragraphs, random)", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            translation.serialize(&mut buffer).unwrap();
        })
    });
}

fn bench_translation_deserialization(c: &mut Criterion) {
    let translation = generate_translation(5000);
    let mut buffer = Vec::new();
    translation.serialize(&mut buffer).unwrap();

    c.bench_function("deserialize translation (5000 paragraphs, random)", |b| {
        b.iter(|| {
            let mut cursor = Cursor::new(&buffer);
            Translation::deserialize(&mut cursor).unwrap();
        })
    });
}

fn bench_snapshot_and_cow_update(c: &mut Criterion) {
    use std::{hint::black_box, sync::Arc};
    let original = Arc::new(generate_translation(5000));
    c.bench_function("retain translation snapshot (5000 paragraphs)", |b| {
        b.iter(|| black_box(original.clone()))
    });
    let paragraph = ParagraphTranslation {
        timestamp: 999999,
        sentences: vec![Sentence {
            full_translation: "updated paragraph".into(),
            words: vec![],
        }],
        total_tokens: None,
    };
    c.bench_function(
        "copy-on-write one paragraph (5000 paragraphs, old snapshot retained)",
        |b| {
            b.iter(|| {
                let mut updated = (*original).clone();
                updated.add_paragraph_translation(2500, &paragraph, "benchmark");
                black_box(updated);
            })
        },
    );
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let id = uuid::Uuid::new_v4();
    let path = directory.path().join(id.to_string());
    std::fs::create_dir_all(&path).unwrap();
    let mut source =
        library::book::book::Book::create(id, "Snapshot benchmark", &isolang::Language::Eng);
    source.push_chapter(None);
    for _ in 0..5000 {
        source.push_paragraph(0, "source", None);
    }
    let mut bytes = Vec::new();
    source.serialize(&mut bytes).unwrap();
    std::fs::write(path.join("book.dat"), &bytes).unwrap();
    bytes.clear();
    original.serialize(&mut bytes).unwrap();
    std::fs::write(path.join("translation_eng_spa.dat"), &bytes).unwrap();
    let library = runtime
        .block_on(library::library::Library::open(
            directory.path().to_path_buf(),
        ))
        .unwrap();
    let handle = runtime.block_on(library.get_book(&id)).unwrap();
    c.bench_function("read book snapshot (5000 paragraphs)", |b| {
        b.iter(|| black_box(handle.snapshot()))
    });
    runtime.block_on(library.close()).unwrap();
}

criterion_group!(
    benches,
    bench_translation_serialization,
    bench_translation_deserialization,
    bench_snapshot_and_cow_update
);
criterion_main!(benches);
