use proptest::prelude::*;

use super::*;
use crate::storage::ColumnFile;
use crate::test_utils::library::{Book, Chapter};
use crate::test_utils::{TempDir, committed, flip_byte};

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

    fn under(&self, book: usize) -> Vec<usize> {
        self.rows(
            &self
                .column
                .children_of(&self.chapters, self.book(book))
                .unwrap(),
        )
    }

    fn walked(&self, book: usize) -> Vec<usize> {
        self.column
            .child_rows_of(self.book(book))
            .unwrap()
            .iter()
            .map(|row| row.index())
            .collect()
    }

    fn loose(&self) -> Vec<usize> {
        self.rows(&self.column.orphans(&self.chapters).unwrap())
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

fn claiming(books: usize, chapters: usize) -> impl Strategy<Value = Vec<Option<usize>>> {
    prop::collection::vec(
        prop_oneof![
            9 => (0..books).prop_map(Some),
            1 => Just(None),
        ],
        0..chapters,
    )
}

#[test]
fn a_reference_column_reads_back_the_parent_it_was_given() {
    let shelf = Shelf::of(2, &[Some(1), Some(0)]);

    assert_eq!(shelf.column.rows(), 2);
    assert_eq!(shelf.column.get(row(0, 2)).unwrap(), shelf.book(1));
    assert_eq!(shelf.column.get(row(1, 2)).unwrap(), shelf.book(0));
}

#[test]
fn every_child_lands_under_the_parent_it_names() {
    let shelf = Shelf::of(2, &[Some(0), Some(1), Some(0), Some(1), Some(0)]);

    assert_eq!(shelf.under(0), [0, 2, 4]);
    assert_eq!(shelf.under(1), [1, 3]);
}

#[test]
fn children_come_back_in_the_order_they_were_pushed() {
    let shelf = Shelf::of(1, &[Some(0), Some(0), Some(0), Some(0)]);

    assert_eq!(
        shelf.under(0),
        [0, 1, 2, 3],
        "a chain is walked newest first, and row order is what a caller was promised"
    );
}

#[test]
fn the_rows_a_column_walks_are_the_rows_the_ids_it_hands_back_name() {
    let shelf = Shelf::of(2, &[Some(1), Some(0), Some(1), None]);

    for book in 0..2 {
        assert_eq!(shelf.walked(book), shelf.under(book));
    }
}

#[test]
fn a_parent_with_no_children_gets_an_empty_answer_and_not_a_missing_one() {
    let shelf = Shelf::of(2, &[Some(0)]);

    assert!(shelf.under(1).is_empty());
    assert!(shelf.walked(1).is_empty());
}

#[test]
fn a_child_that_names_no_parent_at_all_is_an_orphan() {
    let shelf = Shelf::of(1, &[Some(0), None, None]);

    assert_eq!(shelf.under(0), [0]);
    assert_eq!(shelf.loose(), [1, 2]);
}

#[test]
fn a_child_pushed_after_a_question_is_found_by_the_next_one() {
    let mut shelf = Shelf::of(1, &[Some(0)]);
    assert_eq!(shelf.under(0), [0]);

    shelf.shelve(Some(0));

    assert_eq!(shelf.under(0), [0, 1]);
}

#[test]
fn repointing_a_child_moves_it_to_the_parent_it_was_given() {
    let mut shelf = Shelf::of(2, &[Some(0), Some(0), Some(0)]);

    let second = shelf.book(1);
    shelf.column.set(row(1, 3), second).unwrap();

    assert_eq!(shelf.under(0), [0, 2]);
    assert_eq!(shelf.under(1), [1]);
}

#[test]
fn a_child_moved_away_and_back_is_named_once_and_not_twice() {
    let mut shelf = Shelf::of(2, &[Some(0), Some(0)]);
    let first = shelf.book(0);
    let second = shelf.book(1);

    shelf.column.set(row(1, 2), second).unwrap();
    shelf.column.set(row(1, 2), first).unwrap();

    assert_eq!(shelf.under(0), [0, 1]);
    assert!(shelf.under(1).is_empty());
}

#[test]
fn a_child_repointed_at_the_parent_it_already_had_stays_where_it_is() {
    let mut shelf = Shelf::of(1, &[Some(0), Some(0)]);
    let first = shelf.book(0);

    shelf.column.set(row(1, 2), first).unwrap();

    assert_eq!(shelf.under(0), [0, 1]);
}

#[test]
fn a_clone_answers_every_question_the_column_it_came_from_answers() {
    let shelf = Shelf::of(1, &[Some(0), Some(0)]);

    let snapshot = shelf.column.clone();

    assert_eq!(
        shelf.rows(
            &snapshot
                .children_of(&shelf.chapters, shelf.book(0))
                .unwrap()
        ),
        [0, 1]
    );
}

#[test]
fn a_reference_column_describes_the_parents_and_the_index_that_groups_them() {
    let shelf = Shelf::of(1, &[Some(0)]);
    let mut specs = Vec::new();
    shelf.column.describe("chapter", "book", &mut specs);
    let mut held = Vec::new();
    shelf.column.residency("chapter", "book", &mut held);

    assert_eq!(specs.len(), 3);
    assert_eq!(specs[0].name, "chapter.book");
    assert_eq!(specs[0].elem_size, 16);
    assert_eq!(specs[0].kind, ColumnKind::Ref { parent: "book" });
    assert_eq!(specs[1].name, "chapter.book.buckets");
    assert_eq!(specs[2].name, "chapter.book.links");

    let named: Vec<&String> = specs.iter().map(|spec| &spec.name).collect();
    let reported: Vec<&String> = held.iter().map(|entry| &entry.name).collect();
    assert_eq!(named, reported);
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
    assert_eq!(shelf.under(1), [0, 2]);
    shelf.column.audit().unwrap();
}

#[test]
fn reading_a_reference_column_beside_an_index_that_lost_rows_rebuilds_it() {
    let dir = TempDir::new("ref_stale");
    let path = dir.join("chapters.dbs");

    let shelf = Shelf::of(2, &[Some(1), Some(0), Some(1)]);
    let mut behind = HashIndex::new(qualified("chapter", "book"));
    behind.append(shelf.book(1)).unwrap();
    let file = committed(&path, |updater| {
        updater.column("chapter.book", &shelf.column.ids)?;
        behind.offer_to("chapter", "book", updater)
    });

    let mut read = RefColumn::<Book, Chapter>::default();
    read.read_from("chapter", "book", &file).unwrap();

    assert_eq!(
        read.child_rows_of(shelf.book(1))
            .unwrap()
            .iter()
            .map(|row| row.index())
            .collect::<Vec<_>>(),
        [0, 2]
    );
    read.audit().unwrap();
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
    flip_byte(&path, tail.offset);

    let mut read = RefColumn::<Book, Chapter>::default();
    read.read_from("chapter", "book", &ColumnFile::open(&path).unwrap())
        .unwrap();

    let err = read.touch_tail().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}

#[test]
fn touching_the_tail_of_a_rotten_reference_index_reports_the_damage_too() {
    let dir = TempDir::new("ref_index_tail");
    let path = dir.join("chapters.dbs");

    let shelf = Shelf::of(1, &[Some(0)]);
    let file = committed(&path, |updater| {
        shelf.column.offer_to("chapter", "book", updater)
    });
    let tail = file.column_meta("chapter.book.links").unwrap().chunks[0];
    drop(file);
    flip_byte(&path, tail.offset);

    let mut read = RefColumn::<Book, Chapter>::default();
    read.read_from("chapter", "book", &ColumnFile::open(&path).unwrap())
        .unwrap();

    let err = read.touch_tail().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}

#[test]
fn a_child_naming_a_parent_the_table_does_not_hold_is_under_no_parent_it_holds() {
    let mut shelf = Shelf::of(1, &[Some(0)]);
    let stranger = Id::<Book>::mint();
    shelf.chapters.push(Id::mint()).unwrap();
    shelf.column.push(stranger).unwrap();

    assert_eq!(shelf.under(0), [0]);
    assert!(
        shelf.loose().is_empty(),
        "it names a parent, so it is not an orphan either, and no question the table \
         can ask will reach it"
    );
    assert_eq!(
        shelf.rows(&shelf.column.children_of(&shelf.chapters, stranger).unwrap()),
        [1],
        "only an id nobody can walk to finds it"
    );
}

#[test]
fn an_audit_refuses_a_reference_that_names_no_parent_of_the_table() {
    let mut shelf = Shelf::of(1, &[Some(0)]);
    let stranger = Id::<Book>::mint();
    shelf.chapters.push(Id::mint()).unwrap();
    shelf.column.push(stranger).unwrap();

    let err = shelf.column.check_parents(&shelf.books).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("chapter.book row 1"), "{err}");
    assert!(err.to_string().contains(&stranger.to_string()), "{err}");
    assert!(
        err.to_string().contains("which is not a row of book"),
        "{err}"
    );
}

#[test]
fn an_audit_refuses_an_index_that_files_children_under_parents_the_column_never_named() {
    let dir = TempDir::new("ref_audit");
    let path = dir.join("chapters.dbs");

    let shelf = Shelf::of(2, &[Some(0), Some(1), Some(0)]);
    let mut foreign = HashIndex::new(qualified("chapter", "book"));
    for _ in 0..3 {
        foreign.append(Id::<Book>::mint()).unwrap();
    }
    let file = committed(&path, |updater| {
        updater.column("chapter.book", &shelf.column.ids)?;
        foreign.offer_to("chapter", "book", updater)
    });

    let mut read = RefColumn::<Book, Chapter>::default();
    read.read_from("chapter", "book", &file).unwrap();

    let err = read.audit().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("chapter.book"), "{err}");
}

#[test]
fn an_audit_passes_a_column_whose_parents_the_table_all_hold() {
    let shelf = Shelf::of(2, &[Some(0), None, Some(1)]);

    shelf.column.check_parents(&shelf.books).unwrap();
    shelf.column.audit().unwrap();
}

proptest! {
    #[test]
    fn a_column_finds_exactly_what_a_linear_scan_finds(
        books in 1usize..6,
        claims in claiming(6, 20),
    ) {
        let claims: Vec<Option<usize>> =
            claims.into_iter().map(|at| at.map(|at| at % books)).collect();
        let shelf = Shelf::of(books, &claims);

        for book in 0..books {
            let scanned: Vec<usize> = claims
                .iter()
                .enumerate()
                .filter(|(_, claim)| **claim == Some(book))
                .map(|(chapter, _)| chapter)
                .collect();

            prop_assert_eq!(shelf.under(book), scanned);
        }
    }

    #[test]
    fn a_column_covers_every_child_exactly_once(
        books in 1usize..6,
        claims in claiming(6, 20),
    ) {
        let claims: Vec<Option<usize>> =
            claims.into_iter().map(|at| at.map(|at| at % books)).collect();
        let shelf = Shelf::of(books, &claims);

        let mut seen: Vec<usize> = (0..books)
            .flat_map(|book| shelf.under(book))
            .chain(shelf.loose())
            .collect();
        seen.sort_unstable();

        prop_assert_eq!(seen, (0..claims.len()).collect::<Vec<_>>());
    }

    #[test]
    fn every_child_a_column_names_is_at_the_row_it_walks(
        books in 1usize..6,
        claims in claiming(6, 20),
    ) {
        let claims: Vec<Option<usize>> =
            claims.into_iter().map(|at| at.map(|at| at % books)).collect();
        let shelf = Shelf::of(books, &claims);

        for book in 0..books {
            prop_assert_eq!(shelf.walked(book), shelf.under(book));
        }
    }
}
