use super::*;
use crate::schema::{IdColumn, TextColumn};
use crate::storage::Arena;
use crate::storage::{ColumnFile, ColumnFileWriter, WriterId};
use crate::test_utils::library::{Book, Books, Chapters, Paragraphs};
use crate::test_utils::{TempDir, committed, flip_byte};

#[derive(Default, Clone)]
struct Ragged {
    ids: IdColumn<Book>,
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
}

impl Table for Empty {
    type Of = Book;

    fn ids(&self) -> &IdColumn<Book> {
        &self.ids
    }

    fn ids_mut(&mut self) -> &mut IdColumn<Book> {
        &mut self.ids
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
            "book.title",
            "book.title.bytes",
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
    assert_eq!(Table::rows(&books), 3);
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
    assert_eq!(erased.column_specs().len(), 7);
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
