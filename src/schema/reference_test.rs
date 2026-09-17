use super::*;
use crate::id::Id;
use crate::storage::Arena;
use crate::storage::ColumnFile;
use crate::test_utils::library::{Book, Chapter};
use crate::test_utils::{TempDir, committed};

struct Shelf {
    books: IdColumn<Book>,
    chapters: IdColumn<Chapter>,
    column: RefColumn<Book, Chapter>,
}

impl Shelf {
    fn of(books: usize, claims: &[Option<usize>]) -> Self {
        let mut shelf = Self {
            books: IdColumn::default(),
            chapters: IdColumn::default(),
            column: RefColumn::default(),
        };
        for _ in 0..books {
            shelf.books.push(Id::mint()).unwrap();
        }
        for claim in claims {
            shelf.shelve(*claim);
        }
        shelf
    }

    fn shelve(&mut self, claim: Option<usize>) -> Id<Chapter> {
        let chapter = Id::mint();
        self.chapters.push(chapter).unwrap();
        self.column
            .push(claim.map_or(Id::NIL, |book| self.book(book)))
            .unwrap();
        chapter
    }

    fn book(&self, row: usize) -> Id<Book> {
        self.books.at(Row::trusted(row)).unwrap()
    }

    fn under(&self, book: usize) -> Result<Vec<usize>> {
        let children = self
            .column
            .children_of(&self.chapters, &self.books, self.book(book))?;
        Ok(self.rows(children))
    }

    fn rows(&self, children: &[Id<Chapter>]) -> Vec<usize> {
        children
            .iter()
            .map(|child| self.chapters.row_of(*child).unwrap().index())
            .collect()
    }
}

fn row(index: usize, rows: usize) -> Row {
    Row::of(index, rows, "chapter").unwrap()
}

#[test]
fn a_reference_column_reads_back_the_parent_it_was_given() {
    let shelf = Shelf::of(2, &[Some(1), Some(0)]);

    assert_eq!(shelf.column.rows(), 2);
    assert_eq!(shelf.column.get(row(0, 2)).unwrap(), shelf.book(1));
    assert_eq!(shelf.column.get(row(1, 2)).unwrap(), shelf.book(0));
}

#[test]
fn the_first_question_builds_the_index_and_the_second_reuses_it() {
    let shelf = Shelf::of(2, &[Some(0), Some(1), Some(0)]);
    assert!(
        !shelf.column.is_indexed(),
        "nothing is grouped until someone asks"
    );

    assert_eq!(shelf.under(0).unwrap(), [0, 2]);
    assert!(shelf.column.is_indexed());

    assert_eq!(shelf.under(1).unwrap(), [1]);
    assert!(
        shelf.column.is_indexed(),
        "the second question built nothing"
    );
}

#[test]
fn pushing_a_child_drops_the_index() {
    let mut shelf = Shelf::of(1, &[Some(0)]);
    shelf.under(0).unwrap();
    assert!(shelf.column.is_indexed());

    shelf.shelve(Some(0));

    assert!(
        !shelf.column.is_indexed(),
        "a new child cannot be missed by a stale grouping"
    );
    assert_eq!(shelf.under(0).unwrap(), [0, 1]);
}

#[test]
fn repointing_a_child_at_another_parent_drops_the_index() {
    let mut shelf = Shelf::of(2, &[Some(0), Some(0)]);
    shelf.under(0).unwrap();
    assert!(shelf.column.is_indexed());

    let second = shelf.book(1);
    shelf.column.set(row(1, 2), second).unwrap();

    assert!(!shelf.column.is_indexed());
    assert_eq!(shelf.under(0).unwrap(), [0]);
    assert_eq!(shelf.under(1).unwrap(), [1]);
}

#[test]
fn reading_a_reference_column_from_a_file_drops_the_index() {
    let dir = TempDir::new("ref_read");
    let path = dir.join("chapters.dbs");

    let mut shelf = Shelf::of(1, &[Some(0), Some(0)]);
    shelf.under(0).unwrap();
    assert!(shelf.column.is_indexed());

    let file = committed(&path, |updater| {
        shelf.column.offer_to("chapter", "book", updater)
    });
    shelf.column.read_from("chapter", "book", &file).unwrap();

    assert!(
        !shelf.column.is_indexed(),
        "the rows were replaced wholesale, so the grouping of the old ones is worthless"
    );
    assert_eq!(shelf.column.rows(), 2);
}

#[test]
fn a_clone_inherits_the_grouping_it_was_cloned_from() {
    let shelf = Shelf::of(1, &[Some(0), Some(0)]);
    shelf.under(0).unwrap();

    let snapshot = shelf.column.clone();

    assert!(
        snapshot.is_indexed(),
        "a frozen view inherits warmth, as a cloned arena inherits resident chunks"
    );
    assert_eq!(
        shelf.rows(
            snapshot
                .children_of(&shelf.chapters, &shelf.books, shelf.book(0))
                .unwrap()
        ),
        [0, 1]
    );
}

#[test]
fn a_grouping_built_before_a_parent_arrived_still_answers_for_it() {
    let shelf = Shelf::of(1, &[Some(0), Some(0)]);
    assert_eq!(shelf.under(0).unwrap(), [0, 1]);

    let mut later = IdColumn::<Book>::default();
    later.push(shelf.book(0)).unwrap();
    let fourth = Id::mint();
    later.push(fourth).unwrap();

    assert!(
        shelf
            .column
            .children_of(&shelf.chapters, &later, fourth)
            .unwrap()
            .is_empty(),
        "a parent pushed after the build has no children, and the stale index cannot lie about that"
    );
}

#[test]
fn a_reference_column_describes_itself_as_pointing_at_its_parent() {
    let mut specs = Vec::new();
    Shelf::of(0, &[])
        .column
        .describe("chapter", "book", &mut specs);

    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "chapter.book");
    assert_eq!(specs[0].elem_size, 16);
    assert_eq!(specs[0].kind, ColumnKind::Ref { parent: "book" });
}

#[test]
fn a_reference_column_survives_a_commit_and_a_reopen() {
    let dir = TempDir::new("ref_roundtrip");
    let path = dir.join("chapters.dbs");

    let mut shelf = Shelf::of(2, &[Some(1), Some(0), Some(1)]);
    let file = committed(&path, |updater| {
        shelf.column.offer_to("chapter", "book", updater)
    });

    let mut read = RefColumn::<Book, Chapter>::default();
    read.read_from("chapter", "book", &file).unwrap();
    shelf.column = read;

    assert_eq!(shelf.column.rows(), 3);
    assert_eq!(shelf.under(1).unwrap(), [0, 2]);
}

#[test]
fn a_file_whose_reference_names_no_parent_of_the_table_is_corrupt() {
    let dir = TempDir::new("dangling");
    let path = dir.join("chapters.dbs");

    let shelf = Shelf::of(1, &[Some(0)]);
    let stranger = Id::<Book>::mint();
    let mut ids = Arena::<Id<Book>>::with_chunk_rows(4);
    ids.push(stranger).unwrap();
    let file = committed(&path, |updater| updater.column("chapter.book", &ids));

    let mut read = RefColumn::<Book, Chapter>::default();
    read.read_from("chapter", "book", &file).unwrap();

    let err = read
        .children_of(&shelf.chapters, &shelf.books, shelf.book(0))
        .unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("chapter.book row 0"), "{err}");
    assert!(err.to_string().contains(&stranger.to_string()), "{err}");
}

#[test]
fn a_failed_grouping_is_not_remembered() {
    let mut shelf = Shelf::of(0, &[]);
    let coming = Id::<Book>::mint();
    shelf.chapters.push(Id::mint()).unwrap();
    shelf.column.push(coming).unwrap();

    assert!(
        shelf
            .column
            .children_of(&shelf.chapters, &shelf.books, coming)
            .is_err()
    );
    assert!(
        !shelf.column.is_indexed(),
        "the parent table may simply not have grown yet; the answer must be asked again"
    );

    shelf.books.push(coming).unwrap();
    assert_eq!(shelf.under(0).unwrap(), [0]);
    shelf.shelve(Some(0));
    assert!(!shelf.column.is_indexed());
}

#[test]
fn orphans_are_listed_apart_from_every_parent() {
    let shelf = Shelf::of(1, &[Some(0), None]);

    assert_eq!(shelf.under(0).unwrap(), [0]);
    assert_eq!(
        shelf.rows(shelf.column.orphans(&shelf.chapters, &shelf.books).unwrap()),
        [1]
    );
}

#[test]
fn touching_the_tail_of_a_rotten_reference_column_reports_the_damage() {
    let dir = TempDir::new("ref_tail");
    let path = dir.join("chapters.dbs");

    let shelf = Shelf::of(1, &[Some(0)]);
    let file = committed(&path, |updater| {
        shelf.column.offer_to("chapter", "book", updater)
    });
    let tail = file.column_meta("chapter.book").unwrap().chunks[0];
    drop(file);
    crate::test_utils::flip_byte(&path, tail.offset);

    let mut read = RefColumn::<Book, Chapter>::default();
    read.read_from("chapter", "book", &ColumnFile::open(&path).unwrap())
        .unwrap();

    let err = read.touch_tail().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}
