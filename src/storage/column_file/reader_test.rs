use std::fs::OpenOptions;
use std::path::Path;

use super::*;
use crate::storage::ColumnFileWriter;
use crate::storage::arena::Arena;
use crate::storage::column_file::HEAD_LEN;
use crate::test_utils::{TempDir, flip_byte};

#[test]
fn a_column_read_from_disk_can_be_appended_to_without_reading_the_rest() {
    let dir = TempDir::new("append");
    let path = dir.join("numbers.dbs");

    let mut values = Arena::<u32>::with_chunk_rows(4);
    values
        .extend_from_slice(&(0..10).collect::<Vec<u32>>())
        .unwrap();
    let mut writer = ColumnFileWriter::create(&path, WriterId::ANONYMOUS).unwrap();
    writer.column("values", &values).unwrap();
    writer.finish().unwrap();

    let file = ColumnFile::open(&path).unwrap();
    let mut live = file.column::<u32>("values").unwrap();
    let frozen = live.clone();

    live.push(10).unwrap();
    assert_eq!(
        live.resident_chunks(),
        1,
        "only the tail chunk was faulted in"
    );
    assert_eq!(live.len(), 11);
    assert_eq!(frozen.len(), 10, "the frozen view did not see the append");
    assert_eq!(live.get(10).unwrap(), Some(&10));
    assert_eq!(
        live.get(0).unwrap(),
        Some(&0),
        "untouched chunks still load on demand"
    );
}

#[test]
fn a_flipped_byte_in_a_chunk_is_caught_on_read_not_on_open() {
    let dir = TempDir::new("bitrot");
    let path = dir.join("numbers.dbs");

    let mut values = Arena::<u32>::with_chunk_rows(4);
    values
        .extend_from_slice(&(0..10).collect::<Vec<u32>>())
        .unwrap();
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

    assert_eq!(values.get(0).unwrap(), Some(&0));
    assert_eq!(values.get(9).unwrap(), Some(&9));

    let err = values.get(4).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
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
        values.extend_from_slice(&[1, 2, 3]).unwrap();
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
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("failed checksum"), "{err}");

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
    assert!(err.is_corrupt(), "{err}");
    assert!(
        err.to_string().contains("extends outside the file"),
        "{err}"
    );

    let empty = dir.join("empty.dbs");
    std::fs::write(&empty, b"").unwrap();
    let err = ColumnFile::open(&empty).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("too short"), "{err}");
}

#[test]
fn a_column_cannot_be_read_at_the_wrong_width() {
    let dir = TempDir::new("width");
    let path = dir.join("numbers.dbs");

    let mut values = Arena::<u32>::new();
    values.extend_from_slice(&[1, 2, 3]).unwrap();
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

#[test]
fn a_commit_token_reads_without_touching_the_directory() {
    let dir = TempDir::new("token");
    let path = dir.join("values.dbs");

    let device = WriterId::from_device_id("KRZH3XY-7PH4NJK-2WQ5LMC");
    let mut values = Arena::<u32>::with_chunk_rows(4);
    values
        .extend_from_slice(&(0..10).collect::<Vec<u32>>())
        .unwrap();
    let mut writer = ColumnFileWriter::create(&path, device).unwrap();
    writer.column("values", &values).unwrap();
    writer.finish().unwrap();

    let file = ColumnFile::open(&path).unwrap();
    let token = ColumnFile::read_commit_token(&path).unwrap();
    assert_eq!(token, file.commit_token());
    assert_eq!(token.writer_id, device);
    assert_eq!(token.generation, 1);
    assert_eq!(token.file_uuid, file.file_uuid());

    let dir_offset = file.column_meta("values").unwrap().byte_len() + HEAD_LEN;
    flip_byte(&path, dir_offset);
    assert!(
        ColumnFile::open(&path).is_err(),
        "the directory is unreadable"
    );
    assert_eq!(
        ColumnFile::read_commit_token(&path).unwrap(),
        token,
        "but the token never read it"
    );
}

#[test]
fn a_token_tells_our_own_write_from_a_peers() {
    let dir = TempDir::new("echo");
    let path = dir.join("values.dbs");

    let mine = WriterId::from_device_id("mine");
    let peer = WriterId::from_device_id("peer");
    let mut values = Arena::<u32>::with_chunk_rows(4);
    values.extend_from_slice(&[1, 2, 3]).unwrap();
    let mut writer = ColumnFileWriter::create(&path, mine).unwrap();
    writer.column("values", &values).unwrap();
    writer.finish().unwrap();

    let token = ColumnFile::read_commit_token(&path).unwrap();
    assert!(token.is_echo_of(mine, 1));
    assert!(!token.is_echo_of(peer, 1));

    let file = ColumnFile::open(&path).unwrap();
    let mut column = file.column::<u32>("values").unwrap();
    column.set(0, 9).unwrap();
    let mut updater = file.update(peer).unwrap();
    updater.column("values", &column).unwrap();
    updater.commit().unwrap();

    let token = ColumnFile::read_commit_token(&path).unwrap();
    assert!(!token.is_echo_of(mine, 1), "the peer moved it on");
    assert_eq!(token.writer_id, peer);
    assert_eq!(token.generation, 2);
}

#[test]
fn a_token_needs_a_valid_slot() {
    let dir = TempDir::new("token_empty");
    let path = dir.join("bare.dbs");
    std::fs::write(&path, vec![0u8; HEAD_LEN as usize]).unwrap();

    let err = ColumnFile::read_commit_token(&path).unwrap_err();
    assert!(err.to_string().contains("bad column file magic"), "{err}");

    std::fs::write(&path, b"short").unwrap();
    let err = ColumnFile::read_commit_token(&path).unwrap_err();
    assert!(err.to_string().contains("too short"), "{err}");
}

#[test]
fn a_file_we_are_too_old_to_read_is_not_reported_as_damaged() {
    let dir = TempDir::new("too_old");
    let path = dir.join("values.dbs");
    let mut values = Arena::<u32>::with_chunk_rows(4);
    values
        .extend_from_slice(&(0..10).collect::<Vec<u32>>())
        .unwrap();
    let mut writer = ColumnFileWriter::create(&path, WriterId::ANONYMOUS).unwrap();
    writer.column("values", &values).unwrap();
    writer.finish().unwrap();

    let chunk = ColumnFile::open(&path)
        .unwrap()
        .column_meta("values")
        .unwrap()
        .chunks[0];
    flip_byte(&path, chunk.offset);
    let damaged = ColumnFile::open(&path)
        .unwrap()
        .column::<u32>("values")
        .unwrap()
        .row(0)
        .unwrap_err();

    let too_new = Codec::from_u8(7).unwrap_err();

    assert!(damaged.is_corrupt(), "{damaged}");
    assert!(too_new.is_unsupported(), "{too_new}");
    assert!(
        !too_new.is_corrupt(),
        "overwriting a file we are merely too old to read would destroy data a peer still has"
    );
}

#[test]
fn appending_to_a_damaged_byte_column_errors_rather_than_aborting() {
    let dir = TempDir::new("append_damaged");
    let path = dir.join("bytes.dbs");
    let mut bytes = Arena::<u8>::with_chunk_rows(16);
    bytes
        .push_str("some text that spans a couple of chunks")
        .unwrap();
    let mut writer = ColumnFileWriter::create(&path, WriterId::ANONYMOUS).unwrap();
    writer.column("bytes", &bytes).unwrap();
    writer.finish().unwrap();

    let file = ColumnFile::open(&path).unwrap();
    let tail = *file.column_meta("bytes").unwrap().chunks.last().unwrap();
    assert!(
        tail.rows < 16,
        "the tail chunk must be partial to be faulted in"
    );
    flip_byte(&path, tail.offset);

    let mut column = file.column::<u8>("bytes").unwrap();
    let err = column.push_str("more").unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}
