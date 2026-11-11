//! A handle that outlives its book must not bring it back.
use isolang::Language;
use library::library::Library;

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
    assert!(old.flush().await.is_err());
    assert!(
        old.update_folder_path(vec!["resurrect".into()])
            .await
            .is_err()
    );
    assert!(library.get_book(&id).await.is_err());
    assert!(library.list_books().await.unwrap().is_empty());
    assert_eq!(snapshot.book.title, "Deleted");
    library.delete_book(&id).await.unwrap();
    library.close().await.unwrap();
}
