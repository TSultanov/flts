use super::*;
use crate::storage::Arena;
use crate::test_utils::{TempDir, committed};

#[test]
fn a_row_can_only_name_an_index_the_column_holds() {
    assert_eq!(Row::of(2, 3, "book").unwrap().index(), 2);

    let err = Row::of(3, 3, "book").unwrap_err();
    assert!(err.to_string().contains("book has no row 3"), "{err}");
    assert!(err.to_string().contains("it holds 3"), "{err}");
}

#[test]
fn no_row_exists_in_a_column_that_holds_nothing() {
    let err = Row::of(0, 0, "paragraph").unwrap_err();
    assert!(err.to_string().contains("paragraph has no row 0"), "{err}");
}

#[test]
fn a_scalar_column_describes_itself_as_one_file_column() {
    let mut values = Arena::<u32>::new();
    values.extend_from_slice(&[1, 2, 3]).unwrap();

    let mut specs = Vec::new();
    values.describe("book", "year", &mut specs);

    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "book.year");
    assert_eq!(specs[0].elem_size, 4);
    assert_eq!(specs[0].kind, ColumnKind::Scalar);
    assert!(specs[0].counts_rows);
    assert_eq!(values.rows(), 3);
}

#[test]
fn a_scalar_column_survives_a_commit_and_a_reopen() {
    let dir = TempDir::new("scalar");
    let path = dir.join("values.dbs");

    let mut values = Arena::<u32>::with_chunk_rows(4);
    values.extend_from_slice(&[7, 8, 9]).unwrap();
    let file = committed(&path, |updater| values.offer_to("book", "year", updater));

    let mut read = Arena::<u32>::new();
    read.read_from("book", "year", &file).unwrap();

    assert_eq!(read.rows(), 3);
    assert_eq!(*read.row(1).unwrap(), 8);
    assert!(file.column_meta("book.year").is_some());
}

#[test]
fn a_column_erased_behind_a_field_still_reports_its_own_rows() {
    let mut values = Arena::<u64>::new();
    values.extend_from_slice(&[1, 2, 3, 4]).unwrap();
    let field = Field::new("year", &values);

    assert_eq!(field.name, "year");
    assert_eq!(field.column.rows(), 4);
}

#[test]
fn a_column_erased_behind_a_mutable_field_can_be_read_into() {
    let dir = TempDir::new("erased");
    let path = dir.join("values.dbs");

    let mut written = Arena::<u16>::with_chunk_rows(2);
    written.extend_from_slice(&[10, 20, 30]).unwrap();
    let file = committed(&path, |updater| written.offer_to("book", "rank", updater));

    let mut target = Arena::<u16>::new();
    let field = FieldMut::new("rank", &mut target);
    field.column.read_from("book", "rank", &file).unwrap();

    assert_eq!(target.rows(), 3);
    assert_eq!(*target.row(2).unwrap(), 30);
}
