use std::{
    collections::BTreeMap,
    fs::File,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use isolang::Language;

use super::*;
use crate::{
    book::{chapter_summaries::ChapterSummary, translation_import},
    card::{AnkiData, AnkiState, Example},
    library::{BookHandle, Library},
    test_utils::{TempDir, copy_shards, full_word, one_sentence_paragraph},
};

const FLASH: &str = "models/gemini-2.5-flash";
const PRO: &str = "models/gemini-2.5-pro";
const CONFLICT: &str = "sync-conflict-20260520-153912-ABCDEFG";

fn rus() -> Language {
    Language::from_639_3("rus").unwrap()
}

fn write_dat<T: Serializable>(path: &Path, value: &T) {
    let mut bytes = Vec::new();
    value.serialize(&mut bytes).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Backdates `path`; the import orders sibling files by modification time.
fn aged(path: &Path, seconds: u64) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(seconds))
        .unwrap();
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
            let modified = entry.metadata().unwrap().modified().unwrap();
            File::options()
                .write(true)
                .open(&target)
                .unwrap()
                .set_modified(modified)
                .unwrap();
        }
    }
}

fn source_book(id: Uuid, title: &str) -> Book {
    let mut book = Book::create(id, title, &Language::from_639_3("eng").unwrap());
    let first = book.push_chapter(Some("One"));
    book.push_paragraph(first, "alpha one", Some("<p>alpha <b>one</b></p>"));
    book.push_paragraph(first, "beta", None);
    let second = book.push_chapter(None);
    book.push_paragraph(second, "gamma", Some("<p>gamma</p>"));
    book
}

fn version(timestamp: u64, text: &str) -> translation_import::ParagraphTranslation {
    let mut word = full_word("alpha", "alpha", "альфа", "noun", &[text, "вариант"], false);
    word.note = Some(format!("note for {text}"));
    word.grammar.case = Some("nominative".into());
    let mut paragraph = one_sentence_paragraph(
        text,
        vec![word, full_word(".", ".", ".", "punct", &[], true)],
    );
    paragraph.timestamp = timestamp;
    paragraph.total_tokens = Some(timestamp * 3);
    paragraph
}

fn summary(timestamp: u64, text: &str) -> ChapterSummary {
    ChapterSummary {
        generated: true,
        model: FLASH.into(),
        timestamp,
        text: text.into(),
    }
}

fn example(source: &str, paragraph: usize) -> Example {
    Example {
        source: source.into(),
        translation: format!("перевод {source}"),
        book_id: Uuid::nil(),
        chapter: 0,
        paragraph,
    }
}

fn card(translation: &str, example: Example, anki_data: Option<AnkiData>) -> Card {
    Card {
        version: 2,
        id: "flts_spa_rus_poder".into(),
        lemma: "poder".into(),
        translations: BTreeMap::from([("verb".to_string(), vec![translation.to_string()])]),
        examples: vec![example],
        anki_data,
    }
}

fn anki(state: AnkiState, interval_days: f64) -> AnkiData {
    AnkiData {
        state,
        interval_days: Some(interval_days),
        ease_factor: Some(2.5),
        fsrs_difficulty: None,
        fsrs_stability: Some(7.25),
    }
}

/// A book directory as the old format left it after two devices diverged.
fn write_legacy_book(root: &Path, id: Uuid) -> PathBuf {
    let dir = root.join(id.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    write_dat(&dir.join("book.dat"), &source_book(id, "Legacy"));

    let mut canonical = Translation::create("eng", "rus");
    canonical.add_paragraph_translation(0, &version(10, "old"), FLASH);
    canonical.add_paragraph_translation(0, &version(20, "new"), PRO);
    canonical.add_visible_word(0, 1);
    canonical.add_paragraph_translation(1, &version(5, "beta"), FLASH);
    canonical.add_visible_word(1, 0);
    canonical.add_visible_word(1, 3);
    write_dat(&dir.join("translation_eng_rus.dat"), &canonical);

    let mut conflict = Translation::create("eng", "rus");
    conflict.add_paragraph_translation(0, &version(10, "old"), FLASH);
    conflict.add_paragraph_translation(0, &version(30, "theirs"), FLASH);
    conflict.add_paragraph_translation(2, &version(7, "gamma"), PRO);
    conflict.add_visible_word(2, 2);
    write_dat(
        &dir.join(format!("translation_eng_rus.{CONFLICT}.dat")),
        &conflict,
    );

    let mut german = Translation::create("eng", "deu");
    german.add_paragraph_translation(1, &version(8, "Beta"), FLASH);
    write_dat(&dir.join("translation_eng_deu.dat"), &german);

    let mut summaries = ChapterSummaries::empty_for(id, 2);
    summaries.entries[0] = summary(100, "chapter one, first take");
    write_dat(&dir.join("chapter_summaries.dat"), &summaries);
    let mut newer = ChapterSummaries::empty_for(id, 2);
    newer.entries[0] = summary(200, "chapter one, second take");
    newer.entries[1] = summary(150, "chapter two");
    write_dat(
        &dir.join(format!("chapter_summaries.{CONFLICT}.dat")),
        &newer,
    );

    let state = dir.join("state.json");
    std::fs::write(
        &state,
        r#"{"readingState":{"chapterId":0,"paragraphId":1,"pageOffset":2},"folderPath":["Shelf","Modern"]}"#,
    )
    .unwrap();
    aged(&state, 60);
    // Newer, and silent about the folder: it wins the position only.
    std::fs::write(
        dir.join(format!("state.{CONFLICT}.json")),
        r#"{"readingState":{"chapterId":1,"paragraphId":2}}"#,
    )
    .unwrap();
    dir
}

fn write_legacy_cards(root: &Path) -> PathBuf {
    let deck = root.join("cards").join("spa-rus");
    std::fs::create_dir_all(&deck).unwrap();
    let write = |name: &str, card: &Card| {
        std::fs::write(deck.join(name), serde_json::to_vec_pretty(card).unwrap()).unwrap();
    };
    write(
        "poder.json",
        &card(
            "мочь",
            example("puedo", 0),
            Some(anki(AnkiState::Active, 4.0)),
        ),
    );
    write(
        &format!("poder.{CONFLICT}.json"),
        &card(
            "уметь",
            example("pueden", 5),
            Some(anki(AnkiState::Suspended, 9.0)),
        ),
    );
    root.join("cards")
}

fn backup_root(library: &Library) -> PathBuf {
    library.store().local_dir().join(BACKUP_DIR)
}

/// Today's bucket; a test that straddles midnight fails once.
fn backup(library: &Library) -> PathBuf {
    bucket(&backup_root(library), false)
}

fn unreadable(library: &Library) -> PathBuf {
    bucket(&backup_root(library), true)
}

fn fail_commits(library: &Library, count: usize) {
    library
        .store()
        .run_blocking(|db| {
            db.fail_commits = count;
            Ok(())
        })
        .unwrap();
}

/// The first paragraph's history on a device, read fresh from its store.
async fn first_paragraph(library: &Library, id: Uuid) -> History {
    let book = library.get_book(&id).await.unwrap();
    history(&book, rus(), 0).await
}

/// `(timestamp, text, model, total_tokens)` per version, oldest first.
type History = Vec<(u64, String, String, Option<u64>)>;

async fn history(book: &BookHandle, target: Language, paragraph: usize) -> History {
    book.read(move |book| book.versions(&target, paragraph))
        .await
        .unwrap()
        .into_iter()
        .map(|v| {
            (
                v.translation.timestamp,
                v.translation.sentences[0].full_translation.clone(),
                v.model,
                v.translation.total_tokens,
            )
        })
        .collect()
}

async fn version_count(book: &BookHandle, target: Language) -> usize {
    book.read(move |book| book.version_count(&target))
        .await
        .unwrap()
}

fn entry(timestamp: u64, text: &str, model: &str) -> History {
    vec![(
        timestamp,
        text.to_string(),
        model.to_string(),
        Some(timestamp * 3),
    )]
}

async fn assert_imported_book(library: &Library, id: Uuid) -> BookHandle {
    let listed = library.list_books().await.unwrap();
    assert_eq!(listed.len(), 1, "one book, not one per file or device");
    let listing = &listed[0];
    assert_eq!(listing.id, id);
    assert_eq!(listing.title, "Legacy");
    assert_eq!((listing.chapters_count, listing.paragraphs_count), (2, 3));
    assert_eq!(listing.folder_path, ["Shelf", "Modern"]);
    let mut counts: Vec<(String, usize)> = listing
        .translations_metadata
        .iter()
        .map(|t| (t.target_language.clone(), t.translated_paragraphs_count))
        .collect();
    counts.sort();
    assert_eq!(counts, [("deu".to_string(), 1), ("rus".to_string(), 3)]);

    let book = library.get_book(&id).await.unwrap();
    let snapshot = book.snapshot();
    let source = source_book(id, "Legacy");
    assert_eq!(snapshot.book.title, source.title);
    assert_eq!(snapshot.book.language, "eng");
    assert_eq!(snapshot.book.chapter_count(), 2);
    assert_eq!(snapshot.book.paragraphs_count(), 3);
    let outline = book.read(|book| book.outline()).await.unwrap();
    assert_eq!(outline.chapters.len(), 2);
    for chapter in 0..2 {
        let (stored, original) = (&outline.chapters[chapter], source.chapter_view(chapter));
        assert_eq!(stored.title.as_deref(), original.title.as_deref());
        assert_eq!(stored.paragraphs.len(), original.paragraph_count());
        for paragraph in 0..original.paragraph_count() {
            let original = original.paragraph_view(paragraph);
            let number = stored.paragraphs[paragraph];
            assert_eq!(number, original.id);
            assert_eq!(outline.chapter_for_paragraph(number), Some(chapter));
            let stored = book.read(move |book| book.paragraph(number)).await.unwrap();
            assert_eq!(stored.text, original.original_text);
            assert_eq!(stored.html.as_deref(), original.original_html.as_deref());
        }
    }

    assert_eq!(version_count(&book, rus()).await, 5);
    assert_eq!(
        history(&book, rus(), 0).await,
        [
            entry(10, "old", FLASH),
            entry(20, "new", PRO),
            entry(30, "theirs", FLASH),
        ]
        .concat()
    );
    assert_eq!(history(&book, rus(), 1).await, entry(5, "beta", FLASH));
    assert_eq!(history(&book, rus(), 2).await, entry(7, "gamma", PRO));
    // Word-level detail survives too.
    let versions = book.read(|book| book.versions(&rus(), 0)).await.unwrap();
    assert_eq!(versions[2].translation, version(30, "theirs"));
    assert_eq!(versions[1].translation, version(20, "new"));
    assert_eq!(
        book.read(|book| book.translation(&rus(), 0)).await.unwrap(),
        Some(versions[2].clone())
    );
    let german = Language::from_639_3("deu").unwrap();
    assert_eq!(version_count(&book, german).await, 1);
    assert_eq!(history(&book, german, 1).await, entry(8, "Beta", FLASH));

    assert_eq!(
        snapshot.summaries.entries,
        [
            summary(200, "chapter one, second take"),
            summary(150, "chapter two")
        ]
    );
    assert_eq!(
        book.folder_path().await.unwrap(),
        ["Shelf", "Modern"],
        "the folder comes from the only file that names one"
    );
    book
}

async fn assert_imported_card(library: &Library) {
    let cards = library.card_store();
    assert_eq!(
        cards.list_pairs().await.unwrap(),
        [("spa".to_string(), "rus".to_string())]
    );
    assert_eq!(
        cards.list_cards_in_pair("spa", "rus").await.unwrap(),
        ["poder"]
    );
    let stored = cards.load("spa", "rus", "poder").await.unwrap().unwrap();
    assert_eq!(stored.lemma, "poder");
    assert_eq!(stored.id, "flts_spa_rus_poder");
    assert_eq!(stored.translations["verb"], ["мочь", "уметь"]);
    let mut examples = stored.examples.clone();
    examples.sort_by_key(|e| e.paragraph);
    assert_eq!(examples, [example("puedo", 0), example("pueden", 5)]);
    assert_eq!(
        stored.anki_data,
        Some(anki(AnkiState::Active, 4.0)),
        "a conflict copy must not replace the canonical retention data"
    );
}

#[tokio::test]
async fn a_legacy_library_opens_with_everything_it_held() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let id = Uuid::new_v4();
    let book_dir = write_legacy_book(&root, id);
    let cards_dir = write_legacy_cards(&root);

    let library = Library::open(root.clone()).await.unwrap();

    let book = assert_imported_book(&library, id).await;
    assert_eq!(
        book.reading_state().await.unwrap(),
        Some(BookReadingState {
            chapter_id: 1,
            paragraph_id: 2,
            page_offset: 0
        }),
        "the newest file that carries a position wins it"
    );
    assert_imported_card(&library).await;

    assert!(!book_dir.exists(), "imported files leave the synced folder");
    assert!(!cards_dir.exists());
    let kept = backup(&library);
    assert!(kept.join(id.to_string()).join("book.dat").is_file());
    assert!(
        kept.join(id.to_string())
            .join(format!("translation_eng_rus.{CONFLICT}.dat"))
            .is_file()
    );
    assert!(kept.join("cards/spa-rus/poder.json").is_file());
    drop(book);
    // Nothing depends on the files any more.
    std::fs::remove_dir_all(backup_root(&library)).unwrap();
    library.close().await.unwrap();
    drop(library);

    let library = Library::open(root).await.unwrap();
    assert_imported_book(&library, id).await;
    assert_imported_card(&library).await;
    library.close().await.unwrap();
}

#[tokio::test]
async fn importing_the_same_files_again_changes_nothing() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let pristine = dir.path.join("pristine");
    let id = Uuid::new_v4();
    write_legacy_book(&pristine, id);
    write_legacy_cards(&pristine);
    copy_tree(&pristine, &root);

    let library = Library::open(root.clone()).await.unwrap();
    let book = assert_imported_book(&library, id).await;
    // What the user did since must outlive the re-import.
    let moved_on = BookReadingState {
        chapter_id: 0,
        paragraph_id: 0,
        page_offset: 9,
    };
    book.update_reading_state(moved_on.clone()).await.unwrap();
    let reviewed = anki(AnkiState::Suspended, 30.0);
    assert!(
        library
            .card_store()
            .update_anki_data(
                "spa",
                "rus",
                "poder",
                Some(anki(AnkiState::Active, 4.0)),
                reviewed.clone()
            )
            .await
            .unwrap()
    );

    // A device still on the old format syncs the same files in again.
    copy_tree(&pristine, &root);
    let changes = library.refresh_from_peers().await.unwrap();
    assert!(
        changes.books.is_empty(),
        "the loaded book did not change: {changes:?}"
    );

    assert!(!root.join(id.to_string()).exists());
    assert!(!root.join("cards").exists());
    let book = assert_imported_book(&library, id).await;
    assert_eq!(book.reading_state().await.unwrap(), Some(moved_on.clone()));
    let stored = library
        .card_store()
        .load("spa", "rus", "poder")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.translations["verb"], ["мочь", "уметь"]);
    assert_eq!(stored.examples.len(), 2);
    assert_eq!(stored.anki_data, Some(reviewed));
    // Both arrivals are kept aside, neither overwriting the other.
    assert!(backup(&library).join(id.to_string()).is_dir());
    assert!(backup(&library).join(format!("{id}.1")).is_dir());
    drop(book);
    library.close().await.unwrap();
    drop(library);

    let library = Library::open(root).await.unwrap();
    let book = assert_imported_book(&library, id).await;
    assert_eq!(book.reading_state().await.unwrap(), Some(moved_on));
    library.close().await.unwrap();
}

#[tokio::test]
async fn legacy_files_arriving_later_update_the_loaded_book() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let id = Uuid::new_v4();
    write_legacy_book(&root, id);
    let library = Library::open(root.clone()).await.unwrap();
    let book = assert_imported_book(&library, id).await;

    // The old-format device translated one more paragraph version.
    let book_dir = root.join(id.to_string());
    std::fs::create_dir_all(&book_dir).unwrap();
    write_dat(&book_dir.join("book.dat"), &source_book(id, "Legacy"));
    let mut later = Translation::create("eng", "rus");
    later.add_paragraph_translation(1, &version(5, "beta"), FLASH);
    later.add_paragraph_translation(1, &version(40, "beta again"), PRO);
    write_dat(&book_dir.join("translation_eng_rus.dat"), &later);

    let changes = library.refresh_from_peers().await.unwrap();
    assert!(changes.library);
    assert_eq!(changes.books, [id]);
    assert_eq!(version_count(&book, rus()).await, 6);
    assert_eq!(
        history(&book, rus(), 1).await,
        [entry(5, "beta", FLASH), entry(40, "beta again", PRO)].concat()
    );
    assert!(!book_dir.exists());
    library.close().await.unwrap();
}

#[tokio::test]
async fn two_devices_importing_the_same_files_converge_on_one_book() {
    let dir = TempDir::new("flts_legacy");
    let (root_a, root_b) = (dir.path.join("a/library"), dir.path.join("b/library"));
    let id = Uuid::new_v4();
    write_legacy_book(&root_a, id);
    write_legacy_cards(&root_a);
    copy_tree(&root_a, &root_b);

    let a = Library::open(root_a).await.unwrap();
    let b = Library::open(root_b).await.unwrap();
    copy_shards(a.store().shared_dir(), b.store().shared_dir());
    copy_shards(b.store().shared_dir(), a.store().shared_dir());

    for library in [&a, &b] {
        assert!(library.refresh_from_peers().await.unwrap().library);
        let book = assert_imported_book(library, id).await;
        assert_eq!(book.reading_state().await.unwrap().unwrap().chapter_id, 1);
        assert_imported_card(library).await;
        assert_eq!(
            library.refresh_from_peers().await.unwrap(),
            crate::library::PeerChanges::default()
        );
    }
    a.close().await.unwrap();
    b.close().await.unwrap();
}

#[tokio::test]
async fn an_unreadable_book_directory_is_left_alone() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let good = Uuid::new_v4();
    write_legacy_book(&root, good);

    let garbage = root.join(Uuid::new_v4().to_string());
    std::fs::create_dir_all(&garbage).unwrap();
    std::fs::write(garbage.join("book.dat"), b"not a book").unwrap();

    // A readable book under another book's name is not that book.
    let misfiled = root.join(Uuid::new_v4().to_string());
    std::fs::create_dir_all(&misfiled).unwrap();
    write_dat(
        &misfiled.join("book.dat"),
        &source_book(Uuid::new_v4(), "Misfiled"),
    );

    let unrelated = root.join("notes");
    std::fs::create_dir_all(&unrelated).unwrap();
    std::fs::write(unrelated.join("book.dat"), b"whatever").unwrap();
    let empty = root.join(Uuid::new_v4().to_string());
    std::fs::create_dir_all(&empty).unwrap();

    let library = Library::open(root.clone()).await.unwrap();
    assert_imported_book(&library, good).await;
    assert!(!root.join(good.to_string()).exists());
    for left in [&garbage, &misfiled, &unrelated, &empty] {
        assert!(left.is_dir(), "{} must stay in place", left.display());
    }
    assert!(garbage.join("book.dat").is_file());
    // A refresh meets the same directories and still does not fail.
    library.refresh_from_peers().await.unwrap();
    assert_eq!(
        migrate(&root, library.store()).await.unwrap(),
        Imported {
            books: 0,
            cards: false,
            failed: 2
        },
        "the two unreadable books are counted, the empty directory is not"
    );
    assert!(garbage.join("book.dat").is_file());
    assert_eq!(library.list_books().await.unwrap().len(), 1);
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_legacy_copy_of_a_deleted_book_does_not_bring_it_back() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let pristine = dir.path.join("pristine");
    let id = Uuid::new_v4();
    write_legacy_book(&pristine, id);
    copy_tree(&pristine, &root);

    let library = Library::open(root.clone()).await.unwrap();
    assert_eq!(library.list_books().await.unwrap().len(), 1);
    library.delete_book(&id).await.unwrap();
    assert!(library.list_books().await.unwrap().is_empty());

    copy_tree(&pristine, &root);
    library.refresh_from_peers().await.unwrap();
    assert!(library.list_books().await.unwrap().is_empty());
    assert!(library.get_book(&id).await.is_err());
    assert!(
        !root.join(id.to_string()).exists(),
        "the stale copy is deleted so it stops syncing"
    );
    let kept = book_backups(&backup_root(&library));
    assert!(
        kept.iter().all(|(_, owner)| *owner != id),
        "a deleted book keeps no backup copy"
    );
    library.close().await.unwrap();
    drop(library);

    copy_tree(&pristine, &root);
    let library = Library::open(root.clone()).await.unwrap();
    assert!(library.list_books().await.unwrap().is_empty());
    assert!(!root.join(id.to_string()).exists());
    library.close().await.unwrap();
}

#[tokio::test]
async fn unfinished_writes_are_ignored() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let id = Uuid::new_v4();
    let book_dir = write_legacy_book(&root, id);
    let cards_dir = write_legacy_cards(&root);

    write_dat(
        &book_dir.join("book~tmp1.dat"),
        &source_book(id, "Half-written title"),
    );
    let mut partial = Translation::create("eng", "rus");
    partial.add_paragraph_translation(1, &version(99, "half-written"), FLASH);
    write_dat(&book_dir.join("translation_eng_rus~tmp2.dat"), &partial);
    let mut summaries = ChapterSummaries::empty_for(id, 2);
    summaries.entries[1] = summary(999, "half-written");
    write_dat(&book_dir.join("chapter_summaries~tmp3.dat"), &summaries);
    std::fs::write(
        book_dir.join("state~tmp4.json"),
        r#"{"readingState":{"chapterId":0,"paragraphId":0},"folderPath":["Half"]}"#,
    )
    .unwrap();
    std::fs::write(
        cards_dir.join("spa-rus").join("poder~tmp5.json"),
        serde_json::to_vec(&card("half-written", example("x", 9), None)).unwrap(),
    )
    .unwrap();

    let library = Library::open(root).await.unwrap();
    let book = assert_imported_book(&library, id).await;
    assert_eq!(book.reading_state().await.unwrap().unwrap().chapter_id, 1);
    assert_imported_card(&library).await;
    library.close().await.unwrap();
}

#[tokio::test]
async fn the_newest_readable_copy_of_the_book_is_the_book() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let id = Uuid::new_v4();
    let book_dir = root.join(id.to_string());
    std::fs::create_dir_all(&book_dir).unwrap();
    write_dat(&book_dir.join("book.dat"), &source_book(id, "Older"));
    aged(&book_dir.join("book.dat"), 120);
    let renamed = book_dir.join(format!("book.{CONFLICT}.dat"));
    write_dat(&renamed, &source_book(id, "Newer"));
    aged(&renamed, 60);
    std::fs::write(book_dir.join("book.sync-conflict-torn.dat"), b"torn").unwrap();
    // Summaries for some other shape of book are not this book's.
    write_dat(
        &book_dir.join("chapter_summaries.dat"),
        &ChapterSummaries {
            book_id: id,
            entries: vec![summary(1, "one"), summary(2, "two"), summary(3, "three")],
        },
    );

    let library = Library::open(root).await.unwrap();
    let listed = library.list_books().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Newer");
    assert!(listed[0].folder_path.is_empty());
    let book = library.get_book(&id).await.unwrap();
    assert_eq!(book.reading_state().await.unwrap(), None);
    assert!(
        book.snapshot()
            .summaries
            .entries
            .iter()
            .all(|s| !s.generated)
    );
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_bare_legacy_state_file_is_a_reading_position() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let id = Uuid::new_v4();
    let book_dir = root.join(id.to_string());
    std::fs::create_dir_all(&book_dir).unwrap();
    write_dat(&book_dir.join("book.dat"), &source_book(id, "Bare"));
    std::fs::write(
        book_dir.join("state.json"),
        r#"{"chapterId":1,"paragraphId":2}"#,
    )
    .unwrap();

    let library = Library::open(root).await.unwrap();
    let book = library.get_book(&id).await.unwrap();
    assert_eq!(
        book.reading_state().await.unwrap(),
        Some(BookReadingState {
            chapter_id: 1,
            paragraph_id: 2,
            page_offset: 0
        })
    );
    assert!(book.folder_path().await.unwrap().is_empty());
    library.close().await.unwrap();
}

#[test]
fn state_files_parse_in_every_shape_the_old_format_wrote() {
    assert_eq!(parse_user_state("  \n"), Some(StateFile::default()));
    assert_eq!(
        parse_user_state(r#"{"chapter_id":3,"paragraph_id":7,"page_offset":2}"#),
        Some(StateFile {
            reading_state: Some(Some(BookReadingState {
                chapter_id: 3,
                paragraph_id: 7,
                page_offset: 2
            })),
            folder_path: None
        })
    );
    assert_eq!(
        parse_user_state(r#"{"folderPath":["a","b"]}"#),
        Some(StateFile {
            reading_state: None,
            folder_path: Some(vec!["a".into(), "b".into()])
        })
    );
    assert_eq!(
        parse_user_state(r#"{"readingState":null,"folderPath":[]}"#),
        Some(StateFile {
            reading_state: Some(None),
            folder_path: Some(vec![])
        })
    );
    assert_eq!(parse_user_state("{not json"), None);
    assert_eq!(parse_user_state(r#"{"unrelated":1}"#), None);
}

#[test]
fn state_files_union_field_by_field_with_the_newest_winning() {
    let dir = TempDir::new("flts_legacy_state");
    let write = |name: &str, contents: &str, age: u64| {
        let path = dir.path.join(name);
        std::fs::write(&path, contents).unwrap();
        aged(&path, age);
    };
    write(
        "state.json",
        r#"{"readingState":{"chapterId":1,"paragraphId":1},"folderPath":["old"]}"#,
        300,
    );
    write(
        "state.sync-conflict-a.json",
        r#"{"folderPath":["newer"]}"#,
        200,
    );
    write(
        "state.sync-conflict-b.json",
        r#"{"readingState":{"chapterId":2,"paragraphId":5,"pageOffset":1}}"#,
        100,
    );
    write("state.sync-conflict-c.json", "garbage", 50);
    write("state.sync-conflict-d.json", "", 10);

    assert_eq!(
        user_state(&dir.path),
        (
            BookUserState {
                reading_state: Some(BookReadingState {
                    chapter_id: 2,
                    paragraph_id: 5,
                    page_offset: 1
                }),
                folder_path: vec!["newer".into()]
            },
            false
        ),
        "and the file that would not parse is reported"
    );
}

#[test]
fn the_newest_state_file_can_empty_what_an_older_one_set() {
    let dir = TempDir::new("flts_legacy_state");
    let older = dir.path.join("state.sync-conflict-a.json");
    std::fs::write(
        &older,
        r#"{"readingState":{"chapterId":1,"paragraphId":1},"folderPath":["Shelf"]}"#,
    )
    .unwrap();
    aged(&older, 300);
    // The user moved the book back out of its folder afterwards.
    std::fs::write(
        dir.path.join("state.json"),
        r#"{"readingState":{"chapterId":2,"paragraphId":0},"folderPath":[]}"#,
    )
    .unwrap();

    let (state, clean) = user_state(&dir.path);
    assert!(clean);
    assert!(state.folder_path.is_empty());
    assert_eq!(state.reading_state.unwrap().chapter_id, 2);
}

#[tokio::test]
async fn a_library_without_legacy_files_imports_nothing() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let library = Library::open(root.clone()).await.unwrap();
    assert_eq!(
        migrate(&root, library.store()).await.unwrap(),
        Imported::default()
    );
    assert!(!backup_root(&library).exists());
    library.close().await.unwrap();
}

#[tokio::test]
async fn annotations_arriving_after_their_book_are_imported() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let id = Uuid::new_v4();
    let book_dir = root.join(id.to_string());
    std::fs::create_dir_all(&book_dir).unwrap();
    write_dat(&book_dir.join("book.dat"), &source_book(id, "Early"));
    let library = Library::open(root.clone()).await.unwrap();
    assert!(!book_dir.exists());

    // The rest of the directory is delivered once the book is gone from it.
    std::fs::create_dir_all(&book_dir).unwrap();
    let mut late = Translation::create("eng", "rus");
    late.add_paragraph_translation(0, &version(10, "late"), FLASH);
    late.add_visible_word(0, 1);
    write_dat(&book_dir.join("translation_eng_rus.dat"), &late);
    let mut summaries = ChapterSummaries::empty_for(id, 2);
    summaries.entries[1] = summary(50, "late summary");
    write_dat(&book_dir.join("chapter_summaries.dat"), &summaries);
    std::fs::write(
        book_dir.join("state.json"),
        r#"{"readingState":{"chapterId":1,"paragraphId":2},"folderPath":["Late"]}"#,
    )
    .unwrap();
    // And files of a book this device has never heard of.
    let orphan = root.join(Uuid::new_v4().to_string());
    std::fs::create_dir_all(&orphan).unwrap();
    write_dat(&orphan.join("translation_eng_rus.dat"), &late);

    assert_eq!(
        migrate(&root, library.store()).await.unwrap(),
        Imported {
            books: 1,
            cards: false,
            failed: 0
        }
    );
    assert!(!book_dir.exists());
    assert!(
        backup(&library)
            .join(format!("{id}.1"))
            .join("translation_eng_rus.dat")
            .is_file()
    );
    assert!(
        orphan.join("translation_eng_rus.dat").is_file(),
        "files without their book wait for it"
    );
    assert_eq!(
        migrate(&root, library.store()).await.unwrap(),
        Imported::default()
    );

    let book = library.get_book(&id).await.unwrap();
    let snapshot = book.snapshot();
    assert_eq!(history(&book, rus(), 0).await, entry(10, "late", FLASH));
    assert_eq!(snapshot.summaries.entries[1], summary(50, "late summary"));
    assert_eq!(book.reading_state().await.unwrap().unwrap().chapter_id, 1);
    assert_eq!(book.folder_path().await.unwrap(), ["Late"]);
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_directory_still_being_delivered_is_not_touched() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let id = Uuid::new_v4();
    let book_dir = write_legacy_book(&root, id);
    let cards_dir = write_legacy_cards(&root);
    let (book_temp, card_temp) = (
        book_dir.join(".syncthing.translation_eng_deu.dat.tmp"),
        cards_dir.join("spa-rus/.syncthing.querer.json.tmp"),
    );
    std::fs::write(&book_temp, b"half").unwrap();
    std::fs::write(&card_temp, b"half").unwrap();

    let library = Library::open(root.clone()).await.unwrap();
    assert!(library.list_books().await.unwrap().is_empty());
    assert!(library.card_store().list_pairs().await.unwrap().is_empty());
    assert!(book_dir.join("book.dat").is_file());
    assert!(cards_dir.join("spa-rus/poder.json").is_file());
    assert!(!backup_root(&library).exists());

    std::fs::remove_file(book_temp).unwrap();
    std::fs::remove_file(card_temp).unwrap();
    assert_eq!(
        migrate(&root, library.store()).await.unwrap(),
        Imported {
            books: 1,
            cards: true,
            failed: 0
        }
    );
    assert_imported_book(&library, id).await;
    assert_imported_card(&library).await;
    assert!(!book_dir.exists() && !cards_dir.exists());
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_book_that_fails_to_commit_does_not_stop_the_others() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let library = Library::open(root.clone()).await.unwrap();
    let mut ids = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    ids.sort_by_key(|id| id.to_string());
    for id in ids {
        write_legacy_book(&root, id);
    }

    fail_commits(&library, 1);
    assert_eq!(
        migrate(&root, library.store()).await.unwrap(),
        Imported {
            books: 3,
            cards: false,
            failed: 1
        }
    );
    assert!(
        root.join(ids[0].to_string()).join("book.dat").is_file(),
        "what did not commit stays to be tried again"
    );
    for id in &ids[1..] {
        assert!(!root.join(id.to_string()).exists());
        assert!(backup(&library).join(id.to_string()).is_dir());
    }

    assert_eq!(
        migrate(&root, library.store()).await.unwrap(),
        Imported::default(),
        "the retry inserts nothing new and reports nothing"
    );
    assert!(!root.join(ids[0].to_string()).exists());
    assert_eq!(library.list_books().await.unwrap().len(), 3);
    library.close().await.unwrap();
}

#[tokio::test]
async fn members_that_do_not_parse_are_kept_for_good() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let id = Uuid::new_v4();
    let book_dir = write_legacy_book(&root, id);
    std::fs::write(book_dir.join("translation_eng_fra.dat"), b"torn").unwrap();
    write_dat(
        &book_dir.join("chapter_summaries.sync-conflict-other.dat"),
        &ChapterSummaries {
            book_id: id,
            entries: vec![summary(1, "one"), summary(2, "two"), summary(3, "three")],
        },
    );
    let stale = book_dir.join("state.sync-conflict-torn.json");
    std::fs::write(&stale, "{torn").unwrap();
    aged(&stale, 600);
    let cards_dir = write_legacy_cards(&root);
    std::fs::write(cards_dir.join("spa-rus/querer.json"), b"{torn").unwrap();
    let clean = Uuid::new_v4();
    write_legacy_book(&root, clean);

    let library = Library::open(root.clone()).await.unwrap();
    assert_eq!(library.list_books().await.unwrap().len(), 2);
    assert_imported_card(&library).await;
    assert!(!book_dir.exists() && !cards_dir.exists());

    let kept = unreadable(&library);
    for member in [
        "translation_eng_fra.dat",
        "chapter_summaries.sync-conflict-other.dat",
        "state.sync-conflict-torn.json",
        "book.dat",
    ] {
        assert!(kept.join(id.to_string()).join(member).is_file(), "{member}");
    }
    assert!(kept.join("cards/spa-rus/querer.json").is_file());
    assert!(backup(&library).join(clean.to_string()).is_dir());
    assert!(!backup(&library).join(id.to_string()).exists());
    assert!(!backup(&library).join("cards").exists());

    // The same delivery again reports its failures, and nothing as imported.
    write_legacy_book(&root, id);
    std::fs::write(book_dir.join("translation_eng_fra.dat"), b"torn").unwrap();
    assert_eq!(
        migrate(&root, library.store()).await.unwrap(),
        Imported {
            books: 0,
            cards: false,
            failed: 1
        }
    );
    assert!(kept.join(format!("{id}.1")).is_dir());
    library.close().await.unwrap();
}

#[test]
fn buckets_are_named_by_utc_day() {
    for (day, name) in [
        (0, "19700101"),
        (11_016, "20000229"),
        (11_017, "20000301"),
        (19_782, "20240229"),
        (20_453, "20251231"),
        (20_454, "20260101"),
    ] {
        assert_eq!(bucket_name(day), name);
        assert_eq!(bucket_day(name), Some(day));
    }
    for other in ["unreadable", "20260230", "20261301", "2026010", "cards"] {
        assert_eq!(bucket_day(other), None, "{other}");
    }
    assert_eq!(bucket_day(&bucket_name(today())), Some(today()));
}

#[tokio::test]
async fn the_sweep_drops_old_buckets_and_the_backups_of_deleted_books() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let (gone, stays) = (Uuid::new_v4(), Uuid::new_v4());
    write_legacy_book(&root, gone);
    write_legacy_book(&root, stays);
    let library = Library::open(root.clone()).await.unwrap();
    library
        .store()
        .run_blocking(|db| db.delete_book(gone))
        .unwrap();

    let kept = backup_root(&library);
    let now = today();
    let retention = BACKUP_RETENTION_DAYS as i64;
    let (expired, last_day) = (
        kept.join(bucket_name(now - retention - 1)),
        kept.join(bucket_name(now - retention)),
    );
    let never = kept.join(UNREADABLE);
    for path in [
        expired.join(stays.to_string()),
        expired.join("cards/spa-rus"),
        last_day.join(stays.to_string()),
        last_day.join(format!("{gone}.2")),
        never.join(stays.to_string()),
        never.join(format!("{gone}.1")),
        never.join("cards"),
        kept.join(gone.to_string()),
    ] {
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("member"), b"kept").unwrap();
    }
    std::fs::write(last_day.join("devices.json"), b"{}").unwrap();

    sweep(library.store(), now).unwrap();

    assert!(!expired.exists(), "past the retention period");
    assert!(last_day.join(stays.to_string()).is_dir());
    assert!(last_day.join("devices.json").is_file());
    assert!(never.join(stays.to_string()).is_dir());
    assert!(never.join("cards").is_dir());
    assert!(backup(&library).join(stays.to_string()).is_dir());
    assert!(
        book_backups(&kept).iter().all(|(_, owner)| *owner != gone),
        "a deleted book keeps no backup in any bucket"
    );

    remove_backups_of(library.store().local_dir(), stays).unwrap();
    assert!(book_backups(&kept).is_empty());
    assert!(never.join("cards").is_dir());
    assert!(last_day.join("devices.json").is_file());
    library.close().await.unwrap();
}

#[tokio::test]
async fn a_directory_that_cannot_be_set_aside_is_reported_once() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    let library = Library::open(root.clone()).await.unwrap();
    // A file where the backups would go: nothing can be moved there.
    std::fs::write(backup_root(&library), b"in the way").unwrap();
    let id = Uuid::new_v4();
    let book_dir = write_legacy_book(&root, id);
    let cards_dir = write_legacy_cards(&root);

    assert_eq!(
        migrate(&root, library.store()).await.unwrap(),
        Imported {
            books: 1,
            cards: true,
            failed: 2
        }
    );
    for _ in 0..2 {
        let again = migrate(&root, library.store()).await.unwrap();
        assert!(!again.any(), "nothing new was inserted: {again:?}");
        assert_eq!(again.failed, 2);
    }
    assert!(book_dir.join("book.dat").is_file());
    assert!(cards_dir.join("spa-rus/poder.json").is_file());
    assert_imported_book(&library, id).await;

    std::fs::remove_file(backup_root(&library)).unwrap();
    assert_eq!(
        migrate(&root, library.store()).await.unwrap(),
        Imported::default()
    );
    assert!(!book_dir.exists() && !cards_dir.exists());
    assert!(backup(&library).join(id.to_string()).is_dir());
    library.close().await.unwrap();
}

#[test]
fn a_refused_rename_falls_back_to_copying() {
    let dir = TempDir::new("flts_legacy_move");
    let (from, to) = (dir.path.join("from"), dir.path.join("elsewhere/to"));
    std::fs::create_dir_all(from.join("cards/spa-rus")).unwrap();
    std::fs::write(from.join("book.dat"), b"book").unwrap();
    std::fs::write(from.join("cards/spa-rus/poder.json"), b"card").unwrap();
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    let cross_device = |_: &Path, _: &Path| Err(std::io::Error::other("cross-device link"));

    relocate(&from, &to, cross_device).unwrap();
    assert!(!from.exists());
    assert_eq!(std::fs::read(to.join("book.dat")).unwrap(), b"book");
    assert_eq!(
        std::fs::read(to.join("cards/spa-rus/poder.json")).unwrap(),
        b"card"
    );

    // A copy that cannot finish leaves the source whole and no half backup.
    let file = dir.path.join("devices.json");
    std::fs::write(&file, b"{}").unwrap();
    let nowhere = dir.path.join("missing/devices.json");
    assert!(relocate(&file, &nowhere, cross_device).is_err());
    assert!(file.is_file() && !nowhere.exists());
}

#[tokio::test]
async fn a_conflict_copy_imported_alone_or_with_its_main_file_is_the_same_rows() {
    let dir = TempDir::new("flts_legacy");
    let (root_a, root_b) = (dir.path.join("a/library"), dir.path.join("b/library"));
    let id = Uuid::new_v4();
    let conflict_name = format!("translation_eng_rus.{CONFLICT}.dat");
    // Two devices translated the same paragraph in the same second.
    let mut main = Translation::create("eng", "rus");
    main.add_paragraph_translation(0, &version(10, "mine"), FLASH);
    let mut conflict = Translation::create("eng", "rus");
    conflict.add_paragraph_translation(0, &version(10, "theirs"), PRO);
    for root in [&root_a, &root_b] {
        let book_dir = root.join(id.to_string());
        std::fs::create_dir_all(&book_dir).unwrap();
        write_dat(&book_dir.join("book.dat"), &source_book(id, "Shared"));
        write_dat(&book_dir.join(&conflict_name), &conflict);
    }
    write_dat(
        &root_a.join(id.to_string()).join("translation_eng_rus.dat"),
        &main,
    );

    let a = Library::open(root_a).await.unwrap();
    let b = Library::open(root_b).await.unwrap();
    copy_shards(a.store().shared_dir(), b.store().shared_dir());
    copy_shards(b.store().shared_dir(), a.store().shared_dir());
    for library in [&a, &b] {
        library
            .store()
            .run_blocking(|db| db.refresh().map(drop))
            .unwrap();
    }

    let (on_a, on_b) = (first_paragraph(&a, id).await, first_paragraph(&b, id).await);
    assert_eq!(on_a, on_b);
    let mut texts: Vec<&str> = on_a.iter().map(|version| version.1.as_str()).collect();
    texts.sort();
    assert_eq!(texts, ["mine", "theirs"], "each version once: {on_a:?}");
    a.close().await.unwrap();
    b.close().await.unwrap();
}

#[tokio::test]
async fn an_import_at_open_says_how_far_along_it_is() {
    let dir = TempDir::new("flts_legacy");
    let root = dir.path.join("library");
    write_legacy_book(&root, Uuid::new_v4());
    write_legacy_book(&root, Uuid::new_v4());
    write_legacy_cards(&root);
    let told = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let progress: Progress = {
        let told = told.clone();
        std::sync::Arc::new(move |done, total| told.lock().unwrap().push((done, total)))
    };

    let library = Library::open_reporting(root.clone(), progress.clone())
        .await
        .unwrap();
    assert_eq!(*told.lock().unwrap(), [(0, 3), (1, 3), (2, 3), (3, 3)]);
    assert_eq!(library.list_books().await.unwrap().len(), 2);
    library.close().await.unwrap();
    drop(library);

    // Nothing left to import, so nothing to say.
    told.lock().unwrap().clear();
    let library = Library::open_reporting(root, progress).await.unwrap();
    assert!(told.lock().unwrap().is_empty());
    library.close().await.unwrap();
}
