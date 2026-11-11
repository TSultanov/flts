//! Accepted mutations belong to the library even if their caller disappears.
use isolang::Language;
use library::{book::translation_import::ParagraphTranslation, library::Library};

#[tokio::test]
async fn accepted_translation_survives_close_without_a_separate_saver() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("library");
    let library = Library::open(root.clone()).await.unwrap();
    let source = Language::from_639_3("eng").unwrap();
    let target = Language::from_639_3("rus").unwrap();
    let id = library
        .create_book_plain("Owned save", "hello", &source)
        .await
        .unwrap();
    let handle = library.get_book(&id).await.unwrap();
    handle
        .apply_translation(
            0,
            target,
            ParagraphTranslation {
                timestamp: 1,
                sentences: vec![library::book::translation_import::Sentence {
                    full_translation: "hello".into(),
                    words: vec![],
                }],
                total_tokens: None,
            },
            "test".into(),
        )
        .await
        .unwrap();
    library.close().await.unwrap();
    let reopened = Library::open(root).await.unwrap();
    assert!(
        reopened
            .get_book(&id)
            .await
            .unwrap()
            .snapshot()
            .translation(&target)
            .unwrap()
            .paragraph_view(0)
            .is_some()
    );
    assert!(handle.flush().await.is_err());
    reopened.close().await.unwrap();
}
