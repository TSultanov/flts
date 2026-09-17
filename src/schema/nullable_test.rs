use super::*;
use crate::test_utils::{TempDir, committed};

fn row(index: usize, rows: usize) -> Row {
    Row::of(index, rows, "card").unwrap()
}

#[test]
fn a_present_row_reads_its_value_and_an_absent_row_reads_none() {
    let mut interval = NullableColumn::<u64>::default();
    interval.push(Some(7)).unwrap();
    interval.push(None).unwrap();

    assert_eq!(interval.get(row(0, 2)).unwrap(), Some(7));
    assert_eq!(interval.get(row(1, 2)).unwrap(), None);
}

#[test]
fn an_absent_row_still_occupies_a_row_in_both_columns() {
    let mut interval = NullableColumn::<u64>::default();
    interval.push(None).unwrap();
    interval.push(Some(3)).unwrap();

    assert_eq!(interval.rows(), 2);
    assert_eq!(
        interval.get(row(1, 2)).unwrap(),
        Some(3),
        "the absent row did not shift the one after it"
    );
}

#[test]
fn a_zero_that_is_present_is_not_an_absent_value() {
    let mut interval = NullableColumn::<u64>::default();
    interval.push(Some(0)).unwrap();

    assert_eq!(
        interval.get(row(0, 1)).unwrap(),
        Some(0),
        "every bit pattern of a u64 is a real value, which is why presence is a column"
    );
}

#[test]
fn a_value_can_be_given_and_taken_away_again() {
    let mut interval = NullableColumn::<f64>::default();
    interval.push(None).unwrap();

    interval.set(row(0, 1), Some(1.5)).unwrap();
    assert_eq!(interval.get(row(0, 1)).unwrap(), Some(1.5));

    interval.set(row(0, 1), None).unwrap();
    assert_eq!(interval.get(row(0, 1)).unwrap(), None);
}

#[test]
fn a_nullable_scalar_asks_for_a_value_column_and_a_presence_column() {
    let mut specs = Vec::new();
    NullableColumn::<u64>::default().describe("card", "interval", &mut specs);

    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].name, "card.interval");
    assert_eq!(specs[0].elem_size, 8);
    assert_eq!(
        specs[0].kind,
        ColumnKind::Nullable(Box::new(ColumnKind::Scalar))
    );
    assert_eq!(specs[1].name, "card.interval.present");
    assert_eq!(specs[1].elem_size, 1);
    assert_eq!(specs[1].kind, ColumnKind::Presence);
    assert!(
        specs[1].counts_rows,
        "presence has one entry per row, so it must agree with the value column"
    );
}

#[test]
fn a_nullable_scalar_survives_a_commit_and_a_reopen() {
    let dir = TempDir::new("nullable");
    let path = dir.join("cards.dbs");

    let mut interval = NullableColumn::<u64>::default();
    interval.push(Some(11)).unwrap();
    interval.push(None).unwrap();
    interval.push(Some(13)).unwrap();
    let file = committed(&path, |updater| {
        interval.offer_to("card", "interval", updater)
    });

    let mut read = NullableColumn::<u64>::default();
    read.read_from("card", "interval", &file).unwrap();

    assert_eq!(read.rows(), 3);
    assert_eq!(read.get(row(0, 3)).unwrap(), Some(11));
    assert_eq!(read.get(row(1, 3)).unwrap(), None);
    assert_eq!(read.get(row(2, 3)).unwrap(), Some(13));
    assert!(file.column_meta("card.interval.present").is_some());
}
