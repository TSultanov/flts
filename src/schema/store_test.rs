use super::*;
use crate::id::Id;
use crate::schema::Table;
use crate::storage::Arena;
use crate::test_utils::TempDir;
use crate::test_utils::library::{Book, Library};

fn this_device() -> WriterId {
    WriterId::from_device_id("KRZH3XY-7PH4NJK-2WQ5LMC")
}

fn stock(library: &mut Library) -> Id<Book> {
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    let rabbit = library
        .chapters
        .push(alice, "Down the Rabbit-Hole")
        .unwrap();
    library.paragraphs.push(rabbit, "Alice was tired.").unwrap();
    library
        .paragraphs
        .push(rabbit, "So she considered.")
        .unwrap();
    alice
}

fn refused(opened: Result<Library>) -> Error {
    match opened {
        Ok(_) => panic!("this store must not have opened"),
        Err(err) => err,
    }
}

fn len_of(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().len()
}

#[test]
fn a_store_that_was_never_opened_says_so_rather_than_guessing() {
    let mut detached = Library::default();

    assert_eq!(detached.generation(), None);
    let err = detached.commit(this_device()).unwrap_err();
    assert!(err.to_string().contains("not attached to a file"), "{err}");
    assert!(detached.verify().is_err());
}

#[test]
fn a_detached_store_is_still_a_usable_database() {
    let mut library = Library::default();
    let alice = stock(&mut library);

    assert_eq!(library.chapters_of(alice).unwrap().len(), 1);
    assert_eq!(Table::rows(&library.paragraphs), 2);
}

#[test]
fn a_created_store_holds_every_column_its_schema_names() {
    let dir = TempDir::new("create");
    let path = dir.join("library.dbs");

    let library = Library::create(&path, this_device()).unwrap();
    library.verify().unwrap();

    let file = ColumnFile::open(&path).unwrap();
    for spec in library.specs() {
        assert!(
            file.column_meta(&spec.name).is_some(),
            "{} is missing from a file this schema just created",
            spec.name
        );
    }
    assert_eq!(
        library.generation(),
        Some(2),
        "generation 1 is the empty file the columns were committed into"
    );
}

#[test]
fn creating_a_store_refuses_to_flatten_a_file_that_is_already_there() {
    let dir = TempDir::new("clobber");
    let path = dir.join("library.dbs");

    let mut first = Library::create(&path, this_device()).unwrap();
    stock(&mut first);
    first.commit(this_device()).unwrap();

    let err = refused(Library::create(&path, this_device()));
    assert!(matches!(err, Error::Io(_)), "{err}");
    assert_eq!(
        Library::open(&path)
            .unwrap()
            .books
            .title(Id::from_index(0))
            .unwrap(),
        "Alice",
        "the file that was already there is untouched"
    );
}

#[test]
fn opening_a_file_that_is_not_there_says_so_and_open_or_create_makes_one() {
    let dir = TempDir::new("open_or_create");
    let path = dir.join("library.dbs");

    let err = refused(Library::open(&path));
    assert!(matches!(err, Error::Io(_)), "{err}");

    let made = Library::open_or_create(&path, this_device()).unwrap();
    assert_eq!(made.generation(), Some(2));

    let found = Library::open_or_create(&path, this_device()).unwrap();
    assert_eq!(
        found.generation(),
        Some(2),
        "the second call opened the file rather than making another"
    );
}

#[test]
fn a_store_round_trips_every_table_through_a_commit() {
    let dir = TempDir::new("roundtrip");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    let alice = stock(&mut library);
    assert_eq!(library.commit(this_device()).unwrap(), 3);

    let reopened = Library::open(&path).unwrap();
    reopened.verify().unwrap();

    assert_eq!(reopened.books.title(alice).unwrap(), "Alice");
    assert_eq!(reopened.chapters_of(alice).unwrap().len(), 1);
    let chapter = reopened.chapters_of(alice).unwrap()[0];
    assert_eq!(reopened.paragraphs_of(chapter).unwrap().len(), 2);
    assert_eq!(
        reopened
            .paragraphs
            .text(reopened.paragraphs_of(chapter).unwrap()[1])
            .unwrap(),
        "So she considered."
    );
}

#[test]
fn a_commit_that_changes_nothing_does_not_move_the_generation_or_the_file() {
    let dir = TempDir::new("idempotent");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    stock(&mut library);
    let published = library.commit(this_device()).unwrap();
    let before = len_of(&path);

    assert_eq!(library.commit(this_device()).unwrap(), published);
    assert_eq!(library.generation(), Some(published));
    assert_eq!(len_of(&path), before, "an unchanged commit writes nothing");
}

#[test]
fn a_second_commit_appends_only_the_chunks_that_changed() {
    let dir = TempDir::new("append_only");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    let alice = stock(&mut library);
    library.commit(this_device()).unwrap();

    let settled = ColumnFile::open(&path).unwrap();
    let first_paragraph_chunk = settled.column_meta("paragraph.text").unwrap().chunks[0];
    let book_title_chunk = settled.column_meta("book.title").unwrap().chunks[0];
    drop(settled);

    library.books.push("Kokoro", "Natsume Soseki").unwrap();
    library.commit(this_device()).unwrap();

    let grown = ColumnFile::open(&path).unwrap();
    assert_eq!(
        grown.column_meta("paragraph.text").unwrap().chunks[0],
        first_paragraph_chunk,
        "a table nobody touched must not be rewritten"
    );
    assert_ne!(
        grown.column_meta("book.title").unwrap().chunks[0],
        book_title_chunk,
        "the table that grew did move its tail chunk"
    );
    assert_eq!(Table::rows(&library.books), 2);
    let _ = alice;
}

#[test]
fn committing_twice_in_a_row_still_reuses_the_chunks_it_already_wrote() {
    let dir = TempDir::new("provenance");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    stock(&mut library);
    library.commit(this_device()).unwrap();

    library.books.push("Kokoro", "Natsume Soseki").unwrap();
    library.commit(this_device()).unwrap();
    let after_second = len_of(&path);
    let paragraph_chunk = ColumnFile::open(&path)
        .unwrap()
        .column_meta("paragraph.text")
        .unwrap()
        .chunks[0];

    library
        .books
        .push("Il nome della rosa", "Umberto Eco")
        .unwrap();
    library.commit(this_device()).unwrap();

    assert_eq!(
        ColumnFile::open(&path)
            .unwrap()
            .column_meta("paragraph.text")
            .unwrap()
            .chunks[0],
        paragraph_chunk,
        "a store that did not re-read after committing would rewrite every column here"
    );
    let grew_by = len_of(&path) - after_second;
    assert!(
        grew_by < 2000,
        "the third commit appended {grew_by} bytes, which is the whole database, not the change"
    );
}

#[test]
fn a_snapshot_keeps_serving_the_generation_it_was_taken_at() {
    let dir = TempDir::new("snapshot");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    stock(&mut library);
    library.commit(this_device()).unwrap();

    let pinned = library.snapshot();
    let was = pinned.generation();

    library.books.push("Kokoro", "Natsume Soseki").unwrap();
    library.commit(this_device()).unwrap();

    assert_eq!(pinned.generation(), was);
    assert_eq!(
        Table::rows(&pinned.books),
        1,
        "the snapshot did not see the second book"
    );
    assert_eq!(Table::rows(&library.books), 2);
}

#[test]
fn a_snapshot_that_tries_to_commit_is_told_the_file_moved_on() {
    let dir = TempDir::new("snapshot_conflict");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    stock(&mut library);
    library.commit(this_device()).unwrap();

    let mut pinned = library.snapshot();
    library.books.push("Kokoro", "Natsume Soseki").unwrap();
    library.commit(this_device()).unwrap();

    pinned.books.push("Alice again", "Lewis Carroll").unwrap();
    let err = pinned.commit(this_device()).unwrap_err();
    assert!(matches!(err, Error::Conflict { .. }), "{err}");
    assert!(err.to_string().contains("moved to generation"), "{err}");
}

#[test]
fn a_column_this_schema_does_not_know_is_carried_through_a_commit() {
    let dir = TempDir::new("unknown");
    let path = dir.join("library.dbs");

    let library = Library::create(&path, this_device()).unwrap();
    drop(library);

    let file = ColumnFile::open(&path).unwrap();
    let mut newer = Arena::<u64>::with_chunk_rows(4);
    newer.extend_from_slice(&[7, 7, 7]).unwrap();
    let mut updater = file.update(this_device()).unwrap();
    for column in file.columns() {
        updater.keep_column(&column.name).unwrap();
    }
    updater.column("book.isbn", &newer).unwrap();
    updater.commit().unwrap();

    let mut older = Library::open(&path).unwrap();
    assert_eq!(older.unknown_columns(), ["book.isbn"]);

    older.books.push("Alice", "Lewis Carroll").unwrap();
    older.commit(this_device()).unwrap();

    let after = ColumnFile::open(&path).unwrap();
    assert_eq!(
        after
            .column::<u64>("book.isbn")
            .unwrap()
            .iter()
            .map(|row| row.copied())
            .collect::<Result<Vec<_>>>()
            .unwrap(),
        vec![7, 7, 7],
        "an older binary preserved a newer one's column rather than dropping it"
    );
}

#[cfg(unix)]
#[test]
fn open_or_create_reports_a_file_it_cannot_read_rather_than_claiming_it_exists() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("unreadable");
    let path = dir.join("library.dbs");
    Library::create(&path, this_device()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(
        std::fs::File::open(&path).is_err(),
        "this test needs a file the user running it cannot read"
    );

    match refused(Library::open_or_create(&path, this_device())) {
        Error::Io(io) => assert_ne!(
            io.kind(),
            std::io::ErrorKind::AlreadyExists,
            "the file is there and unreadable; reporting that it already exists would hide why"
        ),
        other => panic!("an unreadable file is an io error, not {other}"),
    }
}
