use super::*;
use crate::error::Error;
use crate::storage::Arena;
use crate::test_utils::props;
use proptest::prelude::*;

struct Book;
struct Chapter;

fn refs(parents: &[u64]) -> Arena<Id<Book>> {
    let mut column = Arena::<Id<Book>>::with_chunk_rows(4);
    for parent in parents {
        column.push(Id::from_raw(*parent)).unwrap();
    }
    column
}

fn owned(parents: &[u64], parent_rows: usize) -> ChildIndex<Chapter> {
    ChildIndex::build(&refs(parents), parent_rows, "chapter.book").unwrap()
}

fn rows(children: &[Id<Chapter>]) -> Vec<usize> {
    children.iter().map(|child| child.index()).collect()
}

fn refuses(parents: &[u64], parent_rows: usize) -> Error {
    match ChildIndex::<Chapter>::build(&refs(parents), parent_rows, "chapter.book") {
        Ok(_) => panic!("a reference the parent table cannot hold must not group"),
        Err(err) => err,
    }
}

#[test]
fn every_child_lands_under_the_parent_it_names() {
    let index = owned(&[0, 1, 0, 1, 0], 2);

    assert_eq!(
        rows(index.children_of(Id::<Book>::from_index(0))),
        [0, 2, 4]
    );
    assert_eq!(rows(index.children_of(Id::<Book>::from_index(1))), [1, 3]);
    assert!(index.orphans().is_empty());
}

#[test]
fn children_come_back_in_the_order_they_were_pushed() {
    let index = owned(&[2, 2, 2], 3);

    assert_eq!(
        rows(index.children_of(Id::<Book>::from_index(2))),
        [0, 1, 2]
    );
}

#[test]
fn a_parent_with_no_children_has_an_empty_span_not_a_missing_one() {
    let index = owned(&[0, 0], 3);

    assert!(index.children_of(Id::<Book>::from_index(1)).is_empty());
    assert!(index.children_of(Id::<Book>::from_index(2)).is_empty());
    assert_eq!(index.parent_rows(), 3);
}

#[test]
fn a_null_reference_is_an_orphan_not_a_child() {
    let index = owned(&[0, Id::<Book>::NULL, 0], 1);

    assert_eq!(rows(index.children_of(Id::<Book>::from_index(0))), [0, 2]);
    assert_eq!(rows(index.orphans()), [1]);
}

#[test]
fn an_absent_reference_is_an_orphan_too() {
    let mut column = Arena::<Option<Id<Book>>>::with_chunk_rows(4);
    column.push(Some(Id::from_index(0))).unwrap();
    column.push(None).unwrap();

    let index = ChildIndex::<Chapter>::build_optional(&column, 1, "chapter.book").unwrap();

    assert_eq!(rows(index.children_of(Id::<Book>::from_index(0))), [0]);
    assert_eq!(rows(index.orphans()), [1]);
}

#[test]
fn a_reference_to_the_last_parent_row_is_not_dangling() {
    let index = owned(&[2], 3);

    assert_eq!(rows(index.children_of(Id::<Book>::from_index(2))), [0]);
}

#[test]
fn a_reference_past_the_last_parent_row_is_corrupt() {
    let err = refuses(&[0, 3], 3);

    assert!(err.is_corrupt(), "{err}");
    assert!(
        err.to_string()
            .contains("chapter.book row 1 names parent row 3"),
        "{err}"
    );
    assert!(
        err.to_string().contains("past the 3 rows there are"),
        "{err}"
    );
}

#[test]
fn a_wild_reference_allocates_nothing_for_the_parent_it_claims() {
    let err = refuses(&[u64::MAX - 1], 1);

    assert!(
        err.is_corrupt(),
        "the index is sized by the parent table, never by what a child claims: {err}"
    );
}

#[test]
fn a_parent_the_index_predates_simply_has_no_children_yet() {
    let index = owned(&[0, 0], 1);

    assert!(
        index.children_of(Id::<Book>::from_index(7)).is_empty(),
        "a build that succeeded proved no child names a row this high, so the span is empty"
    );
}

fn parentage(parents: usize, children: usize) -> impl Strategy<Value = Vec<u64>> {
    prop::collection::vec(
        prop_oneof![
            9 => 0..parents as u64,
            1 => Just(Id::<Book>::NULL),
        ],
        0..children,
    )
}

proptest! {
    #[test]
    fn a_grouping_finds_exactly_what_a_linear_scan_finds(
        parent_rows in 1usize..6,
        refs_in in parentage(6, 20),
    ) {
        let refs_in: Vec<u64> = refs_in.into_iter().map(|at| at % parent_rows as u64).collect();
        let index = owned(&refs_in, parent_rows);

        for parent in 0..parent_rows {
            let scanned: Vec<usize> = refs_in
                .iter()
                .enumerate()
                .filter(|(_, owner)| **owner == parent as u64)
                .map(|(child, _)| child)
                .collect();

            prop_assert_eq!(rows(index.children_of(Id::<Book>::from_index(parent))), scanned);
        }
    }

    #[test]
    fn a_grouping_covers_every_child_exactly_once(
        parent_rows in 1usize..6,
        refs_in in parentage(6, 20),
    ) {
        let refs_in: Vec<u64> = refs_in
            .into_iter()
            .map(|at| if at == Id::<Book>::NULL { at } else { at % parent_rows as u64 })
            .collect();
        let index = owned(&refs_in, parent_rows);

        let mut seen: Vec<usize> = (0..parent_rows)
            .flat_map(|parent| rows(index.children_of(Id::<Book>::from_index(parent))))
            .chain(rows(index.orphans()))
            .collect();
        seen.sort_unstable();

        prop_assert_eq!(seen, (0..refs_in.len()).collect::<Vec<_>>());
    }

    #[test]
    fn a_grouping_never_sizes_itself_by_what_a_child_claims(
        claim in props::interesting_u64(),
    ) {
        let parent_rows = 2;
        match ChildIndex::<Chapter>::build(&refs(&[claim]), parent_rows, "chapter.book") {
            Ok(index) => prop_assert!(index.parent_rows() == parent_rows),
            Err(err) => prop_assert!(err.is_corrupt(), "{}", err),
        }
    }
}
