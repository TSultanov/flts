use super::*;
use crate::schema::IndexKey;
use crate::schema::index::home_of;
use crate::schema::{IdColumn, TextColumn, VersionColumn};
use crate::storage::Arena;
use crate::storage::{ColumnFile, ColumnFileWriter, WriterId};
use crate::test_utils::library::{Book, Books, Chapters, Paragraphs};
use crate::test_utils::{TempDir, committed, flip_byte};

#[derive(Default, Clone)]
struct Ragged {
    ids: IdColumn<Book>,
    versions: VersionColumn<Book>,
    title: TextColumn,
    year: Arena<u32>,
}

impl Table for Ragged {
    type Of = Book;

    fn ids(&self) -> &IdColumn<Book> {
        &self.ids
    }

    fn ids_mut(&mut self) -> &mut IdColumn<Book> {
        &mut self.ids
    }

    fn versions(&self) -> &VersionColumn<Book> {
        &self.versions
    }

    fn versions_mut(&mut self) -> &mut VersionColumn<Book> {
        &mut self.versions
    }

    fn fields(&self) -> Vec<Field<'_>> {
        vec![
            Field::new("title", &self.title),
            Field::new("year", &self.year),
        ]
    }

    fn fields_mut(&mut self) -> Vec<FieldMut<'_>> {
        vec![
            FieldMut::new("title", &mut self.title),
            FieldMut::new("year", &mut self.year),
        ]
    }
}

#[derive(Default, Clone)]
struct Empty {
    ids: IdColumn<Book>,
    versions: VersionColumn<Book>,
}

impl Table for Empty {
    type Of = Book;

    fn ids(&self) -> &IdColumn<Book> {
        &self.ids
    }

    fn ids_mut(&mut self) -> &mut IdColumn<Book> {
        &mut self.ids
    }

    fn versions(&self) -> &VersionColumn<Book> {
        &self.versions
    }

    fn versions_mut(&mut self) -> &mut VersionColumn<Book> {
        &mut self.versions
    }

    fn fields(&self) -> Vec<Field<'_>> {
        Vec::new()
    }

    fn fields_mut(&mut self) -> Vec<FieldMut<'_>> {
        Vec::new()
    }
}

fn stocked() -> Books {
    shelved().0
}

fn shelved() -> (Books, Vec<Id<Book>>) {
    let mut books = Books::default();
    let ids = vec![
        books.push("Kokoro", "Natsume Soseki").unwrap(),
        books.push("Alice", "Lewis Carroll").unwrap(),
        books.push("Il nome della rosa", "Umberto Eco").unwrap(),
    ];
    (books, ids)
}

#[test]
fn a_table_counts_the_rows_its_columns_agree_on() {
    let books = stocked();

    assert_eq!(Table::rows(&books), 3);
    books.check_rows_agree().unwrap();
}

#[test]
fn a_table_with_no_columns_holds_no_rows_and_agrees_with_itself() {
    let nothing = Empty::default();

    assert_eq!(Table::rows(&nothing), 0);
    nothing.check_rows_agree().unwrap();
    assert!(nothing.specs().is_empty());
}

#[test]
fn a_table_whose_columns_disagree_on_length_is_corrupt() {
    let mut ragged = Ragged::default();
    ragged.mint().unwrap();
    ragged.title.push("only a title").unwrap();

    let err = ragged.check_rows_agree().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(
        err.to_string()
            .contains("book.title holds 1 rows but book.year holds 0"),
        "{err}"
    );
}

#[test]
fn a_ragged_table_can_never_be_offered_to_a_commit() {
    let dir = TempDir::new("ragged");
    let path = dir.join("books.dbs");

    let mut ragged = Ragged::default();
    ragged.mint().unwrap();
    ragged.title.push("only a title").unwrap();

    ColumnFileWriter::create(&path, WriterId::ANONYMOUS)
        .unwrap()
        .finish()
        .unwrap();
    let file = ColumnFile::open(&path).unwrap();
    let mut updater = file.update(WriterId::ANONYMOUS).unwrap();

    let err = ragged.offer_to(&mut updater).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("holds 1 rows but"), "{err}");
}

#[test]
fn an_id_no_row_holds_names_the_entity_that_does_not_hold_it() {
    let (books, ids) = shelved();
    let stranger = Id::<Book>::mint();

    assert_eq!(books.row_of(ids[2]).unwrap().index(), 2);
    let err = books.row_of(stranger).unwrap_err();
    assert!(err.to_string().contains("book has no row with id"), "{err}");
    assert!(err.to_string().contains(&stranger.to_string()), "{err}");
}

#[test]
fn a_table_names_every_column_it_needs_from_a_file() {
    let names: Vec<String> = stocked()
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect();

    assert_eq!(
        names,
        [
            "book.id",
            "book.id.buckets",
            "book.id.links",
            "book.version",
            "book.title",
            "book.title.bytes",
            "book.title.buckets",
            "book.title.links",
            "book.author",
            "book.author.bytes",
        ]
    );
}

#[test]
fn an_edited_row_keeps_its_id_and_its_neighbours() {
    let (mut books, ids) = shelved();

    books.set_title(ids[1], "Alice in Wonderland").unwrap();

    assert_eq!(books.title(ids[1]).unwrap(), "Alice in Wonderland");
    assert_eq!(books.author(ids[1]).unwrap(), "Lewis Carroll");
    assert_eq!(books.title(ids[0]).unwrap(), "Kokoro");
    assert_eq!(
        Table::rows(&books),
        4,
        "an edit appends the row it replaces rather than writing over it"
    );
}

#[test]
fn a_table_round_trips_through_a_file() {
    let dir = TempDir::new("table_roundtrip");
    let path = dir.join("books.dbs");

    let (books, ids) = shelved();
    let file = committed(&path, |updater| books.offer_to(updater));

    let mut read = Books::default();
    read.read_from(&file).unwrap();

    assert_eq!(Table::rows(&read), 3);
    assert_eq!(read.title(ids[0]).unwrap(), "Kokoro");
    assert_eq!(read.author(ids[2]).unwrap(), "Umberto Eco");
    assert_eq!(
        Table::row_of(&read, ids[2]).unwrap().index(),
        2,
        "a row keeps the id it was given across a file"
    );
}

#[test]
fn a_table_erased_behind_any_table_still_names_its_entity() {
    let books = stocked();
    let erased: &dyn AnyTable = &books;

    assert_eq!(erased.entity(), "book");
    assert_eq!(erased.row_count(), 3);
    assert_eq!(erased.column_specs().len(), 10);
    erased.rows_agree().unwrap();
}

#[test]
fn a_ragged_table_erased_behind_any_table_still_reports_itself_ragged() {
    let mut ragged = Ragged::default();
    ragged.mint().unwrap();
    ragged.title.push("only a title").unwrap();
    let erased: &dyn AnyTable = &ragged;

    let err = erased.rows_agree().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert_eq!(erased.row_count(), 1);
}

#[test]
fn a_table_can_be_written_and_read_entirely_through_the_erased_view() {
    let dir = TempDir::new("erased_roundtrip");
    let path = dir.join("books.dbs");

    let (books, ids) = shelved();
    let written: &dyn AnyTable = &books;
    let file = committed(&path, |updater| written.offer_columns(updater));

    let mut read = Books::default();
    let target: &mut dyn AnyTable = &mut read;
    target.read_columns(&file).unwrap();

    assert_eq!(target.row_count(), 3);
    assert_eq!(read.author(ids[1]).unwrap(), "Lewis Carroll");
}

fn held_by_book(books: &Books, column: &str) -> Residency {
    Table::residency(books)
        .into_iter()
        .find(|held| held.name == column)
        .expect("the schema names this column")
}

fn held_by(chapters: &Chapters, column: &str) -> Residency {
    Table::residency(chapters)
        .into_iter()
        .find(|held| held.name == column)
        .expect("the schema names this column")
}

fn shelf_of(rows: usize) -> (Id<Book>, Chapters) {
    let mut books = Books::default();
    let alice = books.push("Alice", "Lewis Carroll").unwrap();
    let mut chapters = Chapters::default();
    for _ in 0..rows {
        chapters.push(alice, "a chapter").unwrap();
    }
    (alice, chapters)
}

#[test]
fn a_push_touches_no_column_when_another_columns_tail_cannot_be_read() {
    let dir = TempDir::new("reserve");
    let path = dir.join("books.dbs");

    let books = stocked();
    let file = committed(&path, |updater| books.offer_to(updater));
    let author_chunk = file.column_meta("book.author").unwrap().chunks[0];
    drop(file);

    flip_byte(&path, author_chunk.offset);

    let mut read = Books::default();
    read.read_from(&ColumnFile::open(&path).unwrap()).unwrap();

    let err = read.push("a fourth", "and its author").unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert_eq!(
        Table::rows(&read),
        3,
        "the push refused before any column grew, so the table is not ragged"
    );
}

#[test]
fn a_push_that_must_regrow_an_index_reads_all_of_it_before_it_mints_an_id() {
    let dir = TempDir::new("regrow");
    let path = dir.join("chapters.dbs");

    let (alice, chapters) = shelf_of(4096);
    let file = committed(&path, |updater| chapters.offer_to(updater));
    let links = file.column_meta("chapter.book.links").unwrap();
    let interior = links.chunks[0];
    assert!(
        links.chunks.len() > 1,
        "a tail-only fault would reach this chunk anyway: {links:?}"
    );
    drop(file);

    flip_byte(&path, interior.offset);

    let mut read = Chapters::default();
    read.read_from(&ColumnFile::open(&path).unwrap()).unwrap();

    let err = read.push(alice, "one chapter too many").unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert_eq!(
        Table::rows(&read),
        4096,
        "the id was never minted, so no column grew"
    );
    read.check_rows_agree()
        .expect("a refused push leaves a table its columns still agree about");
}

#[test]
fn a_push_that_need_not_regrow_an_index_reads_only_the_tail_of_it() {
    let dir = TempDir::new("no_regrow");
    let path = dir.join("chapters.dbs");

    let (_, chapters) = shelf_of(5000);
    let file = committed(&path, |updater| chapters.offer_to(updater));
    drop(file);

    let mut read = Chapters::default();
    read.read_from(&ColumnFile::open(&path).unwrap()).unwrap();
    read.reserve().unwrap();

    let held = held_by(&read, "chapter.book.links");
    assert!(held.chunks > 1, "{held:?}");
    assert_eq!(
        held.resident, 1,
        "only a push that outgrows its buckets has cause to read a link it already filed"
    );
}

#[test]
fn reserving_for_a_row_that_outgrows_an_index_reads_every_link_it_already_filed() {
    let dir = TempDir::new("reserve_reads");
    let path = dir.join("chapters.dbs");

    let (_, chapters) = shelf_of(4096);
    let file = committed(&path, |updater| chapters.offer_to(updater));
    drop(file);

    let mut read = Chapters::default();
    read.read_from(&ColumnFile::open(&path).unwrap()).unwrap();
    read.reserve().unwrap();

    for column in ["chapter.id.links", "chapter.book.links"] {
        let held = held_by(&read, column);
        assert!(held.chunks > 1, "{held:?}");
        assert_eq!(
            held.resident, held.chunks,
            "a push that rehashes every link must read every link first: {held:?}"
        );
    }
}

#[test]
fn a_table_finds_every_row_that_shares_a_value_in_the_order_they_were_pushed() {
    let mut books = Books::default();
    let alice = books.push("Alice", "Lewis Carroll").unwrap();
    let kokoro = books.push("Kokoro", "Natsume Soseki").unwrap();
    let also = books.push("Alice", "Someone Else").unwrap();

    assert_eq!(books.find_by_title("Alice").unwrap(), [alice, also]);
    assert_eq!(books.find_by_title("Kokoro").unwrap(), [kokoro]);
    assert!(books.find_by_title("Moby-Dick").unwrap().is_empty());
}

#[test]
fn a_value_a_row_was_given_after_the_fact_is_the_value_it_is_found_by() {
    let mut books = Books::default();
    let alice = books.push("Alice", "Lewis Carroll").unwrap();

    books.set_title(alice, "Alice in Wonderland").unwrap();

    assert!(books.find_by_title("Alice").unwrap().is_empty());
    assert_eq!(books.find_by_title("Alice in Wonderland").unwrap(), [alice]);
    assert_eq!(books.title(alice).unwrap(), "Alice in Wonderland");
}

#[test]
fn a_value_index_a_commit_wrote_finds_a_row_after_a_reopen_that_read_nothing() {
    let dir = TempDir::new("value_index_reopen");
    let path = dir.join("books.dbs");

    let mut books = Books::default();
    let alice = books.push("Alice", "Lewis Carroll").unwrap();
    books.push("Kokoro", "Natsume Soseki").unwrap();
    let file = committed(&path, |updater| books.offer_to(updater));
    drop(file);

    let mut read = Books::default();
    read.read_from(&ColumnFile::open(&path).unwrap()).unwrap();

    assert_eq!(read.find_by_title("Alice").unwrap(), [alice]);
    assert_eq!(
        held_by_book(&read, "book.title").resident,
        0,
        "nobody rebuilt the index, and the index read no title to answer"
    );
    read.audit().unwrap();
}

#[test]
fn a_push_reads_the_bucket_it_will_write_before_it_mints_the_row_that_names_it() {
    let dir = TempDir::new("reach_push");
    let path = dir.join("chapters.dbs");

    let mut books = Books::default();
    let shelved = books.push("Alice", "Lewis Carroll").unwrap();
    let mut chapters = Chapters::default();
    for _ in 0..8193 {
        chapters.push(shelved, "a chapter").unwrap();
    }

    let file = committed(&path, |updater| chapters.offer_to(updater));
    let buckets = file.column_meta("chapter.book.buckets").unwrap();
    assert_eq!(buckets.chunks.len(), 2);
    let near = buckets.chunks[0];
    drop(file);

    flip_byte(&path, near.offset);

    let mut read = Chapters::default();
    read.read_from(&ColumnFile::open(&path).unwrap()).unwrap();

    let elsewhere = (0..64)
        .map(|_| books.push("another", "someone").unwrap())
        .find(|book| home_of(book.raw(), 16384) < 8192)
        .expect("of sixty-four books, one files its chapters in the unreadable chunk");

    let err = read
        .push(elsewhere, "a chapter nobody can file")
        .unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert_eq!(
        Table::rows(&read),
        8193,
        "the bucket could not be read, and the id was never minted"
    );
    read.check_rows_agree()
        .expect("a refused push leaves a table its columns still agree about");
}

#[test]
fn a_chapter_finds_its_paragraphs_through_the_tables_that_hold_them() {
    let mut books = Books::default();
    let mut chapters = Chapters::default();
    let mut paragraphs = Paragraphs::default();

    let alice = books.push("Alice", "Lewis Carroll").unwrap();
    let rabbit = chapters.push(alice, "Down the Rabbit-Hole").unwrap();
    let pool = chapters.push(alice, "The Pool of Tears").unwrap();
    paragraphs
        .push(rabbit, "Alice was beginning to get very tired.")
        .unwrap();
    paragraphs.push(pool, "Curiouser and curiouser!").unwrap();
    paragraphs
        .push(rabbit, "So she was considering in her own mind.")
        .unwrap();

    assert_eq!(chapters.of_book(alice).unwrap(), [rabbit, pool]);
    assert_eq!(chapters.title(pool).unwrap(), "The Pool of Tears");
    assert_eq!(chapters.book(pool).unwrap(), alice);
    let of_rabbit = paragraphs.of_chapter(rabbit).unwrap();
    assert_eq!(of_rabbit.len(), 2);
    assert_eq!(
        paragraphs.text(of_rabbit[1]).unwrap(),
        "So she was considering in her own mind."
    );
}

fn two_books() -> (Books, Id<Book>) {
    let mut books = Books::default();
    let alice = books.push("Alice", "Lewis Carroll").unwrap();
    books.push("Kokoro", "Natsume Soseki").unwrap();
    (books, alice)
}

#[test]
fn writing_over_a_row_leaves_the_one_it_replaced_where_it_was() {
    let (mut books, alice) = two_books();

    books
        .write_over(alice, "Alice's Adventures", "Lewis Carroll")
        .unwrap();

    assert_eq!(
        Table::rows(&books),
        3,
        "a write appends rather than overwrites"
    );
    assert_eq!(books.versions_of(alice).unwrap().len(), 2);
    assert_eq!(books.title(alice).unwrap(), "Alice's Adventures");
    Table::audit(&books).unwrap();
}

#[test]
fn an_id_answers_with_its_newest_version_and_the_older_ones_stay_readable() {
    let (mut books, alice) = two_books();
    let first = books.row_of(alice).unwrap();

    books
        .write_over(alice, "Alice's Adventures", "Lewis Carroll")
        .unwrap();
    let second = books.row_of(alice).unwrap();

    assert_ne!(first, second);
    assert_eq!(
        books.versions_of(alice).unwrap(),
        vec![first, second],
        "every version of an id, oldest first"
    );
    assert_eq!(
        books.title_at(first).unwrap(),
        "Alice",
        "the row that was written over still holds what it held"
    );
}

#[test]
fn a_value_only_an_older_version_holds_no_longer_names_that_row() {
    let (mut books, alice) = two_books();

    books
        .write_over(alice, "Alice's Adventures", "Lewis Carroll")
        .unwrap();

    assert_eq!(
        books.find_by_title("Alice's Adventures").unwrap(),
        vec![alice]
    );
    assert!(
        books.find_by_title("Alice").unwrap().is_empty(),
        "a title a book used to have is not a title it has"
    );
}

#[test]
fn a_title_a_newer_version_gave_up_is_free_for_another_book_to_take() {
    let (mut books, alice) = two_books();

    books
        .write_over(alice, "Alice's Adventures", "Lewis Carroll")
        .unwrap();
    let other = books.push("Alice", "Somebody Else").unwrap();

    assert_eq!(books.find_by_title("Alice").unwrap(), vec![other]);
    Table::audit(&books).unwrap();
}

#[test]
fn a_walk_over_a_table_hands_back_the_newest_version_of_each_id_and_no_other() {
    let mut library = crate::test_utils::library::Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    library.books.push("Kokoro", "Natsume Soseki").unwrap();
    library
        .books
        .write_over(alice, "Alice's Adventures", "Lewis Carroll")
        .unwrap();

    let walked: Vec<String> = library
        .all::<Book>()
        .map(|book| book.title().unwrap().into_owned())
        .collect();

    assert_eq!(walked, ["Kokoro", "Alice's Adventures"]);
    assert_eq!(
        Table::rows(&library.books),
        3,
        "the table holds three rows and the walk hands back two"
    );
}

#[test]
fn writing_over_a_row_that_no_id_names_is_refused() {
    let (books_held, _) = two_books();
    let mut books = books_held;
    let stranger = Id::<Book>::mint();

    let err = books
        .write_over(stranger, "Nothing", "Nobody")
        .expect_err("there is no newest version of an id nobody wrote");

    assert!(err.to_string().contains("to write over"), "{err}");
    assert!(err.to_string().contains(&stranger.to_string()), "{err}");
    assert_eq!(Table::rows(&books), 2);
}

#[test]
fn a_table_with_two_rows_that_are_both_the_newest_of_one_id_is_corruption() {
    let (mut books, alice) = two_books();
    books
        .write_over(alice, "Alice's Adventures", "Lewis Carroll")
        .unwrap();
    books
        .quietly_unsupersede(Row::of(0, 3, "book").unwrap())
        .unwrap();

    let err = Table::audit(&books).expect_err("one id has one newest version");

    assert!(err.is_corrupt(), "{err}");
    assert!(
        err.to_string()
            .contains("two rows that are both the newest"),
        "{err}"
    );
}

#[test]
fn a_table_where_no_row_is_the_newest_of_an_id_is_corruption() {
    let (mut books, alice) = two_books();
    let newest = books.row_of(alice).unwrap();
    books.quietly_supersede(newest).unwrap();

    let err = Table::audit(&books).expect_err("some row has to be the one an id names");

    assert!(err.is_corrupt(), "{err}");
    assert!(
        err.to_string().contains("no row holds the newest one"),
        "{err}"
    );
}

#[test]
fn only_a_row_that_is_still_the_newest_of_its_id_holds_a_value_against_another() {
    let (mut books, alice) = two_books();
    let was = books.row_of(alice).unwrap();

    books
        .write_over(alice, "Alice's Adventures", "Lewis Carroll")
        .unwrap();

    assert_eq!(
        books.live_one(vec![was]).unwrap(),
        None,
        "a row that was written over holds nothing against anyone"
    );
    assert_eq!(
        books.live_one(books.versions_of(alice).unwrap()).unwrap(),
        Some(books.row_of(alice).unwrap()),
        "and the newest version of the id is the one that does"
    );
}

#[test]
fn a_handle_that_writes_twice_writes_over_what_it_wrote_and_not_what_it_opened_at() {
    let mut library = crate::test_utils::library::Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();

    let mut held = library.at_mut(alice).unwrap();
    held.write_over("Alice's Adventures", "Lewis Carroll")
        .unwrap();
    held.write_over("Alice Through the Looking-Glass", "Lewis Carroll")
        .unwrap();

    assert_eq!(
        library.books.title(alice).unwrap(),
        "Alice Through the Looking-Glass",
        "a handle holds an id, so the second write replaces the first and not the row it opened at"
    );
    assert_eq!(library.books.versions_of(alice).unwrap().len(), 3);
    Table::audit(&library.books).unwrap();
}

#[test]
fn a_handle_writes_to_the_row_its_id_names_now_and_not_the_one_it_opened_at() {
    let mut library = crate::test_utils::library::Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();

    let mut held = library.at_mut(alice).unwrap();
    held.write_over("Alice's Adventures", "Lewis Carroll")
        .unwrap();
    held.set_title("Alice in Wonderland").unwrap();

    assert_eq!(library.books.title(alice).unwrap(), "Alice in Wonderland");
    assert_eq!(
        library
            .books
            .title_at(Row::of(0, 2, "book").unwrap())
            .unwrap(),
        "Alice",
        "the row the handle opened at was written over, not written to"
    );
}

#[test]
fn settling_an_id_makes_its_newest_version_the_one_it_names() {
    let (mut books, alice) = two_books();
    books
        .write_over(alice, "Alice's Adventures", "Lewis Carroll")
        .unwrap();
    let newest = books.row_of(alice).unwrap();
    let older = Row::of(0, 3, "book").unwrap();

    books.quietly_supersede(newest).unwrap();
    books.quietly_unsupersede(older).unwrap();
    assert_eq!(books.title(alice).unwrap(), "Alice");

    books.settle(alice).unwrap();

    assert_eq!(
        books.title(alice).unwrap(),
        "Alice's Adventures",
        "whichever row was marked, the version that counted furthest is the one the id names"
    );
    assert_eq!(books.row_of(alice).unwrap(), newest);
    Table::audit(&books).unwrap();
}
