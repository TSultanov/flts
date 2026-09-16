use std::fs::OpenOptions;
use std::path::Path;

use super::*;
use crate::storage::ColumnFileWriter;
use crate::storage::arena::Arena;
use crate::test_utils::{TempDir, flip_byte};

#[test]
fn a_column_read_from_disk_can_be_appended_to_without_reading_the_rest() {
    let dir = TempDir::new("append");
    let path = dir.join("numbers.dbs");

    let mut values = Arena::<u32>::with_chunk_rows(4);
    values.extend_from_slice(&(0..10).collect::<Vec<u32>>());
    let mut writer = ColumnFileWriter::create(&path, WriterId::ANONYMOUS).unwrap();
    writer.column("values", &values).unwrap();
    writer.finish().unwrap();

    let file = ColumnFile::open(&path).unwrap();
    let mut live = file.column::<u32>("values").unwrap();
    let frozen = live.clone();

    live.try_push(10).unwrap();
    assert_eq!(
        live.resident_chunks(),
        1,
        "only the tail chunk was faulted in"
    );
    assert_eq!(live.len(), 11);
    assert_eq!(frozen.len(), 10, "the frozen view did not see the append");
    assert_eq!(live.try_get(10).unwrap(), Some(&10));
    assert_eq!(
        live.try_get(0).unwrap(),
        Some(&0),
        "untouched chunks still load on demand"
    );
}

#[test]
fn a_flipped_byte_in_a_chunk_is_caught_on_read_not_on_open() {
    let dir = TempDir::new("bitrot");
    let path = dir.join("numbers.dbs");

    let mut values = Arena::<u32>::with_chunk_rows(4);
    values.extend_from_slice(&(0..10).collect::<Vec<u32>>());
    let mut writer = ColumnFileWriter::create(&path, WriterId::ANONYMOUS).unwrap();
    writer.column("values", &values).unwrap();
    writer.finish().unwrap();

    let second_chunk = ColumnFile::open(&path)
        .unwrap()
        .column_meta("values")
        .unwrap()
        .chunks[1];
    flip_byte(&path, second_chunk.offset);

    let file = ColumnFile::open(&path).unwrap();
    let values = file.column::<u32>("values").unwrap();

    assert_eq!(values.try_get(0).unwrap(), Some(&0));
    assert_eq!(values.try_get(9).unwrap(), Some(&9));

    let err = values.try_get(4).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("failed checksum"), "{err}");

    assert!(
        file.verify()
            .unwrap_err()
            .to_string()
            .contains("failed checksum")
    );
}

#[test]
fn a_damaged_directory_or_slot_is_caught_on_open() {
    let dir = TempDir::new("directory");

    let build = |path: &Path| {
        let mut values = Arena::<u32>::new();
        values.extend_from_slice(&[1, 2, 3]);
        let mut writer = ColumnFileWriter::create(path, WriterId::ANONYMOUS).unwrap();
        writer.column("values", &values).unwrap();
        writer.finish().unwrap();
    };

    let damaged = dir.join("damaged.dbs");
    build(&damaged);
    let dir_offset = ColumnFile::open(&damaged)
        .unwrap()
        .column_meta("values")
        .unwrap()
        .byte_len()
        + HEAD_LEN;
    flip_byte(&damaged, dir_offset);
    let err = ColumnFile::open(&damaged).unwrap_err();
    assert!(
        err.to_string().contains("directory failed checksum"),
        "{err}"
    );

    let truncated = dir.join("truncated.dbs");
    build(&truncated);
    let len = std::fs::metadata(&truncated).unwrap().len();
    OpenOptions::new()
        .write(true)
        .open(&truncated)
        .unwrap()
        .set_len(len - 4)
        .unwrap();
    let err = ColumnFile::open(&truncated).unwrap_err();
    assert!(
        err.to_string()
            .contains("directory extends outside the file"),
        "{err}"
    );

    let empty = dir.join("empty.dbs");
    std::fs::write(&empty, b"").unwrap();
    assert!(
        ColumnFile::open(&empty)
            .unwrap_err()
            .to_string()
            .contains("too short")
    );
}

#[test]
fn a_column_cannot_be_read_at_the_wrong_width() {
    let dir = TempDir::new("width");
    let path = dir.join("numbers.dbs");

    let mut values = Arena::<u32>::new();
    values.extend_from_slice(&[1, 2, 3]);
    let mut writer = ColumnFileWriter::create(&path, WriterId::ANONYMOUS).unwrap();
    writer.column("values", &values).unwrap();
    writer.finish().unwrap();

    let file = ColumnFile::open(&path).unwrap();
    let err = file.column::<u64>("values").unwrap_err();
    assert!(err.to_string().contains("4-byte rows"), "{err}");
    assert!(
        file.column::<u32>("missing")
            .unwrap_err()
            .to_string()
            .contains("no column")
    );
}
