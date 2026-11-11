use std::time::Duration;

use isolang::Language;
use uuid::Uuid;

use super::*;
use crate::{
    book::chapter_summaries::ChapterSummary,
    library::{LibraryError, PeerChanges},
    test_utils::{
        TempDir, copy_shards, copy_shards_where, full_word, one_sentence_paragraph, peer_view,
    },
};

fn eng() -> Language {
    Language::from_639_3("eng").unwrap()
}

fn rus() -> Language {
    Language::from_639_3("rus").unwrap()
}

fn sample_book(title: &str) -> Book {
    let mut book = Book::create(Uuid::new_v4(), title, &eng());
    let first = book.push_chapter(Some("One"));
    book.push_paragraph(first, "alpha", Some("<p>alpha</p>"));
    book.push_paragraph(first, "beta", None);
    let second = book.push_chapter(None);
    book.push_paragraph(second, "gamma", None);
    book
}

fn version(timestamp: u64, text: &str) -> translation_import::ParagraphTranslation {
    let mut paragraph = one_sentence_paragraph(
        text,
        vec![full_word("alpha", "alpha", "альфа", "noun", &[text], false)],
    );
    paragraph.timestamp = timestamp;
    paragraph.total_tokens = Some(timestamp + 100);
    paragraph
}

fn summary(timestamp: u64, text: &str) -> ChapterSummary {
    ChapterSummary {
        generated: true,
        model: "summary-model".into(),
        timestamp,
        text: text.into(),
    }
}

fn reading(page_offset: usize) -> BookReadingState {
    BookReadingState {
        chapter_id: 1,
        paragraph_id: 2,
        page_offset,
    }
}

async fn stored_book(store: &LibraryStore, source: Book) -> LibraryBook {
    let id = source.id;
    store
        .run(move |db| db.create_book(Arc::new(source)))
        .await
        .unwrap();
    LibraryBook::load(store.clone(), id).await.unwrap()
}

fn fail_commits(store: &LibraryStore, count: usize) {
    store
        .run_blocking(|db| {
            db.fail_commits = count;
            Ok(())
        })
        .unwrap();
}

async fn translate(book: &BookHandle, paragraph: usize, timestamp: u64, text: &str) {
    book.apply_translation(paragraph, rus(), version(timestamp, text), "model-a".into())
        .await
        .unwrap();
}

/// Two libraries standing for two devices that sync one folder.
struct Devices {
    _dir: TempDir,
    a: Library,
    b: Library,
}

impl Devices {
    async fn new() -> Self {
        let dir = TempDir::new("flts_devices");
        let a = Library::open(dir.path.join("a/library")).await.unwrap();
        let b = Library::open(dir.path.join("b/library")).await.unwrap();
        Self { _dir: dir, a, b }
    }

    fn sync_a_to_b(&self) {
        copy_shards(self.a.store().shared_dir(), self.b.store().shared_dir());
    }

    fn sync_b_to_a(&self) {
        copy_shards(self.b.store().shared_dir(), self.a.store().shared_dir());
    }
}

#[tokio::test]
async fn everything_about_a_book_survives_a_reopen() {
    let dir = TempDir::new("flts_book");
    let root = dir.path.join("library");
    let source = sample_book("Round trip");
    let id = source.id;
    {
        let library = Library::open(root.clone()).await.unwrap();
        let book = library.create_book(source.clone()).await.unwrap();
        translate(&book, 0, 10, "старое").await;
        translate(&book, 0, 20, "новое").await;
        translate(&book, 2, 30, "гамма").await;
        book.apply_summary(&book.snapshot(), 0, summary(5, "first chapter"))
            .await
            .unwrap();
        book.update_reading_state(reading(3)).await.unwrap();
        book.update_folder_path(vec!["Shelf".into(), "Modern".into()])
            .await
            .unwrap();
        book.flush().await.unwrap();
        drop(book);
        library.close().await.unwrap();
    }

    let library = Library::open(root).await.unwrap();
    let listed = library.list_books().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].title, "Round trip");
    assert_eq!(
        (listed[0].chapters_count, listed[0].paragraphs_count),
        (2, 3)
    );
    assert_eq!(listed[0].folder_path, ["Shelf", "Modern"]);
    let listed_translation = &listed[0].translations_metadata[0];
    assert_eq!(listed_translation.source_langugage, "eng");
    assert_eq!(listed_translation.target_language, "rus");
    assert_eq!(listed_translation.translated_paragraphs_count, 2);

    let book = library.get_book(&id).await.unwrap();
    let snapshot = book.snapshot();
    assert_eq!(snapshot.book.title, "Round trip");
    assert_eq!(snapshot.book.language, "eng");
    assert_eq!(snapshot.book.chapter_count(), 2);
    assert_eq!(snapshot.book.chapter_view(0).title.as_deref(), Some("One"));
    assert_eq!(snapshot.book.chapter_view(1).title, None);
    for paragraph in 0..3 {
        let (stored, original) = (
            snapshot.book.paragraph_view(paragraph),
            source.paragraph_view(paragraph),
        );
        assert_eq!(stored.original_text, original.original_text);
        assert_eq!(stored.original_html, original.original_html);
    }

    let translation = snapshot.translation(&rus()).unwrap();
    assert_eq!(translation.id, listed_translation.id);
    assert_eq!(translation.version_count(), 3);
    let latest = translation.paragraph_view(0).unwrap();
    assert_eq!(latest.to_import(), version(20, "новое"));
    assert_eq!(latest.model, "model-a");
    let previous = latest.get_previous_version().unwrap();
    assert_eq!(previous.to_import(), version(10, "старое"));
    assert!(previous.get_previous_version().is_none());
    assert!(translation.paragraph_view(1).is_none());
    assert_eq!(
        translation.paragraph_view(2).unwrap().to_import(),
        version(30, "гамма")
    );

    assert_eq!(snapshot.summaries.entries[0], summary(5, "first chapter"));
    assert!(!snapshot.summaries.entries[1].generated);
    assert_eq!(book.reading_state().await.unwrap(), Some(reading(3)));
    assert_eq!(book.folder_path().await.unwrap(), ["Shelf", "Modern"]);
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_new_book_is_listed_at_once() {
    let dir = TempDir::new("flts_book");
    let library = Library::open(dir.path.join("library")).await.unwrap();
    library.create_book(sample_book("New")).await.unwrap();
    assert_eq!(library.list_books().await.unwrap().len(), 1);
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_new_book_is_stored_by_its_creation_alone() {
    let dir = TempDir::new("flts_book");
    let root = dir.path.join("library");
    let library = Library::open(root.clone()).await.unwrap();
    let book = library.create_book(sample_book("Implicit")).await.unwrap();
    let id = book.snapshot().book.id;
    drop(book);
    library.close().await.unwrap();
    drop(library);

    let library = Library::open(root).await.unwrap();
    assert_eq!(library.list_books().await.unwrap()[0].id, id);
    library.close().await.unwrap();
}

#[tokio::test]
async fn duplicate_titles_are_rejected() {
    let dir = TempDir::new("flts_book");
    let library = Library::open(dir.path.join("library")).await.unwrap();
    library.create_book(sample_book("Taken")).await.unwrap();

    let error = library
        .create_book(sample_book("Taken"))
        .await
        .err()
        .expect("a stored title is taken");
    assert!(matches!(
        error.downcast_ref::<LibraryError>(),
        Some(LibraryError::DuplicateTitle(title)) if title == "Taken"
    ));

    assert!(
        library
            .create_book_plain("Taken", "text", &eng())
            .await
            .is_err()
    );
    assert_eq!(library.list_books().await.unwrap().len(), 1);
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_deleted_book_is_gone_for_good() {
    let dir = TempDir::new("flts_book");
    let root = dir.path.join("library");
    let library = Library::open(root.clone()).await.unwrap();
    let keep = library
        .create_book_plain("Keep", "text", &eng())
        .await
        .unwrap();
    let doomed = library
        .create_book_plain("Doomed", "text", &eng())
        .await
        .unwrap();
    let held = library.get_book(&doomed).await.unwrap();

    library.delete_book(&doomed).await.unwrap();

    assert_eq!(held.snapshot().status, BookStatus::Closed);
    assert!(held.flush().await.is_err());
    assert!(library.get_book(&doomed).await.is_err());
    let listed = library.list_books().await.unwrap();
    assert_eq!(listed.iter().map(|b| b.id).collect::<Vec<_>>(), [keep]);
    // Deleting twice, or deleting what never existed, is not an error.
    library.delete_book(&doomed).await.unwrap();
    library.delete_book(&Uuid::new_v4()).await.unwrap();
    // The title is free again.
    library
        .create_book_plain("Doomed", "again", &eng())
        .await
        .unwrap();
    library.close().await.unwrap();
    drop(library);

    let library = Library::open(root).await.unwrap();
    assert!(library.get_book(&doomed).await.is_err());
    assert_eq!(library.list_books().await.unwrap().len(), 2);
    library.close().await.unwrap();
}

#[tokio::test]
async fn deleting_a_book_that_is_not_loaded_removes_it() {
    let dir = TempDir::new("flts_book");
    let root = dir.path.join("library");
    let id = {
        let library = Library::open(root.clone()).await.unwrap();
        let id = library
            .create_book_plain("Cold", "text", &eng())
            .await
            .unwrap();
        library.close().await.unwrap();
        id
    };
    let library = Library::open(root).await.unwrap();
    library.delete_book(&id).await.unwrap();
    assert!(library.list_books().await.unwrap().is_empty());
    assert!(library.get_book(&id).await.is_err());
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_failed_commit_leaves_the_changes_staged_for_the_next_publish() {
    let dir = TempDir::new("flts_book");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let source = sample_book("Retry");
    let id = source.id;
    let mut book = stored_book(&store, source).await;

    book.get_or_create_translation(&rus())
        .add_paragraph_translation(0, &version(10, "первый"), "model-a");
    book.set_summary(1, summary(7, "second chapter"));
    fail_commits(&store, 1);
    assert!(book.persist().await.is_err());

    assert!(
        !book.has_unsaved_changes(),
        "staged changes are the store's to land"
    );
    {
        let (_dir, peer) = peer_view(&store);
        let committed = LibraryBook::load(peer, id).await.unwrap();
        assert!(
            committed.get_translation(&rus()).is_none(),
            "the failed commit published nothing"
        );
        assert!(!committed.summaries.entries[1].generated);
    }

    // Staged rows read back, so a reload keeps what the book holds.
    book.reload().await.unwrap();
    let held = book.get_translation(&rus()).unwrap();
    assert!(!held.is_changed());
    assert_eq!(
        held.paragraph_view(0).unwrap().to_import(),
        version(10, "первый")
    );
    assert_eq!(
        book.snapshot().summaries.entries[1],
        summary(7, "second chapter")
    );

    store.run(|db| db.publish()).await.unwrap();
    let (_dir, peer) = peer_view(&store);
    let stored = LibraryBook::load(peer, id).await.unwrap();
    let translation = stored.get_translation(&rus()).unwrap();
    assert_eq!(translation.translation.version_count(), 1);
    assert_eq!(
        translation.paragraph_view(0).unwrap().to_import(),
        version(10, "первый")
    );
    assert_eq!(stored.summaries.entries[1], summary(7, "second chapter"));
}

#[tokio::test]
async fn a_new_book_whose_commit_fails_is_stored_whole_by_the_next_one() {
    let dir = TempDir::new("flts_book");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let source = Arc::new(sample_book("First save"));
    let id = source.id;

    fail_commits(&store, 1);
    assert!(store.run(move |db| db.create_book(source)).await.is_err());
    assert!(store.read(|db| Ok(db.has_unpublished())).await.unwrap());
    store.run(|db| db.publish()).await.unwrap();

    let (_dir, peer) = peer_view(&store);
    assert_eq!(peer.run_blocking(|db| db.list_books()).unwrap().len(), 1);
    assert_eq!(
        peer.run_blocking(|db| db.load_book(id))
            .unwrap()
            .book
            .paragraphs_count(),
        3
    );
}

#[tokio::test]
async fn user_state_whose_commit_fails_is_published_by_the_next_commit() {
    let dir = TempDir::new("flts_book");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let source = sample_book("State");
    let id = source.id;
    let mut book = stored_book(&store, source).await;

    fail_commits(&store, 1);
    book.set_reading_state(reading(4));
    assert!(book.has_unsaved_changes());
    assert!(book.update_folder_path(vec!["Shelf".into()]).await.is_err());
    assert_eq!(book.snapshot().user_state.reading_state, Some(reading(4)));
    assert_eq!(book.snapshot().user_state.folder_path, ["Shelf"]);
    assert!(!book.has_unsaved_changes());
    let committed = |store: &LibraryStore| {
        let (_dir, peer) = peer_view(store);
        peer.run_blocking(|db| db.user_state(id)).unwrap()
    };
    assert_eq!(committed(&store), BookUserState::default());

    store.run(|db| db.publish()).await.unwrap();
    let stored = committed(&store);
    assert_eq!(stored.reading_state, Some(reading(4)));
    assert_eq!(stored.folder_path, ["Shelf"]);
}

#[tokio::test]
async fn a_reading_position_is_stored_by_the_next_persist() {
    let dir = TempDir::new("flts_book");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let source = sample_book("Position");
    let id = source.id;
    let mut book = stored_book(&store, source).await;

    book.set_reading_state(reading(1));
    book.set_reading_state(reading(2));
    assert!(book.only_position_unsaved());
    book.persist().await.unwrap();
    assert!(!book.has_unsaved_changes());
    let stored = LibraryBook::load(store, id).await.unwrap();
    assert_eq!(stored.user_state.reading_state, Some(reading(2)));
}

#[tokio::test]
async fn the_newest_version_always_carries_the_newest_stamp() {
    let dir = TempDir::new("flts_book");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let source = sample_book("Stamps");
    let id = source.id;
    let mut book = stored_book(&store, source).await;
    let translation = book.get_or_create_translation(&rus());
    translation.add_paragraph_translation(0, &version(10, "first"), "model-a");
    translation.add_paragraph_translation(0, &version(5, "older clock"), "model-a");
    translation.add_paragraph_translation(0, &version(11, "same stamp"), "model-a");
    translation.add_paragraph_translation(0, &version(50, "later"), "model-b");
    // Another paragraph has its own history.
    translation.add_paragraph_translation(1, &version(3, "beta"), "model-a");
    book.reload().await.unwrap();

    for book in [book, LibraryBook::load(store, id).await.unwrap()] {
        let translation = &book.get_translation(&rus()).unwrap().translation;
        let history: Vec<(u64, String, String)> = translation
            .versions(0)
            .iter()
            .map(|v| {
                (
                    v.timestamp,
                    v.sentence_view(0).full_translation.to_string(),
                    v.model.clone(),
                )
            })
            .collect();
        assert_eq!(
            history,
            [
                (10, "first".to_string(), "model-a".to_string()),
                (11, "older clock".to_string(), "model-a".to_string()),
                (12, "same stamp".to_string(), "model-a".to_string()),
                (50, "later".to_string(), "model-b".to_string()),
            ]
        );
        assert_eq!(translation.paragraph_view(1).unwrap().timestamp, 3);
    }
}

#[tokio::test]
async fn an_untranslated_translation_is_not_stored() {
    let dir = TempDir::new("flts_book");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let source = sample_book("Empty translation");
    let id = source.id;
    let mut book = stored_book(&store, source).await;
    book.get_or_create_translation(&rus());
    book.reload().await.unwrap();
    assert!(book.get_translation(&rus()).is_none());
    assert!(
        LibraryBook::load(store, id)
            .await
            .unwrap()
            .get_translation(&rus())
            .is_none()
    );
}

#[tokio::test]
async fn translations_to_several_languages_coexist() {
    let dir = TempDir::new("flts_book");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let source = sample_book("Polyglot");
    let id = source.id;
    let deu = Language::from_639_3("deu").unwrap();
    let mut book = stored_book(&store, source).await;
    book.get_or_create_translation(&rus())
        .add_paragraph_translation(0, &version(1, "альфа"), "model-a");
    book.get_or_create_translation(&deu)
        .add_paragraph_translation(0, &version(1, "Alpha"), "model-a");
    book.reload().await.unwrap();

    let stored = LibraryBook::load(store, id).await.unwrap();
    let text = |language: &Language| {
        stored
            .get_translation(language)
            .unwrap()
            .paragraph_view(0)
            .unwrap()
            .sentence_view(0)
            .full_translation
            .to_string()
    };
    assert_eq!(text(&rus()), "альфа");
    assert_eq!(text(&deu), "Alpha");
    assert_ne!(
        stored.get_translation(&rus()).unwrap().translation.id,
        stored.get_translation(&deu).unwrap().translation.id
    );
}

#[tokio::test]
async fn a_device_opening_on_synced_shards_sees_the_other_devices_books() {
    let dir = TempDir::new("flts_devices");
    let a = Library::open(dir.path.join("a/library")).await.unwrap();
    let book = a.create_book(sample_book("Arrived")).await.unwrap();
    let id = book.snapshot().book.id;
    translate(&book, 0, 10, "альфа").await;
    book.flush().await.unwrap();

    // The shards synced in while this device was not running.
    let b_root = dir.path.join("b/library");
    copy_shards(
        a.store().shared_dir(),
        &b_root.join(crate::store::SHARED_DIR),
    );
    let b = Library::open(b_root).await.unwrap();

    let listed = b.list_books().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Arrived");
    let loaded = b.get_book(&id).await.unwrap();
    assert_eq!(
        loaded
            .snapshot()
            .translation(&rus())
            .unwrap()
            .paragraph_view(0)
            .unwrap()
            .to_import(),
        version(10, "альфа")
    );
    a.close().await.unwrap();
    b.close().await.unwrap();
}

#[tokio::test]
async fn a_loaded_book_picks_up_another_devices_translation() {
    let devices = Devices::new().await;
    let on_a = devices.a.create_book(sample_book("Shared")).await.unwrap();
    let id = on_a.snapshot().book.id;
    let other = devices
        .a
        .create_book_plain("Bystander", "text", &eng())
        .await
        .unwrap();
    on_a.flush().await.unwrap();
    devices.sync_a_to_b();
    assert_eq!(
        devices.b.refresh_from_peers().await.unwrap(),
        PeerChanges {
            library: true,
            ..PeerChanges::default()
        },
        "nothing is loaded yet, so no loaded book changed"
    );

    let on_b = devices.b.get_book(&id).await.unwrap();
    let bystander = devices.b.get_book(&other).await.unwrap();
    assert!(on_b.snapshot().translation(&rus()).is_none());
    let mut watching = on_b.subscribe();
    watching.snapshot();

    translate(&on_a, 0, 10, "с другого устройства").await;
    on_a.apply_summary(&on_a.snapshot(), 1, summary(9, "remote summary"))
        .await
        .unwrap();
    on_a.update_reading_state(reading(2)).await.unwrap();
    on_a.flush().await.unwrap();
    devices.sync_a_to_b();

    let changes = devices.b.refresh_from_peers().await.unwrap();
    assert_eq!(
        changes,
        PeerChanges {
            library: true,
            books: vec![id],
            paragraphs: vec![(id, 0)],
            ..PeerChanges::default()
        }
    );
    tokio::time::timeout(Duration::from_secs(5), watching.changed())
        .await
        .expect("subscribers hear about the adopted change")
        .unwrap();
    let snapshot = on_b.snapshot();
    assert_eq!(
        snapshot
            .translation(&rus())
            .unwrap()
            .paragraph_view(0)
            .unwrap()
            .to_import(),
        version(10, "с другого устройства")
    );
    assert_eq!(snapshot.summaries.entries[1], summary(9, "remote summary"));
    assert_eq!(on_b.reading_state().await.unwrap(), Some(reading(2)));
    assert_eq!(snapshot.status, BookStatus::Active);
    assert!(bystander.snapshot().translation(&rus()).is_none());
    assert_eq!(
        devices.b.list_books().await.unwrap().len(),
        2,
        "one book per id, not per device"
    );

    assert_eq!(
        devices.b.refresh_from_peers().await.unwrap(),
        PeerChanges::default(),
        "a second refresh finds nothing new"
    );
    devices.a.close().await.unwrap();
    devices.b.close().await.unwrap();
}

#[tokio::test]
async fn both_devices_translations_of_one_paragraph_are_kept() {
    let devices = Devices::new().await;
    let on_a = devices.a.create_book(sample_book("Both")).await.unwrap();
    let id = on_a.snapshot().book.id;
    on_a.flush().await.unwrap();
    devices.sync_a_to_b();
    devices.b.refresh_from_peers().await.unwrap();
    let on_b = devices.b.get_book(&id).await.unwrap();

    translate(&on_a, 0, 10, "from a").await;
    translate(&on_a, 1, 15, "only a").await;
    on_a.flush().await.unwrap();
    translate(&on_b, 0, 20, "from b").await;
    // Unsaved here when the other device's write arrives.
    translate(&on_b, 2, 25, "only b").await;

    devices.sync_a_to_b();
    devices.sync_b_to_a();
    let changes = devices.b.refresh_from_peers().await.unwrap();
    assert_eq!(changes.paragraphs, [(id, 0), (id, 1)]);
    devices.sync_b_to_a();
    let changes = devices.a.refresh_from_peers().await.unwrap();
    assert_eq!(changes.paragraphs, [(id, 0), (id, 2)]);

    for book in [&on_a, &on_b] {
        let snapshot = book.snapshot();
        let translation = snapshot.translation(&rus()).unwrap();
        assert_eq!(translation.version_count(), 4);
        let history: Vec<(u64, String)> = translation
            .versions(0)
            .iter()
            .map(|v| (v.timestamp, v.sentence_view(0).full_translation.to_string()))
            .collect();
        assert_eq!(
            history,
            [(10, "from a".to_string()), (20, "from b".to_string())]
        );
        assert!(translation.paragraph_view(1).is_some());
        assert!(translation.paragraph_view(2).is_some());
    }
    devices.a.close().await.unwrap();
    devices.b.close().await.unwrap();
}

#[tokio::test]
async fn another_devices_page_turn_touches_only_that_book() {
    let devices = Devices::new().await;
    let on_a = devices.a.create_book(sample_book("Read")).await.unwrap();
    let id = on_a.snapshot().book.id;
    let other = devices
        .a
        .create_book_plain("Bystander", "text", &eng())
        .await
        .unwrap();
    devices.sync_a_to_b();
    devices.b.refresh_from_peers().await.unwrap();
    let on_b = devices.b.get_book(&id).await.unwrap();
    let bystander = devices.b.get_book(&other).await.unwrap();
    let before = bystander.snapshot();

    on_a.update_reading_state(reading(2)).await.unwrap();
    on_a.flush().await.unwrap();
    devices.sync_a_to_b();
    assert_eq!(
        devices.b.refresh_from_peers().await.unwrap(),
        PeerChanges {
            books: vec![id],
            ..PeerChanges::default()
        },
        "neither the shelf nor the cards changed"
    );
    assert_eq!(on_b.reading_state().await.unwrap(), Some(reading(2)));
    assert!(Arc::ptr_eq(&before, &bystander.snapshot()));
    devices.a.close().await.unwrap();
    devices.b.close().await.unwrap();
}

#[tokio::test]
async fn a_book_that_missed_a_reload_catches_up_on_the_next_refresh() {
    let devices = Devices::new().await;
    let on_a = devices.a.create_book(sample_book("Behind")).await.unwrap();
    let id = on_a.snapshot().book.id;
    devices.sync_a_to_b();
    devices.b.refresh_from_peers().await.unwrap();
    let on_b = devices.b.get_book(&id).await.unwrap();

    translate(&on_a, 0, 10, "с другого устройства").await;
    on_a.flush().await.unwrap();
    devices.sync_a_to_b();
    // Adopted, but the reload that should follow never happened.
    let adopted = devices.b.store().run(|db| db.refresh()).await.unwrap();
    assert!(adopted.books.contains_key(&id));
    assert!(on_b.snapshot().translation(&rus()).is_none());

    let changes = devices.b.refresh_from_peers().await.unwrap();
    assert_eq!(changes.books, [id], "which paragraphs is no longer known");
    assert!(on_b.snapshot().translation(&rus()).is_some());
    assert!(devices.b.refresh_from_peers().await.unwrap().is_empty());
    devices.a.close().await.unwrap();
    devices.b.close().await.unwrap();
}

#[tokio::test]
async fn a_remote_delete_closes_the_loaded_book() {
    let devices = Devices::new().await;
    let on_a = devices.a.create_book(sample_book("Doomed")).await.unwrap();
    let id = on_a.snapshot().book.id;
    on_a.flush().await.unwrap();
    let kept = devices
        .a
        .create_book_plain("Kept", "text", &eng())
        .await
        .unwrap();
    devices.sync_a_to_b();
    devices.b.refresh_from_peers().await.unwrap();
    let on_b = devices.b.get_book(&id).await.unwrap();
    let kept_on_b = devices.b.get_book(&kept).await.unwrap();
    // Work on a book that is gone has nowhere to go.
    translate(&on_b, 0, 10, "напрасно").await;

    drop(on_a);
    devices.a.delete_book(&id).await.unwrap();
    devices.sync_a_to_b();
    let changes = devices.b.refresh_from_peers().await.unwrap();

    assert!(changes.library);
    assert!(!changes.books.contains(&id));
    assert_eq!(changes.deleted, [id]);
    assert_eq!(on_b.snapshot().status, BookStatus::Closed);
    assert!(on_b.reading_state().await.is_err());
    assert!(on_b.flush().await.is_err());
    assert!(devices.b.get_book(&id).await.is_err());
    let listed = devices.b.list_books().await.unwrap();
    assert_eq!(listed.iter().map(|b| b.id).collect::<Vec<_>>(), [kept]);
    assert_eq!(kept_on_b.snapshot().status, BookStatus::Active);
    kept_on_b.flush().await.unwrap();

    // The delete must not bounce back as a resurrection.
    devices.sync_b_to_a();
    devices.a.refresh_from_peers().await.unwrap();
    assert_eq!(devices.a.list_books().await.unwrap().len(), 1);
    devices.a.close().await.unwrap();
    devices.b.close().await.unwrap();
}

#[tokio::test]
async fn a_book_whose_rows_arrived_only_in_part_fails_to_load_until_the_rest_does() {
    let devices = Devices::new().await;
    let id = devices
        .a
        .create_book(sample_book("Partial"))
        .await
        .unwrap()
        .snapshot()
        .book
        .id;

    let (from, to) = (
        devices.a.store().shared_dir().to_owned(),
        devices.b.store().shared_dir().to_owned(),
    );
    let mut entities: Vec<String> = std::fs::read_dir(&from)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entities.sort();
    let is_paragraphs = |name: &str| {
        let name = name.to_lowercase();
        name.contains("paragraph") && !name.contains("translation")
    };
    assert_eq!(
        entities.iter().filter(|name| is_paragraphs(name)).count(),
        1,
        "expected one paragraph shard directory among {entities:?}"
    );

    // The book row is here; its paragraphs are still in transit.
    copy_shards_where(&from, &to, &|name| !is_paragraphs(name));
    devices.b.refresh_from_peers().await.unwrap();
    assert!(
        devices.b.list_books().await.unwrap().is_empty(),
        "a book still arriving is not on the shelf"
    );
    let error = devices
        .b
        .get_book(&id)
        .await
        .err()
        .expect("a partial book must not load");
    assert!(
        error.to_string().contains("has not fully arrived yet"),
        "{error:#}"
    );
    assert!(
        devices.b.list_books().await.is_ok(),
        "the shelf still lists while a book is in transit"
    );

    copy_shards(&from, &to);
    devices.b.refresh_from_peers().await.unwrap();
    let loaded = devices.b.get_book(&id).await.unwrap();
    assert_eq!(loaded.snapshot().book.paragraphs_count(), 3);
    devices.a.close().await.unwrap();
    devices.b.close().await.unwrap();
}

#[tokio::test]
async fn our_own_shard_changes_are_not_peer_changes() {
    use crate::library::file_watcher::LibraryFileChange;
    let devices = Devices::new().await;
    let on_a = devices.a.create_book(sample_book("Echo")).await.unwrap();
    on_a.flush().await.unwrap();

    let own: Vec<_> = walk(devices.a.store().shared_dir());
    assert!(!own.is_empty());
    assert!(own.iter().all(|path| devices.a.store().is_own_file(path)));
    let changes = devices
        .a
        .handle_file_change_event(&LibraryFileChange::StoreChanged { paths: own.clone() })
        .await
        .unwrap();
    assert_eq!(changes, PeerChanges::default());

    devices.sync_a_to_b();
    let arrived: Vec<_> = walk(devices.b.store().shared_dir());
    assert!(
        arrived
            .iter()
            .any(|path| !devices.b.store().is_own_file(path))
    );
    let changes = devices
        .b
        .handle_file_change_event(&LibraryFileChange::StoreChanged { paths: arrived })
        .await
        .unwrap();
    assert!(changes.library);
    assert_eq!(devices.b.list_books().await.unwrap().len(), 1);
    devices.a.close().await.unwrap();
    devices.b.close().await.unwrap();
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[tokio::test]
async fn open_books_go_on_after_the_library_takes_a_new_identity() {
    let dir = TempDir::new("flts_book");
    let root = dir.path.join("library");
    let library = Library::open(root.clone()).await.unwrap();
    let book = library.create_book(sample_book("Carried")).await.unwrap();
    let id = book.snapshot().book.id;
    translate(&book, 0, 10, "до").await;
    book.flush().await.unwrap();
    library.note_shared_identity().await.unwrap();
    assert!(library.store_health().await.unwrap().shared_identity);

    library.take_new_identity().await.unwrap();
    assert!(!library.store_health().await.unwrap().shared_identity);
    translate(&book, 1, 20, "после").await;
    book.flush().await.unwrap();
    library.close().await.unwrap();
    drop((book, library));

    let library = Library::open(root).await.unwrap();
    let stored = library.get_book(&id).await.unwrap().snapshot();
    assert_eq!(stored.translation(&rus()).unwrap().version_count(), 2);
    assert!(!library.store_health().await.unwrap().shared_identity);
    library.close().await.unwrap();
}
