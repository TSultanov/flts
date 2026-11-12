use std::time::Duration;

use isolang::Language;
use uuid::Uuid;

use super::*;
use crate::{
    book::{chapter_summaries::ChapterSummary, translation_import},
    library::{LibraryError, PeerChanges, REPUBLISH_EVERY},
    store::{StoredVersion, translation_uuid},
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

async fn stored_book(store: &LibraryStore, source: Book) -> Uuid {
    store
        .run(move |db| db.create_book(Arc::new(source)))
        .await
        .unwrap()
}

/// Stages a translation into `target`, as a book of the library would.
async fn stage_version(
    store: &LibraryStore,
    id: Uuid,
    paragraph: usize,
    target: &str,
    translation: translation_import::ParagraphTranslation,
    model: &str,
) {
    let version = NewVersion {
        source_language: "eng".into(),
        target_language: target.into(),
        paragraph,
        translation,
        model: model.into(),
    };
    store
        .run(move |db| db.add_version(id, version))
        .await
        .unwrap();
}

/// A library holding one book of one chapter and one paragraph.
async fn fixture() -> (TempDir, Library, BookHandle) {
    let dir = TempDir::new("flts_book");
    let library = Library::open(dir.path.join("library")).await.unwrap();
    let id = library
        .create_book_plain("First", "hello", &eng())
        .await
        .unwrap();
    let book = library.get_book(&id).await.unwrap();
    (dir, library, book)
}

fn position(page_offset: usize) -> BookReadingState {
    BookReadingState {
        chapter_id: 0,
        paragraph_id: 0,
        page_offset,
    }
}

/// What another device would read from the shards: only what was committed.
fn committed_user_state(store: &LibraryStore, id: Uuid) -> BookUserState {
    let (_dir, peer) = peer_view(store);
    peer.run_blocking(|db| db.user_state(id)).unwrap()
}

fn committed_reading_state(store: &LibraryStore, id: Uuid) -> Option<BookReadingState> {
    committed_user_state(store, id).reading_state
}

fn committed_versions(store: &LibraryStore, id: Uuid) -> usize {
    let (_dir, peer) = peer_view(store);
    stored_version_count(&peer, id)
}

fn committed_summaries(store: &LibraryStore, id: Uuid) -> Vec<ChapterSummary> {
    let (_dir, peer) = peer_view(store);
    peer.run_blocking(|db| db.summaries(id)).unwrap().entries
}

async fn summarize(book: &BookHandle, chapter: usize, summary: ChapterSummary) {
    let written_from = book.summaries().await.unwrap();
    book.apply_summary(&written_from, chapter, summary)
        .await
        .unwrap();
}

/// Waits in real time: paused time may run ahead of the blocking pool.
async fn eventually(what: &str, mut holds: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !holds() {
        assert!(std::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Makes the next `count` commits fail; zero heals the store.
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

async fn current(book: &BookHandle, target: Language, paragraph: usize) -> Option<StoredVersion> {
    book.read(move |b| b.translation(&target, paragraph))
        .await
        .unwrap()
}

async fn history(book: &BookHandle, paragraph: usize) -> Vec<StoredVersion> {
    book.read(move |b| b.versions(&rus(), paragraph))
        .await
        .unwrap()
}

async fn version_count(book: &BookHandle) -> usize {
    book.read(|b| b.version_count(&rus())).await.unwrap()
}

fn stored_current(
    store: &LibraryStore,
    id: Uuid,
    target: &'static str,
    paragraph: usize,
) -> Option<StoredVersion> {
    store
        .run_blocking(|db| db.paragraph_translation(id, target, paragraph))
        .unwrap()
}

fn stored_version_count(store: &LibraryStore, id: Uuid) -> usize {
    store
        .run_blocking(|db| db.version_count(id, "rus"))
        .unwrap()
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
        summarize(&book, 0, summary(5, "first chapter")).await;
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
    assert_eq!(book.info().title, "Round trip");
    assert_eq!(book.info().language, "eng");
    assert_eq!(book.info().chapter_count(), 2);
    let outline = book.read(|b| b.outline()).await.unwrap();
    assert_eq!(outline.chapters.len(), 2);
    assert_eq!(outline.chapters[0].title.as_deref(), Some("One"));
    assert_eq!(outline.chapters[1].title, None);
    assert_eq!(outline.chapters[0].paragraphs, [0, 1]);
    assert_eq!(outline.chapters[1].paragraphs, [2]);
    for paragraph in 0..3 {
        let stored = book.read(move |b| b.paragraph(paragraph)).await.unwrap();
        let original = source.paragraph_view(paragraph);
        assert_eq!(stored.text, original.original_text);
        assert_eq!(stored.html.as_deref(), original.original_html.as_deref());
    }

    assert_eq!(listed_translation.id, translation_uuid(id, "eng", "rus"));
    assert_eq!(version_count(&book).await, 3);
    let versions = history(&book, 0).await;
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[1].translation, version(20, "новое"));
    assert_eq!(versions[1].model, "model-a");
    assert_eq!(current(&book, rus(), 0).await.as_ref(), Some(&versions[1]));
    assert_eq!(versions[0].translation, version(10, "старое"));
    assert_eq!(current(&book, rus(), 1).await, None);
    assert_eq!(
        current(&book, rus(), 2).await.unwrap().translation,
        version(30, "гамма")
    );

    let summaries = book.summaries().await.unwrap();
    assert_eq!(summaries.entries[0], summary(5, "first chapter"));
    assert!(!summaries.entries[1].generated);
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
    let id = book.id();
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

    assert!(held.read(|b| b.paragraph(0)).await.is_err());
    assert!(held.reading_state().await.is_err());
    assert!(
        held.apply_translation(0, rus(), version(1, "поздно"), "model-a".into())
            .await
            .is_err()
    );
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
async fn deleting_a_book_nobody_holds_removes_it() {
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
    let id = stored_book(&store, sample_book("Retry")).await;
    let summaries =
        |store: &LibraryStore| store.run_blocking(|db| db.summaries(id)).unwrap().entries;

    stage_version(&store, id, 0, "rus", version(10, "первый"), "model-a").await;
    assert_eq!(
        stored_current(&store, id, "rus", 0).unwrap().translation,
        version(10, "первый"),
        "readable before any commit"
    );
    let written_from = summaries(&store)[..1].to_vec();
    store
        .run(move |db| db.set_summary(id, 1, &written_from, summary(7, "second chapter")))
        .await
        .unwrap();
    fail_commits(&store, 1);
    assert!(store.run(|db| db.publish()).await.is_err());

    assert!(
        store.read(|db| Ok(db.has_unpublished())).await.unwrap(),
        "staged changes are the store's to land"
    );
    {
        let (_dir, peer) = peer_view(&store);
        assert_eq!(
            stored_version_count(&peer, id),
            0,
            "the failed commit published nothing"
        );
        assert!(!summaries(&peer)[1].generated);
    }

    // Staged rows read back.
    assert_eq!(
        stored_current(&store, id, "rus", 0).unwrap().translation,
        version(10, "первый")
    );
    assert_eq!(summaries(&store)[1], summary(7, "second chapter"));

    store.run(|db| db.publish()).await.unwrap();
    assert!(!store.read(|db| Ok(db.has_unpublished())).await.unwrap());
    let (_dir, peer) = peer_view(&store);
    assert_eq!(stored_version_count(&peer, id), 1);
    assert_eq!(
        stored_current(&peer, id, "rus", 0).unwrap().translation,
        version(10, "первый")
    );
    assert_eq!(summaries(&peer)[1], summary(7, "second chapter"));
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
        peer.run_blocking(|db| db.book_info(id))
            .unwrap()
            .paragraphs_count(),
        3
    );
}

#[tokio::test]
async fn user_state_whose_commit_fails_is_published_by_the_next_commit() {
    let dir = TempDir::new("flts_book");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let id = stored_book(&store, sample_book("State")).await;

    fail_commits(&store, 1);
    store
        .run(move |db| db.set_user_state(id, Some(reading(4)), None))
        .await
        .unwrap();
    let committing = store.run(move |db| {
        db.set_user_state(id, None, Some(vec!["Shelf".into()]))?;
        db.publish()
    });
    assert!(committing.await.is_err());
    let held = store.run_blocking(|db| db.user_state(id)).unwrap();
    assert_eq!(held.reading_state, Some(reading(4)));
    assert_eq!(held.folder_path, ["Shelf"]);
    assert_eq!(committed_user_state(&store, id), BookUserState::default());

    store.run(|db| db.publish()).await.unwrap();
    let stored = committed_user_state(&store, id);
    assert_eq!(stored.reading_state, Some(reading(4)));
    assert_eq!(stored.folder_path, ["Shelf"]);
}

#[tokio::test]
async fn a_folder_whose_commit_fails_is_reported_and_stays_readable() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    fail_commits(library.store(), usize::MAX);
    let error = book
        .update_folder_path(vec!["Shelf".into()])
        .await
        .unwrap_err();
    assert!(error.to_string().contains("injected"), "{error:#}");
    assert_eq!(book.folder_path().await.unwrap(), ["Shelf"]);
    assert!(
        committed_user_state(library.store(), id)
            .folder_path
            .is_empty()
    );

    fail_commits(library.store(), 0);
    library.close().await.unwrap();
    assert_eq!(
        committed_user_state(library.store(), id).folder_path,
        ["Shelf"]
    );
}

#[tokio::test]
async fn a_reading_position_is_stored_by_the_next_flush() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();

    book.update_reading_state(position(1)).await.unwrap();
    book.update_reading_state(position(2)).await.unwrap();
    assert_eq!(book.reading_state().await.unwrap(), Some(position(2)));
    assert_eq!(committed_reading_state(library.store(), id), None);
    book.flush().await.unwrap();
    assert_eq!(
        committed_reading_state(library.store(), id),
        Some(position(2))
    );
    assert_eq!(book.reading_state().await.unwrap(), Some(position(2)));
    library.close().await.unwrap();
}

#[tokio::test]
async fn the_newest_version_always_carries_the_newest_stamp() {
    let dir = TempDir::new("flts_book");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let id = stored_book(&store, sample_book("Stamps")).await;
    for (paragraph, stamp, text, model) in [
        (0, 10, "first", "model-a"),
        (0, 5, "older clock", "model-a"),
        (0, 11, "same stamp", "model-a"),
        (0, 50, "later", "model-b"),
        // Another paragraph has its own history.
        (1, 3, "beta", "model-a"),
    ] {
        stage_version(&store, id, paragraph, "rus", version(stamp, text), model).await;
    }
    store.run(|db| db.publish()).await.unwrap();

    // Here, and as another device reads the shards.
    let (_dir, peer) = peer_view(&store);
    for store in [store, peer] {
        let history: Vec<(u64, String, String)> = store
            .run_blocking(|db| db.versions(id, "rus", 0))
            .unwrap()
            .into_iter()
            .map(|v| {
                (
                    v.translation.timestamp,
                    v.translation.sentences[0].full_translation.clone(),
                    v.model,
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
        assert_eq!(
            stored_current(&store, id, "rus", 0)
                .unwrap()
                .translation
                .timestamp,
            50
        );
        assert_eq!(
            stored_current(&store, id, "rus", 1)
                .unwrap()
                .translation
                .timestamp,
            3
        );
    }
}

#[tokio::test]
async fn an_untranslated_translation_is_not_stored() {
    let dir = TempDir::new("flts_book");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let id = stored_book(&store, sample_book("Empty translation")).await;
    store.run(|db| db.publish()).await.unwrap();
    assert!(!store.read(|db| Ok(db.has_unpublished())).await.unwrap());
    let (_dir, peer) = peer_view(&store);
    for store in [store, peer] {
        let listed = store.run_blocking(|db| db.list_books()).unwrap();
        assert!(listed[0].translations.is_empty());
        assert_eq!(stored_version_count(&store, id), 0);
        assert_eq!(stored_current(&store, id, "rus", 0), None);
    }
}

#[tokio::test]
async fn translations_to_several_languages_coexist() {
    let dir = TempDir::new("flts_book");
    let store = LibraryStore::open(&dir.path.join("library")).unwrap();
    let id = stored_book(&store, sample_book("Polyglot")).await;
    stage_version(&store, id, 0, "rus", version(1, "альфа"), "model-a").await;
    stage_version(&store, id, 0, "deu", version(1, "Alpha"), "model-a").await;
    store.run(|db| db.publish()).await.unwrap();

    let (_dir, peer) = peer_view(&store);
    let text = |language: &'static str| {
        stored_current(&peer, id, language, 0)
            .unwrap()
            .translation
            .sentences[0]
            .full_translation
            .clone()
    };
    assert_eq!(text("rus"), "альфа");
    assert_eq!(text("deu"), "Alpha");
    let listed = peer.run_blocking(|db| db.list_books()).unwrap();
    let mut ids: Vec<Uuid> = listed[0].translations.iter().map(|t| t.id).collect();
    ids.sort();
    let mut expected = [
        translation_uuid(id, "eng", "rus"),
        translation_uuid(id, "eng", "deu"),
    ];
    expected.sort();
    assert_ne!(expected[0], expected[1]);
    assert_eq!(ids, expected);
}

#[tokio::test]
async fn a_device_opening_on_synced_shards_sees_the_other_devices_books() {
    let dir = TempDir::new("flts_devices");
    let a = Library::open(dir.path.join("a/library")).await.unwrap();
    let book = a.create_book(sample_book("Arrived")).await.unwrap();
    let id = book.id();
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
        current(&loaded, rus(), 0).await.unwrap().translation,
        version(10, "альфа")
    );
    a.close().await.unwrap();
    b.close().await.unwrap();
}

#[tokio::test]
async fn a_held_book_picks_up_another_devices_translation() {
    let devices = Devices::new().await;
    let on_a = devices.a.create_book(sample_book("Shared")).await.unwrap();
    let id = on_a.id();
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
        "a book arriving whole changes the shelf alone"
    );

    let on_b = devices.b.get_book(&id).await.unwrap();
    let bystander = devices.b.get_book(&other).await.unwrap();
    assert_eq!(version_count(&on_b).await, 0);
    let mut watching = on_b.summaries_changed();
    watching.borrow_and_update();

    translate(&on_a, 0, 10, "с другого устройства").await;
    summarize(&on_a, 1, summary(9, "remote summary")).await;
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
    assert_eq!(
        current(&on_b, rus(), 0).await.unwrap().translation,
        version(10, "с другого устройства")
    );
    assert_eq!(
        on_b.summaries().await.unwrap().entries[1],
        summary(9, "remote summary")
    );
    assert_eq!(on_b.reading_state().await.unwrap(), Some(reading(2)));
    assert_eq!(version_count(&bystander).await, 0);
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
    let id = on_a.id();
    on_a.flush().await.unwrap();
    devices.sync_a_to_b();
    devices.b.refresh_from_peers().await.unwrap();
    let on_b = devices.b.get_book(&id).await.unwrap();

    translate(&on_a, 0, 10, "from a").await;
    translate(&on_a, 1, 15, "only a").await;
    on_a.flush().await.unwrap();
    translate(&on_b, 0, 20, "from b").await;
    // Not flushed here when the other device's write arrives.
    translate(&on_b, 2, 25, "only b").await;

    devices.sync_a_to_b();
    devices.sync_b_to_a();
    let changes = devices.b.refresh_from_peers().await.unwrap();
    assert_eq!(changes.paragraphs, [(id, 0), (id, 1)]);
    devices.sync_b_to_a();
    let changes = devices.a.refresh_from_peers().await.unwrap();
    assert_eq!(changes.paragraphs, [(id, 0), (id, 2)]);

    for book in [&on_a, &on_b] {
        assert_eq!(version_count(book).await, 4);
        let history: Vec<(u64, String)> = history(book, 0)
            .await
            .into_iter()
            .map(|v| {
                (
                    v.translation.timestamp,
                    v.translation.sentences[0].full_translation.clone(),
                )
            })
            .collect();
        assert_eq!(
            history,
            [(10, "from a".to_string()), (20, "from b".to_string())]
        );
        assert!(current(book, rus(), 1).await.is_some());
        assert!(current(book, rus(), 2).await.is_some());
    }
    devices.a.close().await.unwrap();
    devices.b.close().await.unwrap();
}

#[tokio::test]
async fn another_devices_page_turn_touches_only_that_book() {
    let devices = Devices::new().await;
    let on_a = devices.a.create_book(sample_book("Read")).await.unwrap();
    let id = on_a.id();
    let other = devices
        .a
        .create_book_plain("Bystander", "text", &eng())
        .await
        .unwrap();
    devices.sync_a_to_b();
    devices.b.refresh_from_peers().await.unwrap();
    let on_b = devices.b.get_book(&id).await.unwrap();
    let bystander = devices.b.get_book(&other).await.unwrap();

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
    assert_eq!(bystander.reading_state().await.unwrap(), None);
    devices.a.close().await.unwrap();
    devices.b.close().await.unwrap();
}

#[tokio::test]
async fn a_held_book_reads_what_the_store_adopted_without_the_library() {
    let devices = Devices::new().await;
    let on_a = devices.a.create_book(sample_book("Behind")).await.unwrap();
    let id = on_a.id();
    devices.sync_a_to_b();
    devices.b.refresh_from_peers().await.unwrap();
    let on_b = devices.b.get_book(&id).await.unwrap();

    translate(&on_a, 0, 10, "с другого устройства").await;
    on_a.flush().await.unwrap();
    devices.sync_a_to_b();
    let adopted = devices.b.store().run(|db| db.refresh()).await.unwrap();
    assert!(adopted.books.contains_key(&id));

    assert_eq!(
        current(&on_b, rus(), 0).await.unwrap().translation,
        version(10, "с другого устройства")
    );
    assert!(devices.b.refresh_from_peers().await.unwrap().is_empty());
    devices.a.close().await.unwrap();
    devices.b.close().await.unwrap();
}

#[tokio::test]
async fn a_remote_delete_ends_the_held_book() {
    let devices = Devices::new().await;
    let on_a = devices.a.create_book(sample_book("Doomed")).await.unwrap();
    let id = on_a.id();
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
    assert!(on_b.reading_state().await.is_err());
    assert!(on_b.read(|b| b.paragraph(0)).await.is_err());
    assert!(
        on_b.apply_translation(1, rus(), version(20, "некуда"), "model-a".into())
            .await
            .is_err()
    );
    assert!(devices.b.get_book(&id).await.is_err());
    let listed = devices.b.list_books().await.unwrap();
    assert_eq!(listed.iter().map(|b| b.id).collect::<Vec<_>>(), [kept]);
    assert_eq!(kept_on_b.info().title, "Kept");
    assert_eq!(kept_on_b.reading_state().await.unwrap(), None);
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
        .id();

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
    assert_eq!(loaded.info().paragraphs_count(), 3);
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
    let id = book.id();
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
    let stored = library.get_book(&id).await.unwrap();
    assert_eq!(version_count(&stored).await, 2);
    assert!(!library.store_health().await.unwrap().shared_identity);
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_translation_is_readable_before_any_flush() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    // Nothing can reach the disk, so what reads back was never committed.
    fail_commits(library.store(), usize::MAX);
    translate(&book, 0, 10, "сразу").await;

    assert_eq!(
        current(&book, rus(), 0).await.unwrap().translation,
        version(10, "сразу")
    );
    assert_eq!(version_count(&book).await, 1);
    let again = library.get_book(&id).await.unwrap();
    assert_eq!(
        current(&again, rus(), 0).await.unwrap().translation,
        version(10, "сразу")
    );
    assert_eq!(
        library.list_books().await.unwrap()[0].translations_metadata[0].translated_paragraphs_count,
        1
    );
    assert_eq!(committed_versions(library.store(), id), 0);

    fail_commits(library.store(), 0);
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_translation_is_committed_without_a_flush() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    translate(&book, 0, 10, "само").await;
    let store = library.store().clone();
    tokio::time::timeout(Duration::from_secs(10), async {
        while committed_versions(&store, id) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the commit follows in the background");
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_reading_position_whose_commit_fails_lands_on_close() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    let store = library.store().clone();
    fail_commits(&store, usize::MAX);
    book.update_reading_state(position(3)).await.unwrap();
    assert_eq!(book.reading_state().await.unwrap(), Some(position(3)));
    assert!(book.flush().await.is_err());
    assert_eq!(book.reading_state().await.unwrap(), Some(position(3)));
    assert_eq!(committed_reading_state(&store, id), None);

    fail_commits(&store, 0);
    drop(book);
    library.close().await.unwrap();
    assert_eq!(committed_reading_state(&store, id), Some(position(3)));
}

#[tokio::test(start_paused = true)]
async fn page_turns_are_coalesced_into_one_commit() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    let store = library.store().clone();
    let superseded = |store: &LibraryStore| store.run_blocking(|db| db.prune(&[])).unwrap();
    assert_eq!(superseded(&store), 0);

    for page in 1..=3 {
        book.update_reading_state(position(page)).await.unwrap();
    }
    assert_eq!(book.reading_state().await.unwrap(), Some(position(3)));
    assert!(
        !store.run_blocking(|db| Ok(db.has_unpublished())).unwrap(),
        "a page turn alone stages nothing"
    );
    assert_eq!(committed_reading_state(&store, id), None);

    tokio::time::sleep(POSITION_DELAY + Duration::from_millis(100)).await;
    eventually("the last position is committed", || {
        committed_reading_state(&store, id) == Some(position(3))
    })
    .await;
    assert_eq!(
        superseded(&store),
        0,
        "one row for the run: no earlier one to drop"
    );

    // Each run of page turns after that replaces the row once.
    for page in 4..=6 {
        book.update_reading_state(position(page)).await.unwrap();
    }
    tokio::time::sleep(POSITION_DELAY + Duration::from_millis(100)).await;
    eventually("the second run is committed", || {
        committed_reading_state(&store, id) == Some(position(6))
    })
    .await;
    assert_eq!(superseded(&store), 1);
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_reading_position_set_just_before_close_survives_a_reopen() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    let root = library.root().to_path_buf();
    book.update_reading_state(position(4)).await.unwrap();
    drop(book);
    library.close().await.unwrap();
    drop(library);

    let library = Library::open(root).await.unwrap();
    let book = library.get_book(&id).await.unwrap();
    assert_eq!(book.reading_state().await.unwrap(), Some(position(4)));
    library.close().await.unwrap();
}

#[tokio::test]
async fn library_flush_lands_a_reading_position_still_waiting() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    book.update_reading_state(position(5)).await.unwrap();
    assert_eq!(committed_reading_state(library.store(), id), None);

    library.flush().await.unwrap();
    assert_eq!(
        committed_reading_state(library.store(), id),
        Some(position(5))
    );
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_summary_is_readable_at_once_and_its_chapter_must_exist() {
    let (_dir, library, book) = fixture().await;
    let before = book.summaries().await.unwrap();
    let mut heard = book.summaries_changed();
    heard.borrow_and_update();
    book.apply_summary(&before, 0, summary(1, "summary"))
        .await
        .unwrap();
    let after = book.summaries().await.unwrap();
    assert!(!before.entries[0].generated);
    assert_eq!(after.entries[0], summary(1, "summary"));
    assert!(heard.has_changed().unwrap());

    book.flush().await.unwrap();
    for chapter in [1, usize::MAX] {
        let error = book
            .apply_summary(&after, chapter, after.entries[0].clone())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("out of range"), "{error:#}");
    }
    assert_eq!(book.summaries().await.unwrap().entries, after.entries);
    library.close().await.unwrap();
}

#[tokio::test]
async fn summaries_survive_a_reopen_and_the_newest_wins() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    let root = library.root().to_path_buf();
    summarize(&book, 0, summary(1, "first")).await;
    let newer = summary(2, "second");
    summarize(&book, 0, newer.clone()).await;
    book.flush().await.unwrap();
    assert_eq!(book.summaries().await.unwrap().entries[0], newer);
    drop(book);
    library.close().await.unwrap();
    drop(library);

    let library = Library::open(root).await.unwrap();
    let book = library.get_book(&id).await.unwrap();
    assert_eq!(book.summaries().await.unwrap().entries, vec![newer]);
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_summary_stamped_no_newer_than_the_one_it_replaces_still_wins() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    let root = library.root().to_path_buf();
    summarize(&book, 0, summary(5, "first")).await;
    book.flush().await.unwrap();
    summarize(&book, 0, summary(5, "second")).await;
    assert_eq!(
        book.summaries().await.unwrap().entries[0],
        summary(6, "second")
    );
    summarize(&book, 0, summary(2, "third")).await;
    let third = summary(7, "third");
    assert_eq!(book.summaries().await.unwrap().entries[0], third);
    book.flush().await.unwrap();
    drop(book);
    library.close().await.unwrap();
    drop(library);

    let library = Library::open(root).await.unwrap();
    let book = library.get_book(&id).await.unwrap();
    assert_eq!(book.summaries().await.unwrap().entries, vec![third]);
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_summary_written_from_changed_context_is_refused() {
    let (_dir, library, _first) = fixture().await;
    let mut source = Book::create(Uuid::new_v4(), "Chapters", &eng());
    for text in ["one", "two"] {
        let chapter = source.push_chapter(None);
        source.push_paragraph(chapter, text, None);
    }
    let book = library.create_book(source).await.unwrap();
    let stale = book.summaries().await.unwrap();
    book.apply_summary(&stale, 0, summary(1, "first"))
        .await
        .unwrap();

    let error = book
        .apply_summary(&stale, 1, summary(1, "second"))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("summary context changed"),
        "{error:#}"
    );
    assert!(!book.summaries().await.unwrap().entries[1].generated);
    summarize(&book, 1, summary(1, "second")).await;
    assert_eq!(
        book.summaries().await.unwrap().entries,
        [summary(1, "first"), summary(1, "second")]
    );
    library.close().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn dropping_the_library_lands_accepted_work_and_releases_the_lease() {
    let (_dir, library, book) = fixture().await;
    let root = library.root().to_path_buf();
    let id = book.id();
    book.update_reading_state(position(2)).await.unwrap();
    assert!(
        Library::open(root.clone()).await.is_err(),
        "the lease is held while the library lives"
    );
    drop(book);
    drop(library);
    let mut reopened = None;
    for _ in 0..120 {
        if let Ok(library) = Library::open(root.clone()).await {
            reopened = Some(library);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let library = reopened.expect("lease released once the last library was dropped");
    let book = library.get_book(&id).await.unwrap();
    assert_eq!(book.reading_state().await.unwrap(), Some(position(2)));
    drop(book);
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_failed_close_reports_keeps_the_lease_and_can_be_retried() {
    let (_dir, library, book) = fixture().await;
    let root = library.root().to_path_buf();
    let id = book.id();
    fail_commits(library.store(), usize::MAX);
    translate(&book, 0, 1, "раз").await;

    let error = library.close().await.unwrap_err();
    assert!(error.to_string().contains("flush failed"), "{error:#}");
    assert!(
        Library::open(root.clone()).await.is_err(),
        "the lease stays with the session that still holds unsaved work"
    );
    assert_eq!(version_count(&book).await, 1);
    assert_eq!(committed_versions(library.store(), id), 0);
    assert!(library.close().await.is_err());

    fail_commits(library.store(), 0);
    library.close().await.unwrap();
    drop(book);
    drop(library);
    let library = Library::open(root).await.unwrap();
    let book = library.get_book(&id).await.unwrap();
    assert_eq!(version_count(&book).await, 1);
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_library_whose_close_failed_still_serves_its_books() {
    let (_dir, library, first) = fixture().await;
    let id = first.id();
    fail_commits(library.store(), usize::MAX);
    translate(&first, 0, 1, "раз").await;
    assert!(library.close().await.is_err());

    let again = library.get_book(&id).await.unwrap();
    for book in [&first, &again] {
        assert_eq!(version_count(book).await, 1, "staged rows read back");
    }
    first.update_reading_state(position(1)).await.unwrap();
    assert!(again.flush().await.is_err());
    assert_eq!(again.reading_state().await.unwrap(), Some(position(1)));
    fail_commits(library.store(), 0);
    again.flush().await.unwrap();
    assert_eq!(committed_versions(library.store(), id), 1);
    assert_eq!(
        committed_reading_state(library.store(), id),
        Some(position(1))
    );
    library.close().await.unwrap();
}

#[tokio::test]
async fn flushing_with_nothing_waiting_commits_nothing() {
    let (_dir, library, book) = fixture().await;
    translate(&book, 0, 1, "раз").await;
    book.flush().await.unwrap();
    // Any commit now would fail; a flush with nothing to land must not run one.
    fail_commits(library.store(), usize::MAX);
    book.flush().await.unwrap();
    library.flush().await.unwrap();
    fail_commits(library.store(), 0);
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_flush_reports_the_failed_commit_and_a_later_flush_lands_it() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    fail_commits(library.store(), usize::MAX);
    translate(&book, 0, 1, "раз").await;
    let error = book.flush().await.unwrap_err();
    assert!(error.to_string().contains("injected"), "{error:#}");
    assert!(book.flush().await.is_err());
    assert_eq!(version_count(&book).await, 1);
    assert_eq!(committed_versions(library.store(), id), 0);

    fail_commits(library.store(), 0);
    book.flush().await.unwrap();
    assert_eq!(committed_versions(library.store(), id), 1);
    library.close().await.unwrap();
}

#[tokio::test]
async fn library_flush_lands_what_a_failed_commit_left_staged() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    fail_commits(library.store(), usize::MAX);
    translate(&book, 0, 1, "раз").await;
    summarize(&book, 0, summary(1, "kept")).await;
    assert!(book.flush().await.is_err());
    assert!(library.flush().await.is_err());
    assert_eq!(committed_versions(library.store(), id), 0);
    assert!(!committed_summaries(library.store(), id)[0].generated);

    fail_commits(library.store(), 0);
    library.flush().await.unwrap();
    assert_eq!(committed_versions(library.store(), id), 1);
    assert_eq!(
        committed_summaries(library.store(), id)[0],
        summary(1, "kept")
    );
    library.close().await.unwrap();
}

#[tokio::test]
async fn any_later_commit_carries_what_a_failed_one_left_staged() {
    let (_dir, library, first) = fixture().await;
    let id = first.id();
    let second = library
        .create_book_plain("Second", "other", &eng())
        .await
        .unwrap();
    let second = library.get_book(&second).await.unwrap();
    fail_commits(library.store(), usize::MAX);
    translate(&first, 0, 1, "раз").await;
    assert!(first.flush().await.is_err());
    drop(first);
    assert_eq!(committed_versions(library.store(), id), 0);

    fail_commits(library.store(), 0);
    second
        .update_folder_path(vec!["shelf".into()])
        .await
        .unwrap();
    assert_eq!(committed_versions(library.store(), id), 1);
    let reloaded = library.get_book(&id).await.unwrap();
    assert_eq!(version_count(&reloaded).await, 1);
    library.close().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_failed_commit_is_republished_in_the_background() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    let store = library.store().clone();
    let folder = |store: &LibraryStore| committed_user_state(store, id).folder_path;
    fail_commits(&store, usize::MAX);
    let error = book
        .update_folder_path(vec!["shelf".into()])
        .await
        .unwrap_err();
    assert!(error.to_string().contains("injected"), "{error:#}");
    assert_eq!(book.folder_path().await.unwrap(), ["shelf"]);
    tokio::time::sleep(REPUBLISH_EVERY * 2).await;
    assert!(folder(&store).is_empty());

    fail_commits(&store, 0);
    tokio::time::sleep(REPUBLISH_EVERY * 2).await;
    eventually("the staged folder is committed with no one asking", || {
        folder(&store) == ["shelf"]
    })
    .await;
    assert_eq!(book.folder_path().await.unwrap(), ["shelf"]);
    library.close().await.unwrap();
}

#[tokio::test]
async fn closing_a_library_with_nothing_waiting_commits_nothing() {
    let (_dir, library, book) = fixture().await;
    book.flush().await.unwrap();
    // Any commit would fail and fail the close with it.
    fail_commits(library.store(), usize::MAX);
    drop(book);
    library.close().await.unwrap();
}

#[tokio::test]
async fn user_state_is_readable_at_once() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    book.update_reading_state(position(2)).await.unwrap();
    book.update_folder_path(vec!["shelf".into()]).await.unwrap();
    let other = library.get_book(&id).await.unwrap();
    for book in [&book, &other] {
        assert_eq!(book.reading_state().await.unwrap(), Some(position(2)));
        assert_eq!(book.folder_path().await.unwrap(), ["shelf"]);
    }
    library.close().await.unwrap();
}

#[tokio::test]
async fn user_state_survives_a_reopen() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    let root = library.root().to_path_buf();
    book.update_reading_state(position(2)).await.unwrap();
    book.update_folder_path(vec!["shelf".into()]).await.unwrap();
    drop(book);
    library.close().await.unwrap();
    drop(library);

    let library = Library::open(root).await.unwrap();
    let book = library.get_book(&id).await.unwrap();
    assert_eq!(book.reading_state().await.unwrap(), Some(position(2)));
    assert_eq!(book.folder_path().await.unwrap(), ["shelf"]);
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_book_of_a_closed_library_rejects_reads_and_writes() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    let written_from = book.summaries().await.unwrap();
    library.close().await.unwrap();
    assert!(book.reading_state().await.is_err());
    assert!(book.folder_path().await.is_err());
    assert!(book.read(|b| b.paragraph(0)).await.is_err());
    assert!(book.summaries().await.is_err());
    assert!(
        book.apply_translation(0, rus(), version(1, "поздно"), "model-a".into())
            .await
            .is_err()
    );
    assert!(
        book.apply_summary(&written_from, 0, summary(1, "late"))
            .await
            .is_err()
    );
    assert!(book.update_folder_path(vec!["late".into()]).await.is_err());
    assert!(book.flush().await.is_err());
    assert!(library.get_book(&id).await.is_err());
}

#[tokio::test]
async fn a_translation_of_a_missing_paragraph_is_rejected() {
    let (_dir, library, book) = fixture().await;
    let error = book
        .apply_translation(1, rus(), version(1, "мимо"), "model-a".into())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("out of range"), "{error:#}");
    assert_eq!(version_count(&book).await, 0);
    assert!(
        !library
            .store()
            .read(|db| Ok(db.has_unpublished()))
            .await
            .unwrap()
    );
    library.close().await.unwrap();
}

#[tokio::test]
async fn writes_to_a_deleted_book_are_refused() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    let other = library.get_book(&id).await.unwrap();
    let written_from = book.summaries().await.unwrap();
    library.delete_book(&id).await.unwrap();

    for book in [&book, &other] {
        assert!(
            book.apply_translation(0, rus(), version(1, "поздно"), "model-a".into())
                .await
                .is_err()
        );
        assert!(
            book.apply_summary(&written_from, 0, summary(1, "late"))
                .await
                .is_err()
        );
        assert!(book.update_folder_path(vec!["late".into()]).await.is_err());
        assert!(book.read(|b| b.paragraph(0)).await.is_err());
        assert!(book.summaries().await.is_err());
        assert!(book.reading_state().await.is_err());
        assert!(book.folder_path().await.is_err());
    }
    // Nothing of the book is left to land.
    library.flush().await.unwrap();
    assert!(library.list_books().await.unwrap().is_empty());
    let (_dir, peer) = peer_view(library.store());
    assert!(!peer.run_blocking(|db| db.has_book(id)).unwrap());
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_reading_position_waiting_when_its_book_is_deleted_is_dropped() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    book.update_reading_state(position(1)).await.unwrap();
    library.delete_book(&id).await.unwrap();

    assert!(book.reading_state().await.is_err());
    library.flush().await.unwrap();
    library.close().await.unwrap();
    let (_dir, peer) = peer_view(library.store());
    assert!(!peer.run_blocking(|db| db.has_book(id)).unwrap());
    assert_eq!(
        peer.run_blocking(|db| db.user_state(id)).unwrap(),
        BookUserState::default()
    );
}

#[tokio::test]
async fn a_reading_position_given_to_a_deleted_book_is_never_stored() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    library.delete_book(&id).await.unwrap();

    let _ = book.update_reading_state(position(1)).await;
    library.flush().await.unwrap();
    assert!(book.reading_state().await.is_err());
    library.close().await.unwrap();
    let (_dir, peer) = peer_view(library.store());
    assert!(!peer.run_blocking(|db| db.has_book(id)).unwrap());
    assert_eq!(
        peer.run_blocking(|db| db.user_state(id)).unwrap(),
        BookUserState::default()
    );
}

#[tokio::test]
async fn a_reading_position_given_to_a_deleted_book_is_refused() {
    let (_dir, library, book) = fixture().await;
    let id = book.id();
    library.delete_book(&id).await.unwrap();

    assert!(book.update_reading_state(position(1)).await.is_err());
    assert!(book.reading_state().await.is_err());
    library.close().await.unwrap();
}
