use proptest::prelude::*;

use super::*;
use crate::schema::nullable::NullableColumn;
use crate::test_utils::library::{Book, Chapter};
use crate::test_utils::{TempDir, committed, props};

type Wide = Sorted<Arena<i32>>;
type Within = Sorted<Arena<u32>, Id<Book>>;

fn scanned(keys: &[i32], low: i32, high: i32) -> Vec<(i32, usize)> {
    let mut hits: Vec<(i32, usize)> = keys
        .iter()
        .enumerate()
        .filter(|(_, key)| (low..=high).contains(*key))
        .map(|(row, key)| (*key, row))
        .collect();
    hits.sort_unstable();
    hits
}

fn found(sorted: &Wide, low: i32, high: i32) -> Vec<(i32, usize)> {
    let (from, to) = key_bounds(&(low..=high)).expect("a range that holds a value");
    let mut hits: Vec<(i32, usize)> = sorted
        .between(from, to)
        .unwrap()
        .into_iter()
        .map(|(key, row)| {
            let value = *sorted.column().row(row.index()).unwrap();
            assert_eq!(key, value.sort_key());
            (value, row.index())
        })
        .collect();
    hits.sort_unstable();
    hits
}

fn filled(keys: &[i32]) -> Wide {
    let mut sorted = Wide::named("card.due");
    for key in keys {
        sorted.push(*key).unwrap();
    }
    sorted
}

#[test]
fn keys_sort_the_way_the_values_they_stand_for_do() {
    let signed = [i64::MIN, -2, -1, 0, 1, i64::MAX];
    for pair in signed.windows(2) {
        assert!(pair[0].sort_key() < pair[1].sort_key(), "{pair:?}");
    }
    let unsigned = [0, 1, u64::MAX - 1, u64::MAX];
    for pair in unsigned.windows(2) {
        assert!(pair[0].sort_key() < pair[1].sort_key(), "{pair:?}");
    }
    assert!(false.sort_key() < true.sort_key());
}

#[test]
fn an_absent_value_sorts_before_every_present_one_and_never_meets_one() {
    let absent = Maybe::<u64>(None).sort_key();
    for present in [0u64, 1, u64::MAX] {
        assert!(absent < Maybe(Some(present)).sort_key());
        assert_eq!(Maybe(Some(present)).sort_key(), present.sort_key());
    }
    assert!(absent < Maybe(Some(i64::MIN)).sort_key());
}

#[test]
fn a_range_bound_by_values_never_takes_in_an_absent_one() {
    let (low, high) = key_bounds::<u64>(&(..)).unwrap();
    assert!(low > Maybe::<u64>(None).sort_key());
    assert!(high >= u64::MAX.sort_key());
    assert_eq!(key_bounds(&(3u32..3)), None);
    assert_eq!(
        key_bounds(&(u64::MAX..)),
        Some((u64::MAX.sort_key(), u64::MAX.sort_key()))
    );
    assert_eq!(key_bounds(&(..0u64)), None);
    assert_eq!(
        key_bounds(&(2u8..5)),
        Some((2u8.sort_key(), 4u8.sort_key()))
    );
}

#[test]
fn rows_pushed_since_the_last_fold_are_found_before_anything_is_folded() {
    let sorted = filled(&[5, -3, 5, 9]);

    assert!(sorted.run().is_empty());
    assert_eq!(found(&sorted, 5, 9), [(5, 0), (5, 2), (9, 3)]);
    assert_eq!(found(&sorted, i32::MIN, 0), [(-3, 1)]);
}

#[test]
fn a_fold_files_every_row_in_order_and_answers_the_same() {
    let keys = [5, -3, 5, 9, 0, -3];
    let mut sorted = filled(&keys);

    sorted.fold(true).unwrap();

    assert_eq!(sorted.run().len(), keys.len());
    let filed: Vec<u64> = (0..keys.len())
        .map(|at| sorted.run.run.row(at).unwrap().row)
        .collect();
    assert_eq!(filed, [1, 5, 4, 0, 2, 3]);
    assert_eq!(found(&sorted, -3, 5), scanned(&keys, -3, 5));
    Column::audit(&sorted).unwrap();
}

#[test]
fn a_fold_waits_until_the_tail_is_worth_sorting_unless_it_is_told_not_to() {
    let mut sorted = filled(&vec![1; FOLD_AT_LEAST - 1]);
    sorted.fold(false).unwrap();
    assert!(sorted.run().is_empty(), "a short tail is cheaper to scan");

    sorted.push(1).unwrap();
    sorted.fold(false).unwrap();
    assert_eq!(sorted.run().len(), FOLD_AT_LEAST);

    let before = sorted.run().len();
    let mut many = sorted.clone();
    for _ in 0..FOLD_AT_LEAST - 1 {
        many.push(2).unwrap();
    }
    many.fold(false).unwrap();
    assert_eq!(
        many.run().len(),
        before,
        "the tail is still under the floor"
    );
    many.fold(true).unwrap();
    assert_eq!(many.run().len(), before + FOLD_AT_LEAST - 1);
    assert!(!many.run().wants_fold(Column::rows(&many), true));
}

#[test]
fn a_run_long_enough_to_span_chunks_is_searched_rather_than_walked() {
    let keys: Vec<i32> = (0..6000).map(|n| (n * 7919) % 1000 - 500).collect();
    let mut sorted = filled(&keys);
    sorted.fold(true).unwrap();
    assert!(
        sorted.run.run.chunk_count() > 2,
        "{}",
        sorted.run.run.chunk_count()
    );

    assert_eq!(found(&sorted, 17, 17), scanned(&keys, 17, 17));
    assert_eq!(found(&sorted, -500, -490), scanned(&keys, -500, -490));
    assert_eq!(found(&sorted, 499, i32::MAX), scanned(&keys, 499, i32::MAX));
}

#[test]
fn an_audit_refuses_a_run_that_files_a_row_out_of_order() {
    let mut sorted = filled(&[1, 2, 3]);
    sorted.fold(true).unwrap();
    let mut first = *sorted.run.run.row(0).unwrap();
    let second = *sorted.run.run.row(1).unwrap();
    first.row = second.row;
    sorted.run.run.set(1, first).unwrap();
    sorted.run.run.set(0, second).unwrap();

    let err = Column::audit(&sorted).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}

#[test]
fn an_audit_refuses_a_run_that_files_a_row_twice() {
    let mut sorted = filled(&[1, 1, 3]);
    sorted.fold(true).unwrap();
    let mut twin = *sorted.run.run.row(0).unwrap();
    twin.row = sorted.run.run.row(1).unwrap().row;
    sorted.run.run.set(0, twin).unwrap();

    let err = Column::audit(&sorted).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}

#[test]
fn an_audit_refuses_a_run_that_files_a_row_under_a_key_it_does_not_hold() {
    let mut sorted = filled(&[1, 2, 3]);
    sorted.fold(true).unwrap();
    let mut wrong = *sorted.run.run.row(2).unwrap();
    wrong.key += 1;
    sorted.run.run.set(2, wrong).unwrap();

    let err = Column::audit(&sorted).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}

#[test]
fn a_fold_refuses_a_tail_that_names_a_row_the_run_already_files() {
    let mut run = SortedRun::<()>::new("card.due".to_owned());
    run.fold(vec![SortEntry {
        prefix: (),
        key: 1,
        row: 0,
    }])
    .unwrap();

    let err = run
        .fold(vec![SortEntry {
            prefix: (),
            key: 2,
            row: 0,
        }])
        .unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert_eq!(run.len(), 1, "a refused fold leaves the run it had");
}

#[test]
fn an_absent_value_is_filed_first_and_found_by_no_range_of_values() {
    let mut sorted = Sorted::<NullableColumn<u32>>::named("card.interval");
    for value in [Some(3), None, Some(0), None] {
        sorted.push(value).unwrap();
    }
    sorted.fold(true).unwrap();

    let first: Vec<u64> = (0..2)
        .map(|at| sorted.run.run.row(at).unwrap().row)
        .collect();
    assert_eq!(first, [1, 3]);
    let (low, high) = key_bounds::<u32>(&(..)).unwrap();
    let rows: Vec<usize> = sorted
        .between(low, high)
        .unwrap()
        .into_iter()
        .map(|(_, row)| row.index())
        .collect();
    assert_eq!(rows, [2, 0]);
}

#[test]
fn a_run_is_written_beside_its_column_and_read_back_without_a_chunk_of_either() {
    let dir = TempDir::new("sorted_file");
    let path = dir.join("sorted.dbs");
    let mut sorted = filled(&[4, 2, 8]);
    sorted.fold(true).unwrap();

    let file = committed(&path, |updater| {
        Column::offer_to(&sorted, "card", "due", updater)
    });
    let mut read = Wide::named("card.due");
    Column::read_from(&mut read, "card", "due", &file).unwrap();

    let mut held = Vec::new();
    Column::residency(&read, "card", "due", &mut held);
    assert!(held.iter().all(|column| column.resident == 0), "{held:?}");
    assert_eq!(read.run().len(), 3);
    assert_eq!(found(&read, 3, 8), [(4, 0), (8, 2)]);
    Column::audit(&read).unwrap();
}

#[test]
fn a_run_the_file_lacks_or_holds_at_another_width_leaves_every_row_to_the_tail() {
    let dir = TempDir::new("sorted_width");
    let path = dir.join("sorted.dbs");
    let sorted = filled(&[4, 2, 8]);
    let mut other = Arena::<u64>::new();
    other.push(1).unwrap();
    let file = committed(&path, |updater| {
        sorted.column().offer_to("card", "due", updater)?;
        updater.column(&sorted_name("card", "due"), &other)
    });

    let mut read = Wide::named("card.due");
    Column::read_from(&mut read, "card", "due", &file).unwrap();

    assert!(read.run().is_empty());
    assert_eq!(found(&read, 0, 9), [(2, 1), (4, 0), (8, 2)]);
}

#[test]
fn a_run_longer_than_its_column_is_not_trusted() {
    let dir = TempDir::new("sorted_long");
    let path = dir.join("sorted.dbs");
    let mut sorted = filled(&[4, 2, 8]);
    sorted.fold(true).unwrap();
    let shorter = filled(&[4, 2]);
    let file = committed(&path, |updater| {
        shorter.column().offer_to("card", "due", updater)?;
        sorted.run().offer_to("card", "due", updater)
    });

    let mut read = Wide::named("card.due");
    Column::read_from(&mut read, "card", "due", &file).unwrap();

    assert!(read.run().is_empty());
    assert_eq!(found(&read, 0, 9), [(2, 1), (4, 0)]);
}

struct Family {
    books: Vec<Id<Book>>,
    parents: RefColumn<Book, Chapter>,
    order: Within,
    claims: Vec<(usize, u32)>,
}

impl Family {
    fn of(books: usize) -> Self {
        Self {
            books: (0..books).map(|_| Id::mint()).collect(),
            parents: RefColumn::named("chapter.book"),
            order: Within::named("chapter.pos"),
            claims: Vec::new(),
        }
    }

    fn shelve(&mut self, book: usize, pos: u32) {
        let book = book % self.books.len();
        self.parents.push(self.books[book]).unwrap();
        self.order.push(pos).unwrap();
        self.claims.push((book, pos));
    }

    fn walked(&self, book: usize, low: u32, high: u32) -> Vec<(u32, usize)> {
        let (from, to) = key_bounds(&(low..=high)).unwrap();
        let mut hits: Vec<(u128, usize)> = self
            .order
            .children_between(self.books[book], from, to, &self.parents)
            .unwrap()
            .into_iter()
            .map(|(key, row)| (key, row.index()))
            .collect();
        hits.sort_unstable();
        hits.into_iter()
            .map(|(_, row)| (self.claims[row].1, row))
            .collect()
    }

    fn scanned(&self, book: usize, low: u32, high: u32) -> Vec<(u32, usize)> {
        let mut hits: Vec<(u32, usize)> = self
            .claims
            .iter()
            .enumerate()
            .filter(|(_, (held, pos))| *held == book && (low..=high).contains(pos))
            .map(|(row, (_, pos))| (*pos, row))
            .collect();
        hits.sort_unstable();
        hits
    }
}

#[test]
fn children_come_back_by_their_position_under_the_parent_they_name() {
    let mut family = Family::of(2);
    for (book, pos) in [(0, 30), (1, 5), (0, 10), (0, 20), (1, 1)] {
        family.shelve(book, pos);
    }

    assert_eq!(family.walked(0, 0, u32::MAX), [(10, 2), (20, 3), (30, 0)]);
    family.order.fold_within(&family.parents, true).unwrap();
    assert_eq!(family.walked(0, 0, u32::MAX), [(10, 2), (20, 3), (30, 0)]);
    assert_eq!(family.walked(1, 0, u32::MAX), [(1, 4), (5, 1)]);
    assert_eq!(family.walked(0, 15, 30), [(20, 3), (30, 0)]);
    family.order.audit_within(&family.parents).unwrap();
}

#[test]
fn an_audit_within_refuses_a_row_filed_under_a_parent_it_does_not_name() {
    let mut family = Family::of(2);
    for (book, pos) in [(0, 1), (1, 2)] {
        family.shelve(book, pos);
    }
    family.order.fold_within(&family.parents, true).unwrap();
    let mut moved = *family.order.run.run.row(0).unwrap();
    moved.prefix = Id::NIL;
    family.order.run.run.set(0, moved).unwrap();

    Column::audit(&family.order).unwrap();
    let err = family.order.audit_within(&family.parents).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}

#[derive(Clone, Debug)]
enum Move {
    Push(i32),
    Fold(bool),
}

fn moves() -> impl Strategy<Value = Vec<Move>> {
    let key = prop_oneof![3 => -4i32..4, 1 => any::<i32>()];
    prop::collection::vec(
        prop_oneof![
            6 => key.prop_map(Move::Push),
            1 => any::<bool>().prop_map(Move::Fold),
        ],
        0..80,
    )
}

#[derive(Clone, Debug)]
enum Deed {
    Shelve(usize, u32),
    Fold(bool),
}

fn deeds() -> impl Strategy<Value = Vec<Deed>> {
    let pos = prop_oneof![3 => 0u32..4, 1 => any::<u32>()];
    prop::collection::vec(
        prop_oneof![
            6 => (0usize..3, pos).prop_map(|(book, pos)| Deed::Shelve(book, pos)),
            1 => any::<bool>().prop_map(Deed::Fold),
        ],
        0..80,
    )
}

proptest! {
    #![proptest_config(props::scaled(2))]

    #[test]
    fn every_range_finds_exactly_the_rows_a_scan_of_the_column_would(
        moves in moves(),
        low in prop_oneof![3 => -5i32..5, 1 => any::<i32>()],
        span in prop_oneof![3 => 0i32..6, 1 => any::<i32>()],
    ) {
        let high = low.saturating_add(span.abs());
        let mut sorted = Wide::named("card.due");
        let mut keys = Vec::new();
        for step in &moves {
            match step {
                Move::Push(key) => {
                    sorted.push(*key).unwrap();
                    keys.push(*key);
                }
                Move::Fold(force) => sorted.fold(*force).unwrap(),
            }
            Column::audit(&sorted).unwrap();
            prop_assert_eq!(found(&sorted, low, high), scanned(&keys, low, high));
        }
        sorted.fold(true).unwrap();
        prop_assert_eq!(sorted.run().len(), keys.len());
        prop_assert_eq!(found(&sorted, low, high), scanned(&keys, low, high));
    }

    #[test]
    fn every_parent_walks_exactly_the_children_a_scan_of_the_column_would(
        deeds in deeds(),
        low in prop_oneof![3 => 0u32..5, 1 => any::<u32>()],
        span in prop_oneof![3 => 0u32..6, 1 => any::<u32>()],
    ) {
        let high = low.saturating_add(span);
        let mut family = Family::of(3);
        for deed in &deeds {
            match deed {
                Deed::Shelve(book, pos) => family.shelve(*book, *pos),
                Deed::Fold(force) => family.order.fold_within(&family.parents, *force).unwrap(),
            }
            family.order.audit_within(&family.parents).unwrap();
            for book in 0..3 {
                prop_assert_eq!(family.walked(book, low, high), family.scanned(book, low, high));
            }
        }
    }
}
