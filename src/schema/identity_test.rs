use super::*;
use crate::test_utils::library::Book;
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
fn two_rows_claiming_one_id_are_corruption_that_names_both() {
    let (mut column, ids) = stocked(2);
    column.push(ids[0]).unwrap();

    let err = column.row_of(ids[1]).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(
        err.to_string().contains("book.id rows 0 and 2 are both"),
        "{err}"
    );
    assert!(err.to_string().contains(&ids[0].to_string()), "{err}");
}

#[test]
fn a_row_with_no_id_at_all_is_corruption_that_names_the_row() {
    let (mut column, _) = stocked(1);
    column.push(Id::NIL).unwrap();

    let err = column.row_of(Id::mint()).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("book.id row 1 has no id"), "{err}");
}

#[test]
fn a_clone_answers_every_question_the_column_it_came_from_answers() {
    let (column, ids) = stocked(4);
    column.row_of(ids[2]).unwrap();

    let copy = column.clone();

    assert_eq!(copy.row_of(ids[2]).unwrap().index(), 2);
    assert_eq!(copy.at(Row::trusted(3)).unwrap(), ids[3]);
    assert_eq!(Column::rows(&copy), 4);
}

#[test]
fn the_identity_column_describes_itself_as_sixteen_bytes_that_count_rows() {
    let (column, _) = stocked(1);
    let mut specs = Vec::new();
    column.describe("book", "id", &mut specs);

    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "book.id");
    assert_eq!(specs[0].elem_size, 16);
    assert_eq!(specs[0].kind, ColumnKind::Identity);
    assert!(specs[0].counts_rows);
}

#[test]
fn an_identity_column_survives_a_commit_and_a_reopen() {
    let dir = crate::test_utils::TempDir::new("id_roundtrip");
    let path = dir.join("books.dbs");

    let (column, ids) = stocked(3);
    let file =
        crate::test_utils::committed(&path, |updater| column.offer_to("book", "id", updater));

    let mut read = IdColumn::<Book>::default();
    read.read_from("book", "id", &file).unwrap();

    assert_eq!(Column::rows(&read), 3);
    for (row, id) in ids.iter().enumerate() {
        assert_eq!(read.row_of(*id).unwrap().index(), row);
    }
}

#[test]
fn reading_an_identity_column_from_a_file_drops_the_map_built_for_the_old_rows() {
    let dir = crate::test_utils::TempDir::new("id_reread");
    let path = dir.join("books.dbs");

    let (column, ids) = stocked(2);
    let file =
        crate::test_utils::committed(&path, |updater| column.offer_to("book", "id", updater));

    let (mut later, others) = stocked(3);
    later.row_of(others[0]).unwrap();
    later.read_from("book", "id", &file).unwrap();

    assert_eq!(later.row_of(ids[1]).unwrap().index(), 1);
    assert_eq!(later.find(others[0]).unwrap(), None);
}

#[test]
fn touching_the_tail_of_a_rotten_identity_column_reports_the_damage() {
    let dir = crate::test_utils::TempDir::new("id_tail");
    let path = dir.join("books.dbs");

    let (column, _) = stocked(1);
    let file =
        crate::test_utils::committed(&path, |updater| column.offer_to("book", "id", updater));
    let tail = file.column_meta("book.id").unwrap().chunks[0];
    drop(file);
    crate::test_utils::flip_byte(&path, tail.offset);

    let mut read = IdColumn::<Book>::default();
    read.read_from(
        "book",
        "id",
        &crate::storage::ColumnFile::open(&path).unwrap(),
    )
    .unwrap();

    let err = read.touch_tail().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}

proptest! {
    #[test]
    fn a_column_of_any_size_finds_every_id_it_was_given_and_no_other(rows in 0usize..64) {
        let (column, ids) = stocked(rows);

        prop_assert_eq!(Column::rows(&column), rows);
        for (row, id) in ids.iter().enumerate() {
            prop_assert_eq!(column.row_of(*id).unwrap().index(), row);
        }
        prop_assert_eq!(column.find(Id::mint()).unwrap(), None);
    }
}
