use std::fs::OpenOptions;
use std::path::Path;

use super::*;
use crate::storage::ColumnFileWriter;
use crate::storage::arena::Arena;
use crate::storage::blob::Blob;
use crate::storage::column_file::{FORMAT_VERSION, HEAD_LEN, MAGIC};
use crate::test_utils::column_file_bytes as model;
use crate::test_utils::{TempDir, flip_byte};
use proptest::prelude::*;

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

    assert!(file.verify().unwrap_err().is_corrupt());
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
    let dir_offset = ColumnFile::open(&damaged).unwrap().slot.dir_offset;
    flip_byte(&damaged, dir_offset);
    let err = ColumnFile::open(&damaged).unwrap_err();
    assert!(err.is_corrupt(), "{err}");

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

    let dir_offset = file.slot.dir_offset;
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

#[test]
fn the_debug_of_a_column_file_names_its_generation_and_columns() {
    let dir = TempDir::new("reader_debug");
    let path = dir.join("debug.dbs");
    let image = model::FileModel::of(vec![model::Column::of("values", 4, 4, &[4, 2])]).bytes();
    std::fs::write(&path, &image).unwrap();

    let shown = format!("{:?}", ColumnFile::open(&path).unwrap());
    for part in ["generation", "columns", "values"] {
        assert!(shown.contains(part), "{shown} is missing {part}");
    }
}

proptest! {
    #[test]
    fn live_bytes_counts_the_bytes_its_columns_hold(
        chunk_rows in 1u64..6,
        chunks in 1usize..5,
    ) {
        let dir = TempDir::new("live_bytes");
        let path = dir.join("live.dbs");
        let counts = vec![chunk_rows as u32; chunks];
        let image = model::FileModel::of(vec![model::Column::of(
            "values",
            4,
            chunk_rows,
            &counts,
        )])
        .bytes();
        std::fs::write(&path, &image).unwrap();

        let file = ColumnFile::open(&path).unwrap();
        let held = 4 * chunk_rows * chunks as u64;

        prop_assert_eq!(file.live_bytes(), held);
        file.verify().unwrap();
    }
}

#[test]
fn a_file_that_is_only_a_head_has_no_commit_rather_than_being_too_short() {
    let dir = TempDir::new("head_only");
    let path = dir.join("head.dbs");

    let mut image = vec![0u8; HEAD_LEN as usize];
    image[..4].copy_from_slice(&MAGIC);
    image[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    image[8..24].copy_from_slice(&1u128.to_le_bytes());
    std::fs::write(&path, &image).unwrap();

    let err = ColumnFile::open(&path).unwrap_err();
    assert!(err.to_string().contains("no valid commit slot"), "{err}");
    assert!(err.is_corrupt());
}

#[test]
fn a_file_whose_directory_starts_where_the_payload_would_is_readable() {
    let dir = TempDir::new("no_payload");
    let path = dir.join("empty.dbs");

    let image = model::FileModel::of(vec![model::Column::of("empty", 4, 4, &[])]).bytes();
    std::fs::write(&path, &image).unwrap();

    let file = ColumnFile::open(&path).unwrap();
    assert_eq!(file.live_bytes(), 0);
    assert_eq!(file.column_meta("empty").unwrap().chunks.len(), 0);
    assert!(file.column::<u32>("empty").unwrap().is_empty());
    file.verify().unwrap();
}

#[test]
fn a_cloned_handle_serves_the_generation_it_was_cloned_from() {
    let dir = TempDir::new("cloned");
    let path = dir.join("values.dbs");

    let mut values = Arena::<u32>::with_chunk_rows(4);
    values.extend_from_slice(&[1, 2, 3]).unwrap();
    let mut writer = ColumnFileWriter::create(&path, WriterId::ANONYMOUS).unwrap();
    writer.column("values", &values).unwrap();
    writer.finish().unwrap();

    let file = ColumnFile::open(&path).unwrap();
    let pinned = file.clone();

    let mut edited = file.column::<u32>("values").unwrap();
    edited.set(0, 99).unwrap();
    let mut updater = file.update(WriterId::ANONYMOUS).unwrap();
    updater.column("values", &edited).unwrap();
    assert_eq!(updater.commit().unwrap(), 2);

    assert_eq!(pinned.generation(), 1);
    assert_eq!(
        pinned
            .column::<u32>("values")
            .unwrap()
            .iter()
            .map(|row| row.copied())
            .collect::<Result<Vec<_>>>()
            .unwrap(),
        vec![1, 2, 3],
        "the clone is a handle on its own generation, not a view of the newest"
    );

    let refused = pinned.update(WriterId::ANONYMOUS).unwrap_err();
    assert!(
        refused.to_string().contains("moved to generation 2"),
        "{refused}"
    );
}

#[test]
fn a_nullable_text_column_tells_an_absent_string_from_an_empty_one() {
    let dir = TempDir::new("nullable_text");
    let path = dir.join("notes.dbs");

    let mut heap = Arena::<u8>::with_chunk_rows(8);
    let mut notes = Arena::<Option<Blob>>::with_chunk_rows(4);
    let filled = heap.push_str("a note").unwrap();
    let empty = heap.push_str("").unwrap();
    notes.push(Some(filled)).unwrap();
    notes.push(None).unwrap();
    notes.push(Some(empty)).unwrap();

    let mut writer = ColumnFileWriter::create(&path, WriterId::ANONYMOUS).unwrap();
    writer.column("note", &notes).unwrap();
    writer.column("note.bytes", &heap).unwrap();
    writer.finish().unwrap();

    let file = ColumnFile::open(&path).unwrap();
    file.verify().unwrap();
    let read_notes = file.column::<Option<Blob>>("note").unwrap();
    let read_heap = file.column::<u8>("note.bytes").unwrap();

    let text_at = |row: usize| -> Option<String> {
        (*read_notes.row(row).unwrap()).map(|blob| read_heap.text(blob).unwrap().into_owned())
    };

    assert_eq!(text_at(0), Some("a note".to_owned()));
    assert_eq!(text_at(1), None, "an absent note is absent, not empty");
    assert_eq!(text_at(2), Some(String::new()));
    assert_eq!(
        file.column_meta("note").unwrap().elem_size,
        <Blob as Record>::SIZE as u32,
        "nullability costs no bytes and no second column"
    );
}

#[test]
fn a_file_cut_off_before_its_newest_directory_opens_at_the_commit_before_it() {
    let dir = TempDir::new("cut_off");
    let path = dir.join("numbers.dbs");

    let mut values = Arena::<u32>::with_chunk_rows(4);
    values.extend_from_slice(&[1, 2, 3]).unwrap();
    let mut writer = ColumnFileWriter::create(&path, WriterId::ANONYMOUS).unwrap();
    writer.column("values", &values).unwrap();
    writer.finish().unwrap();
    let whole = std::fs::metadata(&path).unwrap().len();
    let before = ColumnFile::open(&path).unwrap();

    let mut grown = before.column::<u32>("values").unwrap();
    grown.push(4).unwrap();
    let mut updater = before.update(WriterId::ANONYMOUS).unwrap();
    updater.column("values", &grown).unwrap();
    updater.commit().unwrap();
    assert_eq!(
        ColumnFile::open(&path).unwrap().generation(),
        before.generation() + 1
    );

    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(whole)
        .unwrap();

    let cut = ColumnFile::open(&path).unwrap();
    assert_eq!(cut.generation(), before.generation());
    assert_eq!(cut.column::<u32>("values").unwrap().len(), 3);
    cut.verify().unwrap();
}

#[test]
fn a_checked_open_falls_back_when_the_newest_commit_wrote_a_chunk_that_did_not_land() {
    let dir = TempDir::new("checked_open");
    let path = dir.join("numbers.dbs");

    let mut values = Arena::<u32>::with_chunk_rows(4);
    values.extend_from_slice(&[1, 2, 3, 4]).unwrap();
    let mut writer = ColumnFileWriter::create(&path, WriterId::ANONYMOUS).unwrap();
    writer.column("values", &values).unwrap();
    writer.finish().unwrap();
    let before = ColumnFile::open_checked(&path).unwrap();

    let mut grown = before.column::<u32>("values").unwrap();
    grown.extend_from_slice(&[5, 6, 7, 8]).unwrap();
    let mut updater = before.update(WriterId::ANONYMOUS).unwrap();
    updater.flushing(crate::storage::Flush::Once);
    updater.column("values", &grown).unwrap();
    let after = updater.commit_file().unwrap();
    let added = after.column_meta("values").unwrap().chunks[1];
    assert_eq!(
        ColumnFile::open_checked(&path).unwrap().generation(),
        after.generation()
    );

    flip_byte(&path, added.offset + added.physical_len() - 1);

    assert_eq!(
        ColumnFile::open(&path).unwrap().generation(),
        after.generation(),
        "an unchecked open trusts the slot and finds the damage when it reads"
    );
    let checked = ColumnFile::open_checked(&path).unwrap();
    assert_eq!(checked.generation(), before.generation());
    assert_eq!(checked.column::<u32>("values").unwrap().len(), 4);
    checked.verify().unwrap();
}
