use super::*;
use crate::error::Error;
use crate::test_utils::library::{Book, Chapter};
use proptest::prelude::*;

struct Shelf {
    books: IdColumn<Book>,
    chapters: IdColumn<Chapter>,
    named: Arena<Id<Book>>,
}

impl Shelf {
    fn new(books: usize, claims: &[Option<usize>]) -> Self {
        let mut shelf = Self {
            books: IdColumn::default(),
            chapters: IdColumn::default(),
            named: Arena::with_chunk_rows(4),
        };
        let ids: Vec<Id<Book>> = (0..books)
            .map(|_| {
                let id = Id::mint();
                shelf.books.push(id).unwrap();
                id
            })
            .collect();

        for claim in claims {
            shelf.chapters.push(Id::mint()).unwrap();
            shelf
                .named
                .push(claim.map_or(Id::NIL, |book| ids[book]))
                .unwrap();
        }
        shelf
    }

    fn book(&self, row: usize) -> Id<Book> {
        self.books.at(Row::trusted(row)).unwrap()
    }

    fn grouped(&self) -> Result<ChildIndex<Chapter>> {
        ChildIndex::build(&self.named, &self.chapters, &self.books, "chapter.book")
    }

    fn index(&self) -> ChildIndex<Chapter> {
        self.grouped().unwrap()
    }

    fn refused(&self) -> Error {
        match self.grouped() {
            Ok(_) => panic!("a reference the parent table cannot hold must not group"),
            Err(err) => err,
        }
    }

    fn under(&self, index: &ChildIndex<Chapter>, book: usize) -> Vec<usize> {
        self.rows(index.children_of(&self.books, self.book(book)))
    }

    fn rows(&self, children: &[Id<Chapter>]) -> Vec<usize> {
        children
            .iter()
            .map(|child| self.chapters.row_of(*child).unwrap().index())
            .collect()
    }
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
fn every_child_lands_under_the_parent_it_names() {
    let shelf = Shelf::new(2, &[Some(0), Some(1), Some(0), Some(1), Some(0)]);
    let index = shelf.index();

    assert_eq!(shelf.under(&index, 0), [0, 2, 4]);
    assert_eq!(shelf.under(&index, 1), [1, 3]);
}

#[test]
fn children_come_back_in_the_order_they_were_pushed() {
    let shelf = Shelf::new(3, &[Some(2), Some(2), Some(2)]);

    assert_eq!(shelf.under(&shelf.index(), 2), [0, 1, 2]);
}

#[test]
fn the_rows_a_grouping_walks_are_the_rows_its_ids_name() {
    let shelf = Shelf::new(2, &[Some(1), Some(0), Some(1)]);
    let index = shelf.index();

    let walked: Vec<usize> = index
        .child_rows_of(&shelf.books, shelf.book(1))
        .iter()
        .map(|row| row.index())
        .collect();

    assert_eq!(walked, shelf.under(&index, 1));
    assert_eq!(walked, [0, 2]);
}

#[test]
fn a_parent_with_no_children_has_an_empty_span_not_a_missing_one() {
    let shelf = Shelf::new(3, &[Some(0), Some(0)]);
    let index = shelf.index();

    assert!(shelf.under(&index, 1).is_empty());
    assert!(shelf.under(&index, 2).is_empty());
    assert!(index.child_rows_of(&shelf.books, shelf.book(1)).is_empty());
    assert_eq!(index.parent_rows(), 3);
}

#[test]
fn a_child_that_names_no_parent_at_all_is_an_orphan() {
    let shelf = Shelf::new(1, &[Some(0), None, Some(0)]);
    let index = shelf.index();

    assert_eq!(shelf.under(&index, 0), [0, 2]);
    assert_eq!(shelf.rows(index.orphans()), [1]);
}

#[test]
fn an_absent_reference_is_an_orphan_too() {
    let shelf = Shelf::new(1, &[Some(0), Some(0)]);
    let mut column = Arena::<Option<Id<Book>>>::with_chunk_rows(4);
    column.push(Some(shelf.book(0))).unwrap();
    column.push(None).unwrap();

    let index = ChildIndex::<Chapter>::build_optional(
        &column,
        &shelf.chapters,
        &shelf.books,
        "chapter.book",
    )
    .unwrap();

    assert_eq!(
        shelf.rows(index.children_of(&shelf.books, shelf.book(0))),
        [0]
    );
    assert_eq!(shelf.rows(index.orphans()), [1]);
}

#[test]
fn a_reference_to_the_last_parent_row_is_not_dangling() {
    let shelf = Shelf::new(3, &[Some(2)]);

    assert_eq!(shelf.under(&shelf.index(), 2), [0]);
}

#[test]
fn a_reference_to_a_parent_no_row_holds_is_corrupt() {
    let mut shelf = Shelf::new(3, &[Some(0)]);
    let stranger = Id::mint();
    shelf.chapters.push(Id::mint()).unwrap();
    shelf.named.push(stranger).unwrap();

    let err = shelf.refused();

    assert!(err.is_corrupt(), "{err}");
    assert!(
        err.to_string()
            .contains(&format!("chapter.book row 1 names parent {stranger}")),
        "{err}"
    );
    assert!(
        err.to_string().contains("which is not a row of book"),
        "{err}"
    );
}

#[test]
fn a_parent_the_index_predates_simply_has_no_children_yet() {
    let shelf = Shelf::new(1, &[Some(0), Some(0)]);
    let index = shelf.index();

    let mut later = IdColumn::<Book>::default();
    later.push(shelf.book(0)).unwrap();
    let fourth = Id::mint();
    later.push(fourth).unwrap();

    assert!(
        index.children_of(&later, fourth).is_empty(),
        "a build that succeeded proved no child names this parent, so the span is empty"
    );
    assert!(index.child_rows_of(&later, fourth).is_empty());
}

#[test]
fn a_grouping_is_sized_by_the_parent_table_and_never_by_a_child() {
    let shelf = Shelf::new(2, &[Some(0), Some(1), None]);

    assert_eq!(shelf.index().parent_rows(), 2);
}

proptest! {
    #[test]
    fn a_grouping_finds_exactly_what_a_linear_scan_finds(
        books in 1usize..6,
        claims in claiming(6, 20),
    ) {
        let claims: Vec<Option<usize>> =
            claims.into_iter().map(|at| at.map(|at| at % books)).collect();
        let shelf = Shelf::new(books, &claims);
        let index = shelf.index();

        for book in 0..books {
            let scanned: Vec<usize> = claims
                .iter()
                .enumerate()
                .filter(|(_, claim)| **claim == Some(book))
                .map(|(chapter, _)| chapter)
                .collect();

            prop_assert_eq!(shelf.under(&index, book), scanned);
        }
    }

    #[test]
    fn a_grouping_covers_every_child_exactly_once(
        books in 1usize..6,
        claims in claiming(6, 20),
    ) {
        let claims: Vec<Option<usize>> =
            claims.into_iter().map(|at| at.map(|at| at % books)).collect();
        let shelf = Shelf::new(books, &claims);
        let index = shelf.index();

        let mut seen: Vec<usize> = (0..books)
            .flat_map(|book| shelf.under(&index, book))
            .chain(shelf.rows(index.orphans()))
            .collect();
        seen.sort_unstable();

        prop_assert_eq!(seen, (0..claims.len()).collect::<Vec<_>>());
    }

    #[test]
    fn every_child_a_grouping_names_is_at_the_row_it_walks(
        books in 1usize..6,
        claims in claiming(6, 20),
    ) {
        let claims: Vec<Option<usize>> =
            claims.into_iter().map(|at| at.map(|at| at % books)).collect();
        let shelf = Shelf::new(books, &claims);
        let index = shelf.index();

        for book in 0..books {
            let walked: Vec<usize> = index
                .child_rows_of(&shelf.books, shelf.book(book))
                .iter()
                .map(|row| row.index())
                .collect();

            prop_assert_eq!(walked, shelf.under(&index, book));
        }
    }
}
