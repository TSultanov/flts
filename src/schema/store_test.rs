use super::*;
use crate::id::Id;
use crate::schema::{HashIndex, Table, TextKey};
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
fn a_store_verifies_the_indexes_the_file_hands_it_and_not_only_its_checksums() {
    let dir = TempDir::new("store_audit");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    stock(&mut library);
    library.commit(this_device()).unwrap();
    library.verify().unwrap();

    let mut foreign = HashIndex::<Id<Book>>::new("book.id".to_owned());
    for _ in 0..Table::rows(&library.books) {
        foreign.append(Id::mint()).unwrap();
    }
    let file = ColumnFile::open(&path).unwrap();
    let kept: Vec<String> = file
        .columns()
        .iter()
        .map(|column| column.name.clone())
        .filter(|name| name != "book.id.buckets" && name != "book.id.links")
        .collect();
    let mut updater = file.update(this_device()).unwrap();
    for name in &kept {
        updater.keep_column(name).unwrap();
    }
    foreign.offer_to("book", "id", &mut updater).unwrap();
    updater.commit().unwrap();

    let reopened = Library::open(&path).unwrap();
    let err = reopened.verify().unwrap_err();
    assert!(
        err.is_corrupt(),
        "every chunk checksums, and the index still lies: {err}"
    );
    assert!(err.to_string().contains("book.id"), "{err}");
}

#[test]
fn a_store_verifies_the_index_over_a_value_and_not_only_the_one_over_an_id() {
    let dir = TempDir::new("store_audit_value");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    stock(&mut library);
    library.commit(this_device()).unwrap();
    library.verify().unwrap();

    let mut foreign = HashIndex::<TextKey>::new("book.title".to_owned());
    for row in 0..Table::rows(&library.books) {
        foreign
            .append(TextKey::of(&format!("no book is called this {row}")))
            .unwrap();
    }
    let file = ColumnFile::open(&path).unwrap();
    let kept: Vec<String> = file
        .columns()
        .iter()
        .map(|column| column.name.clone())
        .filter(|name| name != "book.title.buckets" && name != "book.title.links")
        .collect();
    let mut updater = file.update(this_device()).unwrap();
    for name in &kept {
        updater.keep_column(name).unwrap();
    }
    foreign.offer_to("book", "title", &mut updater).unwrap();
    updater.commit().unwrap();

    let reopened = Library::open(&path).unwrap();
    let err = reopened.verify().unwrap_err();
    assert!(
        err.is_corrupt(),
        "an index that files a row under a value it does not hold is damage: {err}"
    );
    assert!(err.to_string().contains("book.title"), "{err}");
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
    let alice = stock(&mut first);
    first.commit(this_device()).unwrap();

    let err = refused(Library::create(&path, this_device()));
    assert!(matches!(err, Error::Io(_)), "{err}");
    assert_eq!(
        Library::open(&path).unwrap().books.title(alice).unwrap(),
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
fn an_index_a_commit_wrote_is_the_index_the_next_commit_reuses() {
    let dir = TempDir::new("index_reuse");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    let alice = stock(&mut library);
    library.commit(this_device()).unwrap();

    let was = ColumnFile::open(&path).unwrap();
    let chapters = was.column_meta("chapter.book.links").unwrap().chunks[0];
    let books = was.column_meta("book.id.links").unwrap().chunks[0];
    drop(was);

    library.books.push("Kokoro", "Natsume Soseki").unwrap();
    library.commit(this_device()).unwrap();

    let now = ColumnFile::open(&path).unwrap();
    assert_eq!(
        now.column_meta("chapter.book.links").unwrap().chunks[0],
        chapters,
        "a table nobody touched must not rewrite the index it already had"
    );
    assert_ne!(
        now.column_meta("book.id.links").unwrap().chunks[0],
        books,
        "the table that grew wrote the one chunk that changed"
    );
    assert_eq!(library.chapters_of(alice).unwrap().len(), 1);
}

#[test]
fn a_snapshot_of_a_store_costs_nothing_the_rows_it_holds_can_measure() {
    let dir = TempDir::new("snapshot_cost");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    stock(&mut library);
    library.commit(this_device()).unwrap();

    let opened = Library::open(&path).unwrap();
    let pinned = opened.snapshot();

    assert_eq!(
        pinned.residency(),
        opened.residency(),
        "an index is columns held by refcount, not a map copied per table"
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
        grew_by * 4 < len_of(&path),
        "the third commit appended {grew_by} bytes of a {} byte database: the directory is \
         rewritten whole on every commit, but the chunks that did not change are not",
        len_of(&path)
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

fn without(path: &std::path::Path, dropped: &[&str]) {
    let file = ColumnFile::open(path).unwrap();
    let kept: Vec<String> = file
        .columns()
        .iter()
        .map(|column| column.name.clone())
        .filter(|name| !dropped.contains(&name.as_str()))
        .collect();
    let mut updater = file.update(this_device()).unwrap();
    for name in &kept {
        updater.keep_column(name).unwrap();
    }
    for name in dropped {
        updater.remove_column(name);
    }
    updater.commit().unwrap();
}

#[test]
fn an_index_a_file_never_held_is_rebuilt_when_it_is_opened_rather_than_refused() {
    let dir = TempDir::new("store_derived");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    let alice = stock(&mut library);
    library.commit(this_device()).unwrap();

    without(
        &path,
        &[
            "book.title.buckets",
            "book.title.links",
            "book.id.buckets",
            "book.id.links",
        ],
    );

    let reopened = Library::open(&path).unwrap();
    assert_eq!(
        reopened.books.find_by_title("Alice").unwrap(),
        [alice],
        "a column an index was derived from is all an index needs"
    );
    assert_eq!(reopened.at(alice).unwrap().title().unwrap(), "Alice");
    reopened.verify().unwrap();
}

#[test]
fn an_index_rebuilt_on_one_open_is_written_by_the_next_commit() {
    let dir = TempDir::new("store_derived_written");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    stock(&mut library);
    library.commit(this_device()).unwrap();
    without(&path, &["book.title.buckets", "book.title.links"]);
    assert!(
        ColumnFile::open(&path)
            .unwrap()
            .column_meta("book.title.links")
            .is_none()
    );

    let mut reopened = Library::open(&path).unwrap();
    reopened.commit(this_device()).unwrap();

    assert!(
        ColumnFile::open(&path)
            .unwrap()
            .column_meta("book.title.links")
            .is_some(),
        "a file pays for a rebuild once, not on every open"
    );
    Library::open(&path).unwrap().verify().unwrap();
}

#[test]
fn a_column_that_holds_rows_is_still_missing_when_the_file_does_not_have_it() {
    let dir = TempDir::new("store_missing");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    stock(&mut library);
    library.commit(this_device()).unwrap();

    without(&path, &["book.title", "book.title.bytes"]);

    let err = refused(Library::open(&path));
    assert!(err.to_string().contains("missing columns"), "{err}");
    assert!(err.to_string().contains("book.title"), "{err}");
}

#[test]
fn an_index_a_file_wrote_to_another_shape_is_rebuilt_rather_than_read_wrongly() {
    let dir = TempDir::new("store_derived_width");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    let alice = stock(&mut library);
    library.commit(this_device()).unwrap();

    let file = ColumnFile::open(&path).unwrap();
    let kept: Vec<String> = file
        .columns()
        .iter()
        .map(|column| column.name.clone())
        .filter(|name| name != "book.title.links")
        .collect();
    let mut updater = file.update(this_device()).unwrap();
    for name in &kept {
        updater.keep_column(name).unwrap();
    }
    updater.remove_column("book.title.links");
    let mut narrower = Arena::<u64>::new();
    narrower.push(0).unwrap();
    updater.column("book.title.links", &narrower).unwrap();
    updater.commit().unwrap();

    assert_eq!(
        ColumnFile::open(&path)
            .unwrap()
            .column_meta("book.title.links")
            .unwrap()
            .elem_size,
        8,
        "the file now holds links of a width this schema never writes"
    );

    let reopened = Library::open(&path).unwrap();
    assert_eq!(
        reopened.books.find_by_title("Alice").unwrap(),
        [alice],
        "a key of another width is rebuilt, not decoded under the rules of this one"
    );
    reopened.verify().unwrap();
}

fn held(library: &Library, column: &str) -> Residency {
    library
        .residency()
        .into_iter()
        .find(|held| held.name == column)
        .unwrap_or_else(|| panic!("{column} is not a column this store reports"))
}

#[test]
fn a_created_store_has_not_seen_a_write_from_anyone() -> Result<()> {
    let dir = TempDir::new("store_clock_fresh");
    let path = dir.join("library.dbs");

    let library = Library::create(&path, this_device())?;

    assert!(library.clock().is_empty());
    assert_eq!(library.clock().frontier(), 0);
    assert_eq!(
        held(&library, CLOCK).chunks,
        0,
        "a clock nobody has counted in costs nothing to carry"
    );
    Ok(())
}

#[test]
fn a_count_a_store_reached_is_still_there_when_the_file_is_opened_again() -> Result<()> {
    let dir = TempDir::new("store_clock_kept");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    stock(&mut library);
    let dot = library.tick(this_device())?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;

    assert!(reopened.clock().dominates(dot), "{:?}", reopened.clock());
    assert_eq!(reopened.clock().frontier(), dot.counter());
    Ok(())
}

#[test]
fn a_count_reached_but_never_committed_is_not_there_on_the_next_open() -> Result<()> {
    let dir = TempDir::new("store_clock_uncommitted");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    let dot = library.tick(this_device())?;
    drop(library);

    let reopened = Library::open(&path)?;

    assert!(
        !reopened.clock().dominates(dot),
        "a clock is data a commit wrote, not something the file keeps on its own"
    );
    Ok(())
}

#[test]
fn a_store_remembers_every_writer_that_ever_counted_in_it() -> Result<()> {
    let dir = TempDir::new("store_clock_writers");
    let path = dir.join("library.dbs");
    let annexe = WriterId::from_device_id("the annexe");

    let mut library = Library::create(&path, this_device())?;
    let mine = library.tick(this_device())?;
    let theirs = library.tick(annexe)?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;

    assert_eq!(reopened.clock().writers(), 2);
    assert!(reopened.clock().dominates(mine));
    assert!(reopened.clock().dominates(theirs));
    Ok(())
}

#[test]
fn a_count_in_one_file_says_nothing_about_another_file() -> Result<()> {
    let dir = TempDir::new("store_clock_apart");
    let annexe = WriterId::from_device_id("the annexe");

    let here = dir.join("here.dbs");
    let mut mine = Library::create(&here, this_device())?;
    let counted_here = mine.tick(this_device())?;
    mine.commit(this_device())?;

    let there = dir.join("there.dbs");
    let mut theirs = Library::create(&there, annexe)?;
    let counted_there = theirs.tick(annexe)?;
    theirs.commit(annexe)?;

    assert!(!mine.clock().dominates(counted_there));
    assert!(!theirs.clock().dominates(counted_here));
    Ok(())
}

#[test]
fn opening_a_store_reads_its_clock_and_not_a_row_of_any_table() -> Result<()> {
    let dir = TempDir::new("store_clock_cold");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    stock(&mut library);
    library.tick(this_device())?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;

    for column in reopened.residency() {
        match column.name.as_str() {
            CLOCK => assert_eq!(
                (column.resident, column.chunks),
                (1, 1),
                "a store cannot mint a dot without knowing how far it has counted"
            ),
            _ => assert_eq!(
                column.resident, 0,
                "{} was read before anyone asked for a row",
                column.name
            ),
        }
    }
    Ok(())
}

#[test]
fn a_file_with_no_clock_column_is_refused_rather_than_handed_a_fresh_one() -> Result<()> {
    let dir = TempDir::new("store_clock_stripped");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    library.tick(this_device())?;
    library.commit(this_device())?;
    drop(library);

    without(&path, &[CLOCK]);
    let err = refused(Library::open(&path));

    assert!(err.to_string().contains("missing columns"), "{err}");
    assert!(err.to_string().contains(CLOCK), "{err}");
    Ok(())
}

#[test]
fn committing_again_does_not_lose_the_count_the_store_had_reached() -> Result<()> {
    let dir = TempDir::new("store_clock_recommit");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    let dot = library.tick(this_device())?;
    library.commit(this_device())?;

    stock(&mut library);
    library.commit(this_device())?;

    assert!(
        library.clock().dominates(dot),
        "a commit that advanced the generation reads the file back, and the clock \
         must come back with it rather than start again: {:?}",
        library.clock()
    );
    Ok(())
}
