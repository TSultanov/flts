use super::*;

fn row(index: usize, rows: usize) -> Row {
    Row::of(index, rows, "page").unwrap()
}

#[test]
fn bytes_that_are_not_text_read_back_exactly() {
    let mut scans = BytesColumn::default();
    scans.push(&[0xff, 0x00, 0xfe]).unwrap();
    scans.push(&[]).unwrap();

    assert_eq!(*scans.get(row(0, 2)).unwrap(), [0xff, 0x00, 0xfe]);
    assert!(scans.get(row(1, 2)).unwrap().is_empty());
}

#[test]
fn a_row_value_survives_the_trip_another_copy_takes_it_on() {
    let mut scans = BytesColumn::default();
    scans.push(&[1, 2, 3]).unwrap();
    let carried = scans.value_bytes(row(0, 1)).unwrap();

    let mut theirs = BytesColumn::default();
    theirs.append_value_bytes(&carried).unwrap();

    assert_eq!(*theirs.get(row(0, 1)).unwrap(), [1, 2, 3]);
    assert!(
        theirs
            .append_value_bytes(&carried[..carried.len() - 1])
            .is_err()
    );
}

#[test]
fn compaction_drops_the_bytes_a_rewrite_left_behind() {
    let mut scans = BytesColumn::default();
    scans.push(&[7; 64]).unwrap();
    scans.set(row(0, 1), &[9; 8]).unwrap();
    assert_eq!(scans.unused_heap_bytes().unwrap(), 64);

    scans.compact_storage().unwrap();

    assert_eq!(scans.unused_heap_bytes().unwrap(), 0);
    assert_eq!(scans.heap_len(), 8);
    assert_eq!(*scans.get(row(0, 1)).unwrap(), [9; 8]);
}

#[test]
fn absent_bytes_are_not_empty_bytes() {
    let mut scans = NullableBytesColumn::default();
    scans.push(None).unwrap();
    scans.push(Some(&[])).unwrap();
    scans.push(Some(&[5])).unwrap();

    assert!(scans.get(row(0, 3)).unwrap().is_none());
    assert!(
        scans
            .get(row(1, 3))
            .unwrap()
            .is_some_and(|held| held.is_empty())
    );
    assert_eq!(scans.get(row(2, 3)).unwrap().as_deref(), Some(&[5u8][..]));

    let mut theirs = NullableBytesColumn::default();
    for index in 0..3 {
        theirs
            .append_value_bytes(&scans.value_bytes(row(index, 3)).unwrap())
            .unwrap();
    }
    assert!(theirs.get(row(0, 3)).unwrap().is_none());
    assert_eq!(theirs.get(row(2, 3)).unwrap().as_deref(), Some(&[5u8][..]));
    assert!(theirs.append_value_bytes(&[2]).is_err());
}

#[test]
fn optional_compaction_keeps_absence_and_shares_equal_values() {
    let mut scans = NullableBytesColumn::default();
    scans.push(Some(&[4; 16])).unwrap();
    scans.push(None).unwrap();
    scans.push(Some(&[4; 16])).unwrap();

    scans.compact_storage().unwrap();

    assert_eq!(scans.heap_len(), 16);
    assert!(scans.get(row(1, 3)).unwrap().is_none());
    assert_eq!(
        scans.get(row(2, 3)).unwrap().as_deref(),
        Some(&[4u8; 16][..])
    );
}
