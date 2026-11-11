mod support {
    pub mod allocations;
}
use criterion::{Criterion, criterion_group, criterion_main};
use library::book::translation::Translation;
use library::book::translation_import::{Grammar, ParagraphTranslation, Sentence, Word};
use rand::distr::Alphanumeric;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use support::allocations::measure;

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

fn bench_snapshot_and_cow_update(c: &mut Criterion) {
    use std::{hint::black_box, sync::Arc};
    let original = Arc::new(measure("build 5000 paragraphs", || {
        generate_translation(5000)
    }));
    std::hint::black_box(measure("retain translation snapshot", || original.clone()));
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
    std::hint::black_box(measure("update paragraph with retained snapshot", || {
        let mut updated = (*original).clone();
        updated.add_paragraph_translation(2500, &paragraph, "benchmark");
        updated
    }));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("library");
    let mut source = library::book::book::Book::create(
        uuid::Uuid::new_v4(),
        "Snapshot benchmark",
        &isolang::Language::Eng,
    );
    source.push_chapter(None);
    for _ in 0..5000 {
        source.push_paragraph(0, "source", None);
    }
    let id = source.id;
    let library = runtime
        .block_on(library::library::Library::open(root.clone()))
        .unwrap();
    runtime.block_on(async {
        let handle = library.create_book(source).await.unwrap();
        for paragraph in 0..5000 {
            handle
                .apply_translation(
                    paragraph,
                    isolang::Language::Spa,
                    original.paragraph_view(paragraph).unwrap().to_import(),
                    "benchmark".into(),
                )
                .await
                .unwrap();
        }
        handle.flush().await.unwrap();
        drop(handle);
        library.close().await.unwrap();
    });
    drop(library);
    let library = runtime
        .block_on(library::library::Library::open(root))
        .unwrap();
    let handle = measure("load 5000 paragraphs from the store", || {
        runtime.block_on(library.get_book(&id)).unwrap()
    });
    std::hint::black_box(measure("read book snapshot", || handle.snapshot()));
    c.bench_function("read book snapshot (5000 paragraphs)", |b| {
        b.iter(|| black_box(handle.snapshot()))
    });
    runtime.block_on(library.close()).unwrap();
}

criterion_group!(benches, bench_snapshot_and_cow_update);
criterion_main!(benches);
