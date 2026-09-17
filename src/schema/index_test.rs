use proptest::prelude::*;

use super::*;
use crate::id::Uuid;
use crate::storage::ColumnFile;
use crate::test_utils::library::Book;
use crate::test_utils::{TempDir, committed, flip_byte, props};

const _: () = assert!(<Link<Id<Book>> as Record>::SIZE == 24);
const _: () = assert!(capacity_for(0) == 0);
const _: () = assert!(capacity_for(1) == MIN_CAPACITY);
const _: () = assert!(capacity_for(MIN_CAPACITY) == MIN_CAPACITY);
const _: () = assert!(capacity_for(MIN_CAPACITY + 1) == 2 * MIN_CAPACITY);

fn empty() -> HashIndex<Id<Book>> {
    HashIndex::new("book.id".to_owned())
}

struct Shelf {
    keys: Arena<Id<Book>>,
    index: HashIndex<Id<Book>>,
    minted: Vec<Id<Book>>,
}

fn stocked(rows: usize) -> Shelf {
    let mut keys = Arena::new();
    let mut index = empty();
    let mut minted = Vec::new();

    for _ in 0..rows {
        let id = Id::mint();
        let row = keys.push(id).unwrap();
        assert_eq!(index.append(id).unwrap(), row);
        minted.push(id);
    }
    Shelf {
        keys,
        index,
        minted,
    }
}

fn shared(rows: usize, key: Id<Book>) -> HashIndex<Id<Book>> {
    let mut index = empty();
    for _ in 0..rows {
        index.append(key).unwrap();
    }
    index
}

#[test]
fn a_capacity_is_the_smallest_power_of_two_that_holds_every_row() {
    for (rows, capacity) in [
        (0, 0),
        (1, 8),
        (7, 8),
        (8, 8),
        (9, 16),
        (16, 16),
        (17, 32),
        (1000, 1024),
        (1024, 1024),
        (1025, 2048),
    ] {
        assert_eq!(capacity_for(rows), capacity, "{rows} rows");
    }
}

#[test]
fn a_key_names_the_same_bucket_the_file_format_has_always_put_it_in() {
    for (raw, eight, sixteen, thousand) in [
        (0u128, 5, 5, 101),
        (1, 4, 4, 36),
        (u128::MAX, 5, 5, 85),
        (0x0199_c4f2_8a1c_7f3b_9d0e_5a6c_1b2d_3e4f, 3, 3, 163),
        (42, 7, 15, 719),
    ] {
        assert_eq!(home_of(raw, 8), eight, "{raw:#034x} in 8 buckets");
        assert_eq!(home_of(raw, 16), sixteen, "{raw:#034x} in 16 buckets");
        assert_eq!(home_of(raw, 1024), thousand, "{raw:#034x} in 1024 buckets");
    }
}

#[test]
fn a_key_is_hashed_by_the_whole_uuid_the_id_carries() {
    let uuid = Uuid::from_u128(0x0199_c4f2_8a1c_7f3b_9d0e_5a6c_1b2d_3e4f);
    let id = Id::<Book>::from_uuid(uuid);

    assert_eq!(id.raw(), uuid.to_u128());
    assert_eq!(home_of(id.raw(), 1024), 163);
}

#[test]
fn an_index_names_the_row_each_key_was_appended_at() {
    let Shelf { index, minted, .. } = stocked(20);

    for (row, id) in minted.iter().enumerate() {
        assert_eq!(index.row_of(*id).unwrap(), Some(row));
    }
}

#[test]
fn an_index_names_no_row_for_a_key_it_was_never_given() {
    let Shelf { index, .. } = stocked(20);

    assert_eq!(index.row_of(Id::mint()).unwrap(), None);
    assert!(index.rows_of(Id::mint()).unwrap().is_empty());
}

#[test]
fn an_empty_index_answers_for_no_key_at_all() {
    let index = empty();

    assert_eq!(index.rows(), 0);
    assert_eq!(index.capacity(), 0);
    assert_eq!(index.row_of(Id::mint()).unwrap(), None);
    assert!(index.agrees_with(0));
}

#[test]
fn every_row_filed_under_one_key_comes_back_in_the_order_it_was_appended() {
    let key = Id::<Book>::mint();
    let index = shared(12, key);

    assert_eq!(index.rows_of(key).unwrap(), (0..12).collect::<Vec<_>>());
    assert_eq!(index.row_of(key).unwrap(), Some(11));
}

#[test]
fn an_index_that_outgrew_its_buckets_still_names_every_key_it_already_held() {
    let Shelf { index, minted, .. } = stocked(300);

    assert_eq!(index.capacity(), 512);
    for (row, id) in minted.iter().enumerate() {
        assert_eq!(index.row_of(*id).unwrap(), Some(row), "row {row}");
    }
}

#[test]
fn a_repointed_row_leaves_the_key_it_had_for_the_one_it_was_given() {
    let first = Id::<Book>::mint();
    let second = Id::<Book>::mint();
    let mut index = shared(3, first);

    index.repoint_row(1, second).unwrap();

    assert_eq!(index.rows_of(first).unwrap(), [0, 2]);
    assert_eq!(index.rows_of(second).unwrap(), [1]);
}

#[test]
fn a_row_repointed_back_to_the_key_it_started_with_is_named_once_and_not_twice() {
    let first = Id::<Book>::mint();
    let second = Id::<Book>::mint();
    let mut index = shared(2, first);

    index.repoint_row(1, second).unwrap();
    index.repoint_row(1, first).unwrap();

    assert_eq!(
        index.rows_of(first).unwrap(),
        [0, 1],
        "a row that left and came back is still one row"
    );
    assert!(index.rows_of(second).unwrap().is_empty());
}

#[test]
fn the_only_row_a_bucket_held_can_be_repointed_away_from_it() {
    let first = Id::<Book>::mint();
    let second = Id::<Book>::mint();
    let mut index = shared(1, first);

    index.repoint_row(0, second).unwrap();

    assert!(index.rows_of(first).unwrap().is_empty());
    assert_eq!(index.rows_of(second).unwrap(), [0]);
}

#[test]
fn an_index_rebuilt_from_a_column_names_every_row_the_column_holds() {
    let Shelf { keys, minted, .. } = stocked(40);
    let mut fresh = empty();

    fresh.rebuild(&keys).unwrap();

    assert!(fresh.agrees_with(40));
    for (row, id) in minted.iter().enumerate() {
        assert_eq!(fresh.row_of(*id).unwrap(), Some(row));
    }
}

#[test]
fn an_index_agrees_only_with_the_number_of_rows_it_was_built_for() {
    let Shelf { index, .. } = stocked(9);

    assert_eq!(index.rows(), 9);
    assert_eq!(index.capacity(), 16);
    assert!(index.agrees_with(9));
    assert!(!index.agrees_with(8), "one row short is one link too many");
    assert!(!index.agrees_with(10));
}

#[test]
fn making_room_for_a_row_that_never_arrives_leaves_the_index_the_size_it_was() {
    let Shelf { index, .. } = stocked(8);

    assert_eq!(index.capacity(), 8);
    index.make_room(9).unwrap();

    assert!(
        index.agrees_with(8),
        "make_room reads what a growth would read; appending is what grows"
    );
    assert_eq!(index.capacity(), 8);
}

#[test]
fn an_index_describes_and_reports_residency_for_the_same_two_columns() {
    let Shelf { index, .. } = stocked(3);

    let mut specs = Vec::new();
    index.describe("book", "id", &mut specs);
    let mut held = Vec::new();
    index.residency("book", "id", &mut held);

    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].name, "book.id.buckets");
    assert_eq!(specs[0].elem_size, 8);
    assert_eq!(specs[0].kind, ColumnKind::Bucket);
    assert!(!specs[0].counts_rows);
    assert_eq!(specs[1].name, "book.id.links");
    assert_eq!(specs[1].elem_size, 24);
    assert_eq!(specs[1].kind, ColumnKind::Link);
    assert!(!specs[1].counts_rows);

    let named: Vec<&String> = specs.iter().map(|spec| &spec.name).collect();
    let reported: Vec<&String> = held.iter().map(|entry| &entry.name).collect();
    assert_eq!(named, reported);
}

#[test]
fn an_index_survives_a_commit_and_a_reopen() {
    let dir = TempDir::new("index_roundtrip");
    let path = dir.join("books.dbs");

    let Shelf { index, minted, .. } = stocked(30);
    let file = committed(&path, |updater| index.offer_to("book", "id", updater));

    let mut read = empty();
    read.read_from("book", "id", &file).unwrap();

    assert!(read.agrees_with(30));
    for (row, id) in minted.iter().enumerate() {
        assert_eq!(read.row_of(*id).unwrap(), Some(row));
    }
}

#[test]
fn reading_an_index_from_a_file_reads_none_of_its_payload() {
    let dir = TempDir::new("index_cold");
    let path = dir.join("books.dbs");

    let Shelf { index, .. } = stocked(30);
    let file = committed(&path, |updater| index.offer_to("book", "id", updater));

    let mut read = empty();
    read.read_from("book", "id", &file).unwrap();

    let mut held = Vec::new();
    read.residency("book", "id", &mut held);
    assert!(
        held.iter().all(|entry| entry.resident == 0),
        "{held:?} was faulted in by a read that only asked where the columns are"
    );
}

#[test]
fn touching_the_tail_of_a_rotten_index_reports_the_damage() {
    let dir = TempDir::new("index_tail");
    let path = dir.join("books.dbs");

    let Shelf { index, .. } = stocked(3);
    let file = committed(&path, |updater| index.offer_to("book", "id", updater));
    let tail = file.column_meta("book.id.links").unwrap().chunks[0];
    drop(file);
    flip_byte(&path, tail.offset);

    let mut read = empty();
    read.read_from("book", "id", &ColumnFile::open(&path).unwrap())
        .unwrap();

    let err = read.touch_tail().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}

#[test]
fn a_row_is_not_unlinked_until_the_bucket_it_is_moving_to_can_be_read() {
    let dir = TempDir::new("repoint_cold");
    let path = dir.join("books.dbs");

    let Shelf { index, minted, .. } = stocked(8193);
    assert_eq!(index.capacity(), 16384);
    let file = committed(&path, |updater| index.offer_to("book", "id", updater));
    let buckets = file.column_meta("book.id.buckets").unwrap();
    assert_eq!(buckets.chunks.len(), 2);
    let far = buckets.chunks[1];
    drop(file);

    flip_byte(&path, far.offset);

    let mut read = empty();
    read.read_from("book", "id", &ColumnFile::open(&path).unwrap())
        .unwrap();

    let near = (0..minted.len())
        .find(|&row| home_of(minted[row].raw(), 16384) < 8192)
        .expect("some key of eight thousand homes in the readable chunk");
    let unreadable = (0..64)
        .map(|_| Id::<Book>::mint())
        .find(|id| home_of(id.raw(), 16384) >= 8192)
        .expect("of sixty-four ids, one homes in the second half of the buckets");

    let err = read.repoint_row(near, unreadable).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert_eq!(
        read.rows_of(minted[near]).unwrap(),
        [near],
        "a row the index could not move is still filed under the key it had"
    );
}

#[test]
fn an_audit_passes_an_index_built_from_the_column_it_indexes() {
    let Shelf { keys, index, .. } = stocked(50);

    index.audit(&keys).unwrap();
}

#[test]
fn an_audit_refuses_an_index_that_holds_a_link_the_column_has_no_row_for() {
    let Shelf {
        keys, mut index, ..
    } = stocked(4);
    index.append(Id::mint()).unwrap();

    let err = index.audit(&keys).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(
        err.to_string().contains("holds 5 links for 4 rows"),
        "{err}"
    );
}

#[test]
fn an_audit_refuses_a_link_filed_under_a_key_its_row_does_not_hold() {
    let Shelf {
        keys, mut index, ..
    } = stocked(6);
    let stray = *index.links.row(2).unwrap();
    index
        .links
        .set(
            2,
            Link {
                key: Id::mint(),
                next: stray.next,
            },
        )
        .unwrap();

    let err = index.audit(&keys).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("row 2"), "{err}");
}

#[test]
fn an_audit_refuses_an_index_no_bucket_reaches_every_row_through() {
    let Shelf {
        keys, mut index, ..
    } = stocked(6);
    for home in 0..index.capacity() {
        index.buckets.set(home, NO_ROW).unwrap();
    }

    let err = index.audit(&keys).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(
        err.to_string().contains("reaches no link for row 0"),
        "{err}"
    );
}

#[test]
fn an_audit_refuses_an_index_whose_buckets_do_not_fit_its_rows() {
    let Shelf {
        keys, mut index, ..
    } = stocked(6);
    index.buckets.push(NO_ROW).unwrap();

    let err = index.audit(&keys).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(
        err.to_string().contains("holds 9 buckets for 6 rows"),
        "{err}"
    );
}

#[test]
fn a_chain_that_never_ends_is_reported_as_damage_and_never_hangs() {
    let key = Id::<Book>::mint();
    let mut index = shared(3, key);
    let last = *index.links.row(2).unwrap();
    index.links.set(0, Link { next: 2, ..last }).unwrap();
    for home in 0..index.capacity() {
        index.buckets.set(home, 0).unwrap();
    }

    let err = index.rows_of(key).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("without end"), "{err}");
}

#[test]
fn a_link_that_names_a_row_the_index_does_not_hold_is_reported_as_damage() {
    let key = Id::<Book>::mint();

    for names in [2usize, 9] {
        let mut index = shared(2, key);
        for home in 0..index.capacity() {
            index.buckets.set(home, names as u64).unwrap();
        }

        let err = index.row_of(key).unwrap_err();
        assert!(err.is_corrupt(), "{err}");
        assert!(
            err.to_string()
                .contains(&format!("names row {names}, which it does not hold")),
            "the row just past the last is not a row it holds either: {err}"
        );
    }
}

#[test]
fn a_row_no_bucket_reaches_can_never_be_moved_to_another_key() {
    let key = Id::<Book>::mint();
    let other = Id::<Book>::mint();
    let mut index = shared(2, key);
    let home = home_of(key.raw(), index.capacity());
    index.buckets.set(home, 0).unwrap();

    let err = index.repoint_row(1, other).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("row 1"), "{err}");
}

proptest! {
    #[test]
    fn every_home_is_a_bucket_the_index_holds(
        raw in any::<u128>(),
        power in 3u32..20,
    ) {
        let capacity = 1usize << power;
        prop_assert!(home_of(raw, capacity) < capacity);
    }

    #[test]
    fn a_capacity_always_holds_every_row_and_never_more_than_twice_as_many(rows in 0usize..5000) {
        let capacity = capacity_for(rows);
        prop_assert!(capacity >= rows, "{capacity} buckets cannot hold {rows} rows");
        prop_assert!(rows == 0 || capacity.is_power_of_two());
        prop_assert!(capacity <= (rows * 2).max(MIN_CAPACITY));
    }

    #[test]
    fn an_index_of_any_size_names_every_key_it_holds_and_no_other(rows in 0usize..200) {
        let Shelf { keys, index, minted } = stocked(rows);

        index.audit(&keys)?;
        for (row, id) in minted.iter().enumerate() {
            prop_assert_eq!(index.row_of(*id)?, Some(row));
        }
        prop_assert_eq!(index.row_of(Id::mint())?, None);
    }

    #[test]
    fn a_key_laid_out_by_hand_is_found_wherever_its_bucket_puts_it(
        ours in props::uuid(),
        theirs in prop::collection::vec(props::uuid(), 0..40),
    ) {
        let mine = Id::<Book>::from_uuid(ours);
        let mut index = empty();
        let mut ours_at = Vec::new();

        for (row, other) in theirs.iter().enumerate() {
            let id = Id::<Book>::from_uuid(*other);
            index.append(id)?;
            if id == mine {
                ours_at.push(row);
            }
        }
        let row = index.append(mine)?;
        ours_at.push(row);

        prop_assert_eq!(index.rows_of(mine)?, ours_at);
    }
}
