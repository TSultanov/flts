//! A copy of an installation takes an identity of its own, against the real
//! embedded engine: `cargo test -p library --features sync-engine --test sync_identity`.
#![cfg(feature = "sync-engine")]

use library::{
    library::Library,
    sync::{
        control::OptionsPatch,
        engine::{EngineConfig, SyncEngine, forget_identity},
    },
};

async fn start(home: &std::path::Path, root: &std::path::Path) -> SyncEngine {
    SyncEngine::start(EngineConfig {
        home: home.to_owned(),
        library_root: root.to_owned(),
        options: OptionsPatch::loopback(),
        loopback_only: true,
    })
    .await
    .expect("the embedded engine starts")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_identity_is_a_new_device_and_a_new_writer() {
    let dir = tempfile::tempdir().unwrap();
    let (home, root) = (dir.path().join("syncthing"), dir.path().join("library"));
    let library = Library::open(root.clone()).await.unwrap();
    library
        .create_book_plain(
            "Kept",
            "one\ntwo",
            &isolang::Language::from_639_3("eng").unwrap(),
        )
        .await
        .unwrap();

    let engine = start(&home, &root).await;
    let before = engine.my_id().to_string();
    engine.set_device_name("this device").await.unwrap();
    assert!(engine.retired_writers().unwrap().is_empty());
    engine.stop().await.unwrap();
    drop(engine);

    // What the app does when the user has this copy take a new identity.
    library.note_shared_identity().await.unwrap();
    forget_identity(&home).await.unwrap();
    library.take_new_identity().await.unwrap();

    let engine = start(&home, &root).await;
    let after = engine.my_id().to_string();
    assert_ne!(after, before, "a device the others have never seen");
    engine.set_device_name("this device").await.unwrap();
    // The earlier device is still listed, with the writer it had; this one
    // is listed beside it, and its own writer is not among the retired.
    engine.reconcile_once().await.unwrap();
    let retired = engine.retired_writers().unwrap();
    assert!(
        retired.is_empty(),
        "the copy it came from may still be in use: {retired:?}"
    );
    assert!(!library.store_health().await.unwrap().shared_identity);
    assert_eq!(library.list_books().await.unwrap().len(), 1);
    engine.stop().await.unwrap();
    library.close().await.unwrap();
}
