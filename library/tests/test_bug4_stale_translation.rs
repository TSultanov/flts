//! A handle that outlives its book must not bring it back.
use isolang::Language;
use library::{book::translation_import::ParagraphTranslation, library::Library};

#[tokio::test]
async fn old_handle_cannot_resurrect_a_deleted_book() {
    let directory = tempfile::tempdir().unwrap();
    let library = Library::open(directory.path().join("library"))
        .await
        .unwrap();
    let rus = Language::from_639_3("rus").unwrap();
    let id = library
        .create_book_plain("Deleted", "text", &Language::from_639_3("eng").unwrap())
        .await
        .unwrap();
    let old = library.get_book(&id).await.unwrap();
    library.delete_book(&id).await.unwrap();
    let translation = ParagraphTranslation {
        timestamp: 1,
        total_tokens: None,
        sentences: vec![],
    };
    assert!(
        old.apply_translation(0, rus, translation, "model".into())
            .await
            .is_err()
    );
    assert!(old.read(|book| book.paragraph(0)).await.is_err());
    assert!(
        old.update_folder_path(vec!["resurrect".into()])
            .await
            .is_err()
    );
    assert!(library.get_book(&id).await.is_err());
    assert!(library.list_books().await.unwrap().is_empty());
    assert_eq!(old.info().title, "Deleted");
    library.delete_book(&id).await.unwrap();
    library.close().await.unwrap();
}
