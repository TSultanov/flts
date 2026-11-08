//! Validation and mutation must run together on the current book owner.
use isolang::Language;
use library::{book::translation_import::ParagraphTranslation, library::Library};

#[tokio::test]
async fn rejects_changed_source_and_wrong_owner_tokens_without_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let library = Library::open(directory.path().join("library"))
        .await
        .unwrap();
    let source = Language::from_639_3("eng").unwrap();
    let target = Language::from_639_3("rus").unwrap();
    let id = library
        .create_book_plain("Stale result", "current text", &source)
        .await
        .unwrap();
    let handle = library.get_book(&id).await.unwrap();
    let result = || ParagraphTranslation {
        timestamp: 1,
        sentences: vec![],
        total_tokens: None,
    };
    assert!(
        handle
            .apply_translation(
                handle.token,
                0,
                "old text".into(),
                source,
                target,
                result(),
                "test".into()
            )
            .await
            .is_err()
    );
    let mut wrong = handle.token;
    wrong.incarnation = uuid::Uuid::new_v4();
    assert!(
        handle
            .apply_translation(
                wrong,
                0,
                "current text".into(),
                source,
                target,
                result(),
                "test".into()
            )
            .await
            .is_err()
    );
    assert!(handle.snapshot().translation(&target).is_none());
    library.close().await.unwrap();
}

#[tokio::test]
async fn old_handle_cannot_resurrect_a_deleted_book() {
    let directory = tempfile::tempdir().unwrap();
    let library = Library::open(directory.path().join("library"))
        .await
        .unwrap();
    let id = library
        .create_book_plain("Deleted", "text", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    let old = library.get_book(&id).await.unwrap();
    let snapshot = old.snapshot();
    library.delete_book(&id).await.unwrap();
    assert!(old.save().await.is_err());
    assert!(
        old.update_folder_path(vec!["resurrect".into()])
            .await
            .is_err()
    );
    assert!(library.get_book(&id).await.is_err());
    assert!(!snapshot.path.exists());
    assert_eq!(snapshot.book.title, "Deleted");
    library.delete_book(&id).await.unwrap();
    library.close().await.unwrap();
}
