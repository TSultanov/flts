//! Two devices translating the same paragraph in the same second must both
//! keep their work once their stores meet.

use std::path::Path;

use isolang::Language;
use library::{
    book::{book::Book, translation_import},
    library::{BookHandle, Library},
};

fn make_paragraph(ts: u64, text: &str) -> translation_import::ParagraphTranslation {
    translation_import::ParagraphTranslation {
        total_tokens: None,
        timestamp: ts,
        sentences: vec![translation_import::Sentence {
            full_translation: text.into(),
            words: vec![translation_import::Word {
                original: text.into(),
                contextual_translations: vec![text.into()],
                note: Some(String::new()),
                is_punctuation: false,
                grammar: translation_import::Grammar {
                    original_initial_form: text.into(),
                    target_initial_form: text.into(),
                    part_of_speech: "n".into(),
                    plurality: None,
                    person: None,
                    tense: None,
                    case: None,
                    other: None,
                },
            }],
        }],
    }
}

/// Stands in for the file sync between two devices' shard directories.
fn copy_shards(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_shards(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

async fn apply_fixture(
    handle: &BookHandle,
    target: Language,
    paragraph: usize,
    translation: translation_import::ParagraphTranslation,
) {
    handle
        .apply_translation(
            paragraph,
            target,
            translation,
            "models/gemini-2.5-flash".into(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn same_timestamp_translations_from_two_devices_both_survive() {
    let directory = tempfile::tempdir().unwrap();
    let here = Library::open(directory.path().join("here/lib"))
        .await
        .unwrap();
    let there = Library::open(directory.path().join("there/lib"))
        .await
        .unwrap();
    let source_language = Language::from_639_3("eng").unwrap();
    let target_language = Language::from_639_3("rus").unwrap();

    let mut new_book = Book::create(uuid::Uuid::new_v4(), "Translation Base", &source_language);
    new_book.push_chapter(Some("Intro"));
    new_book.push_paragraph(0, "source paragraph", None);
    let book_id = new_book.id;
    let ours = here.create_book(new_book).await.unwrap();
    ours.flush().await.unwrap();
    copy_shards(here.store().shared_dir(), there.store().shared_dir());
    there.refresh_from_peers().await.unwrap();
    let theirs = there.get_book(&book_id).await.unwrap();

    apply_fixture(&ours, target_language, 0, make_paragraph(1, "main version")).await;
    ours.flush().await.unwrap();
    apply_fixture(
        &theirs,
        target_language,
        0,
        make_paragraph(1, "conflict version"),
    )
    .await;
    theirs.flush().await.unwrap();

    copy_shards(here.store().shared_dir(), there.store().shared_dir());
    copy_shards(there.store().shared_dir(), here.store().shared_dir());
    assert_eq!(
        here.refresh_from_peers().await.unwrap().paragraphs,
        [(book_id, 0)]
    );
    assert_eq!(
        there.refresh_from_peers().await.unwrap().paragraphs,
        [(book_id, 0)]
    );

    let mut histories = Vec::new();
    for book in [&ours, &theirs] {
        let versions = book
            .read(move |book| book.versions(&target_language, 0))
            .await
            .unwrap();
        let [previous, latest] = versions.as_slice() else {
            panic!("both versions survive: one latest, one previous");
        };
        let current = book
            .read(move |book| book.translation(&target_language, 0))
            .await
            .unwrap();
        // Same version; only the history gives colliding stamps distinct ones.
        assert_eq!(
            current.map(|c| c.translation.sentences),
            Some(latest.translation.sentences.clone())
        );
        // The collision is resolved by bumping one timestamp.
        assert!(previous.translation.timestamp < latest.translation.timestamp);
        let texts = [
            previous.translation.sentences[0].full_translation.clone(),
            latest.translation.sentences[0].full_translation.clone(),
        ];
        assert!(texts.contains(&"main version".to_string()));
        assert!(texts.contains(&"conflict version".to_string()));
        histories.push(texts);
    }
    assert_eq!(
        histories[0], histories[1],
        "both devices order the two versions the same way"
    );
    here.close().await.unwrap();
    there.close().await.unwrap();
}
