use super::*;
use crate::storage::ColumnFile;
use crate::test_utils::library::Book;
use crate::test_utils::{TempDir, committed, flip_byte};
use proptest::prelude::*;

fn stocked(rows: usize) -> (IdColumn<Book>, Vec<Id<Book>>) {
    let mut column = IdColumn::default();
    let ids = (0..rows)
        .map(|_| {
            let id = Id::mint();
            column.push(id).unwrap();
            id
        })
        .collect();
    (column, ids)
}

#[test]
fn every_id_is_found_at_the_row_it_was_pushed_into() {
    let (column, ids) = stocked(3);

    for (row, id) in ids.iter().enumerate() {
        assert_eq!(column.row_of(*id).unwrap().index(), row);
        assert_eq!(column.at(Row::trusted(row)).unwrap(), *id);
    }
}

#[test]
fn an_id_no_row_holds_is_refused_by_the_entity_that_does_not_hold_it() {
    let (column, _) = stocked(2);
    let stranger = Id::mint();

    let err = column.row_of(stranger).unwrap_err();
    assert!(err.to_string().contains("book has no row with id"), "{err}");
    assert!(err.to_string().contains(&stranger.to_string()), "{err}");
    assert_eq!(column.find(stranger).unwrap(), None);
}

#[test]
fn an_empty_column_holds_no_row_for_any_id() {
    let column = IdColumn::<Book>::default();

    assert_eq!(Column::rows(&column), 0);
    assert_eq!(column.find(Id::mint()).unwrap(), None);
    column.audit().unwrap();
}

#[test]
fn an_id_pushed_after_a_lookup_is_found_by_the_next_one() {
    let (mut column, ids) = stocked(1);
    assert_eq!(column.row_of(ids[0]).unwrap().index(), 0);

    let later = Id::mint();
    column.push(later).unwrap();

    assert_eq!(column.row_of(later).unwrap().index(), 1);
    assert_eq!(column.row_of(ids[0]).unwrap().index(), 0);
}

#[test]
fn a_second_row_claiming_an_id_the_column_already_holds_is_refused_at_the_door() {
    let (mut column, ids) = stocked(3);

    let err = column.push(ids[1]).unwrap_err();
    assert!(!err.is_corrupt(), "a caller asked for this: {err}");
    assert!(
        err.to_string().contains("book already holds row 1 with id"),
        "{err}"
    );
    assert!(err.to_string().contains(&ids[1].to_string()), "{err}");
    assert_eq!(Column::rows(&column), 3, "the refused push grew nothing");
}

#[test]
fn a_row_can_never_be_given_no_id_at_all() {
    let (mut column, _) = stocked(1);

    let err = column.push(Id::NIL).unwrap_err();
    assert!(!err.is_corrupt(), "a caller asked for this: {err}");
    assert!(
        err.to_string()
            .contains("a book row cannot be given no id at all"),
        "{err}"
    );
    assert_eq!(Column::rows(&column), 1);
}

#[test]
fn an_audit_passes_a_column_of_any_size_every_id_was_pushed_into() {
    for rows in [0, 1, 8, 9, 16, 17, 32, 33, 40] {
        let (column, _) = stocked(rows);

        column.audit().unwrap_or_else(|err| {
            panic!("a column grown one push at a time to {rows} rows is sound: {err}")
        });
    }
}

#[test]
fn two_rows_claiming_one_id_are_corruption_that_names_both() {
    let (mut column, ids) = stocked(3);
    column.ids.set(2, ids[0]).unwrap();
    column.index.repoint_row(2, ids[0]).unwrap();

    let err = column.audit().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(
        err.to_string().contains("book.id rows 0 and 2 are both"),
        "{err}"
    );
    assert!(err.to_string().contains(&ids[0].to_string()), "{err}");
}

#[test]
fn a_row_with_no_id_at_all_is_corruption_that_names_the_row() {
    let (mut column, _) = stocked(2);
    column.ids.set(1, Id::NIL).unwrap();
    column.index.repoint_row(1, Id::NIL).unwrap();

    let err = column.audit().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("book.id row 1 has no id"), "{err}");
}

#[test]
fn an_audit_refuses_an_index_that_does_not_answer_for_the_ids_the_column_holds() {
    let (mut column, ids) = stocked(3);
    column.ids.set(1, Id::mint()).unwrap();

    let err = column.audit().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("book.id"), "{err}");
    assert!(!err.to_string().contains(&ids[1].to_string()), "{err}");
}

#[test]
fn a_clone_answers_every_question_the_column_it_came_from_answers() {
    let (column, ids) = stocked(4);

    let copy = column.clone();

    assert_eq!(copy.row_of(ids[2]).unwrap().index(), 2);
    assert_eq!(copy.at(Row::trusted(3)).unwrap(), ids[3]);
    assert_eq!(Column::rows(&copy), 4);
}

#[test]
fn the_identity_column_describes_the_ids_and_the_index_that_finds_their_rows() {
    let (column, _) = stocked(1);
    let mut specs = Vec::new();
    column.describe("book", "id", &mut specs);
    let mut held = Vec::new();
    column.residency("book", "id", &mut held);

    assert_eq!(specs.len(), 3);
    assert_eq!(specs[0].name, "book.id");
    assert_eq!(specs[0].elem_size, 16);
    assert_eq!(specs[0].kind, ColumnKind::Identity);
    assert!(specs[0].counts_rows);
    assert_eq!(specs[1].name, "book.id.buckets");
    assert_eq!(specs[2].name, "book.id.links");

    let named: Vec<&String> = specs.iter().map(|spec| &spec.name).collect();
    let reported: Vec<&String> = held.iter().map(|entry| &entry.name).collect();
    assert_eq!(named, reported);
}

#[test]
fn an_identity_column_survives_a_commit_and_a_reopen() {
    let dir = TempDir::new("id_roundtrip");
    let path = dir.join("books.dbs");

    let (column, ids) = stocked(3);
    let file = committed(&path, |updater| column.offer_to("book", "id", updater));

    let mut read = IdColumn::<Book>::default();
    read.read_from("book", "id", &file).unwrap();

    assert_eq!(Column::rows(&read), 3);
    for (row, id) in ids.iter().enumerate() {
        assert_eq!(read.row_of(*id).unwrap().index(), row);
    }
    read.audit().unwrap();
}

#[test]
fn reading_an_identity_column_from_a_file_answers_for_its_rows_and_for_no_others() {
    let dir = TempDir::new("id_reread");
    let path = dir.join("books.dbs");

    let (column, ids) = stocked(2);
    let file = committed(&path, |updater| column.offer_to("book", "id", updater));

    let (mut later, others) = stocked(3);
    later.row_of(others[0]).unwrap();
    later.read_from("book", "id", &file).unwrap();

    assert_eq!(later.row_of(ids[1]).unwrap().index(), 1);
    assert_eq!(later.find(others[0]).unwrap(), None);
}

#[test]
fn an_identity_column_read_beside_an_index_that_lost_rows_is_rebuilt_from_the_ids() {
    let dir = TempDir::new("id_stale");
    let path = dir.join("books.dbs");

    let (column, ids) = stocked(5);
    let mut behind = HashIndex::new(qualified("book", "id"));
    for id in &ids[..3] {
        behind.append(*id).unwrap();
    }
    let file = committed(&path, |updater| {
        updater.column("book.id", &column.ids)?;
        behind.offer_to("book", "id", updater)
    });

    let mut read = IdColumn::<Book>::default();
    read.read_from("book", "id", &file).unwrap();

    for (row, id) in ids.iter().enumerate() {
        assert_eq!(
            read.row_of(*id).unwrap().index(),
            row,
            "an index the file left behind is rebuilt from the ids it indexes"
        );
    }
    read.audit().unwrap();
}

fn held<'a>(reported: &'a [Residency], name: &str) -> &'a Residency {
    reported
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("no residency reported for {name}"))
}

fn reported(column: &IdColumn<Book>) -> Vec<Residency> {
    let mut out = Vec::new();
    column.residency("book", "id", &mut out);
    out
}

#[test]
fn a_lookup_after_a_cold_open_reads_a_slot_and_never_the_column_it_indexes() {
    let dir = TempDir::new("id_cold_lookup");
    let path = dir.join("books.dbs");

    let (column, ids) = stocked(8200);
    let file = committed(&path, |updater| column.offer_to("book", "id", updater));

    let mut read = IdColumn::<Book>::default();
    read.read_from("book", "id", &file).unwrap();
    let cold = reported(&read);
    assert!(
        cold.iter().all(|entry| entry.resident == 0),
        "opening read payload it was not asked for: {cold:?}"
    );
    assert!(held(&cold, "book.id").chunks >= 3, "{cold:?}");

    assert_eq!(read.row_of(ids[7000]).unwrap().index(), 7000);

    let warm = reported(&read);
    assert_eq!(
        held(&warm, "book.id").resident,
        0,
        "a link carries its own key, so finding a row never reads the ids"
    );
    assert_eq!(
        held(&warm, "book.id.buckets").resident,
        1,
        "one bucket was read, not the bucket array"
    );
    let links = held(&warm, "book.id.links");
    assert!(
        (1..links.chunks).contains(&links.resident),
        "a chain is walked, not the whole column: {links:?}"
    );
}

#[test]
fn touching_the_tail_of_a_rotten_identity_column_reports_the_damage() {
    let dir = TempDir::new("id_tail");
    let path = dir.join("books.dbs");

    let (column, _) = stocked(1);
    let file = committed(&path, |updater| column.offer_to("book", "id", updater));
    let tail = file.column_meta("book.id").unwrap().chunks[0];
    drop(file);
    flip_byte(&path, tail.offset);

    let mut read = IdColumn::<Book>::default();
    read.read_from("book", "id", &ColumnFile::open(&path).unwrap())
        .unwrap();

    let err = read.touch_tail().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}

#[test]
fn touching_the_tail_of_a_rotten_index_reports_the_damage_too() {
    let dir = TempDir::new("id_index_tail");
    let path = dir.join("books.dbs");

    let (column, _) = stocked(1);
    let file = committed(&path, |updater| column.offer_to("book", "id", updater));
    let tail = file.column_meta("book.id.links").unwrap().chunks[0];
    drop(file);
    flip_byte(&path, tail.offset);

    let mut read = IdColumn::<Book>::default();
    read.read_from("book", "id", &ColumnFile::open(&path).unwrap())
        .unwrap();

    let err = read.touch_tail().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}

proptest! {
    #[test]
    fn a_column_of_any_size_finds_every_id_it_was_given_and_no_other(rows in 0usize..200) {
        let (column, ids) = stocked(rows);

        prop_assert_eq!(Column::rows(&column), rows);
        column.audit()?;
        for (row, id) in ids.iter().enumerate() {
            prop_assert_eq!(column.row_of(*id).unwrap().index(), row);
        }
        prop_assert_eq!(column.find(Id::mint()).unwrap(), None);
    }
}
