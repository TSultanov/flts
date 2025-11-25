use criterion::{criterion_group, criterion_main, Criterion};
use library::book::translation::Translation;
use library::book::translation_import::{ParagraphTranslation, Sentence, Word, Grammar};
use library::dictionary::Dictionary;
use library::translator::TranslationModel;
use library::book::serialization::Serializable;
use std::io::Cursor;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;
use rand::distr::Alphanumeric;

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
        note: if rng.random_bool(0.2) { Some(random_string(rng, 10)) } else { None },
        is_punctuation: false,
        grammar: Grammar {
            original_initial_form: random_string(rng, 5),
            target_initial_form: random_string(rng, 5),
            part_of_speech: random_string(rng, 4),
            plurality: if rng.random_bool(0.1) { Some(random_string(rng, 4)) } else { None },
            person: if rng.random_bool(0.1) { Some(random_string(rng, 3)) } else { None },
            tense: if rng.random_bool(0.1) { Some(random_string(rng, 4)) } else { None },
            case: if rng.random_bool(0.1) { Some(random_string(rng, 3)) } else { None },
            other: if rng.random_bool(0.05) { Some(random_string(rng, 10)) } else { None },
        },
    }
}

fn generate_translation(paragraphs_count: usize) -> Translation {
    let mut translation = Translation::create("eng", "spa");
    let mut dictionary = Dictionary::create("eng".to_string(), "spa".to_string());
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
            source_language: "eng".to_string(),
            target_language: "spa".to_string(),
            total_tokens: Some(rng.random_range(50..200)),
        };
        
        translation.add_paragraph_translation(
            i,
            &paragraph_translation,
            TranslationModel::Gemini25Flash,
            &mut dictionary
        );
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

criterion_group!(benches, bench_translation_serialization, bench_translation_deserialization);
criterion_main!(benches);
