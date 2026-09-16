use std::path::{Path, PathBuf};

use super::*;
use crate::storage::ColumnFileWriter;
use crate::storage::arena::Arena;
use crate::storage::column_file::HEAD_LEN;
use crate::test_utils::{TempDir, flip_byte};

fn this_device() -> WriterId {
    WriterId::from_device_id("KRZH3XY-7PH4NJK-2WQ5LMC")
}

fn seed(path: &Path, values: &[u32]) {
    let mut column = Arena::<u32>::with_chunk_rows(4);
    column.extend_from_slice(values);
    let mut writer = ColumnFileWriter::create(path, this_device()).unwrap();
    writer.column("values", &column).unwrap();
    writer.finish().unwrap();
}

fn len_of(path: &PathBuf) -> u64 {
    std::fs::metadata(path).unwrap().len()
}

fn read_all(path: &Path) -> Vec<u32> {
    let file = ColumnFile::open(path).unwrap();
    let column = file.column::<u32>("values").unwrap();
    column
        .try_iter()
        .map(|row| row.copied())
        .collect::<io::Result<Vec<_>>>()
        .unwrap()
}

#[test]
fn editing_one_row_appends_one_chunk_even_after_the_column_was_read() {
    let dir = TempDir::new("reuse");
    let path = dir.join("values.dbs");
    seed(&path, &(0..10).collect::<Vec<u32>>());

    let payload = 10 * 4;
    let directory_len = len_of(&path) - HEAD_LEN - payload;

    let file = ColumnFile::open(&path).unwrap();
    let mut column = file.column::<u32>("values").unwrap();
    column.prefetch().unwrap();
    assert_eq!(column.resident_chunks(), 3, "the whole column was read");

    column.try_set(5, 555).unwrap();
    let before = len_of(&path);
    let mut updater = file.update(this_device()).unwrap();
    updater.column("values", &column).unwrap();
    assert_eq!(updater.commit().unwrap(), 2);

    let one_chunk = 4 * 4;
    assert_eq!(
        len_of(&path) - before,
        one_chunk + directory_len,
        "only the edited chunk and a new directory were appended"
    );
    assert_eq!(read_all(&path), vec![0, 1, 2, 3, 4, 555, 6, 7, 8, 9]);
}

#[test]
fn a_reader_keeps_its_generation_after_later_commits() {
    let dir = TempDir::new("mvcc");
    let path = dir.join("values.dbs");
    seed(&path, &(0..10).collect::<Vec<u32>>());

    let reader = ColumnFile::open(&path).unwrap();
    let snapshot = reader.column::<u32>("values").unwrap();
    assert_eq!(reader.generation(), 1);

    for (generation, value) in [(2u64, 111u32), (3, 222)] {
        let live = ColumnFile::open(&path).unwrap();
        let mut column = live.column::<u32>("values").unwrap();
        column.try_set(5, value).unwrap();
        let mut updater = live.update(this_device()).unwrap();
        updater.column("values", &column).unwrap();
        assert_eq!(updater.commit().unwrap(), generation);
    }

    snapshot.prefetch().unwrap();
    assert_eq!(
        snapshot
            .try_iter()
            .map(|row| row.copied())
            .collect::<io::Result<Vec<_>>>()
            .unwrap(),
        (0..10).collect::<Vec<u32>>(),
        "the snapshot still serves generation 1"
    );
    assert_eq!(read_all(&path), vec![0, 1, 2, 3, 4, 222, 6, 7, 8, 9]);
}

#[test]
fn a_torn_newest_slot_falls_back_to_the_previous_generation() {
    let dir = TempDir::new("torn");
    let path = dir.join("values.dbs");
    seed(&path, &(0..10).collect::<Vec<u32>>());

    let file = ColumnFile::open(&path).unwrap();
    let mut column = file.column::<u32>("values").unwrap();
    column.try_set(0, 999).unwrap();
    let mut updater = file.update(this_device()).unwrap();
    updater.column("values", &column).unwrap();
    assert_eq!(updater.commit().unwrap(), 2);
    assert_eq!(read_all(&path)[0], 999);

    flip_byte(&path, SLOT_B);

    let recovered = ColumnFile::open(&path).unwrap();
    assert_eq!(recovered.generation(), 1);
    recovered.verify().unwrap();
    assert_eq!(read_all(&path), (0..10).collect::<Vec<u32>>());
}

#[test]
fn a_stale_handle_cannot_commit_over_a_newer_generation() {
    let dir = TempDir::new("cas");
    let path = dir.join("values.dbs");
    seed(&path, &(0..10).collect::<Vec<u32>>());

    let stale = ColumnFile::open(&path).unwrap();
    let stale_column = stale.column::<u32>("values").unwrap();

    let live = ColumnFile::open(&path).unwrap();
    let mut column = live.column::<u32>("values").unwrap();
    column.try_set(1, 111).unwrap();
    let mut updater = live.update(this_device()).unwrap();
    updater.column("values", &column).unwrap();
    updater.commit().unwrap();

    let err = stale.update(this_device()).unwrap_err();
    assert!(err.to_string().contains("moved to generation 2"), "{err}");
    drop(stale_column);
}

#[test]
fn a_commit_that_changes_nothing_does_not_grow_the_file() {
    let dir = TempDir::new("noop");
    let path = dir.join("values.dbs");
    seed(&path, &(0..10).collect::<Vec<u32>>());
    let before = len_of(&path);

    let file = ColumnFile::open(&path).unwrap();
    let column = file.column::<u32>("values").unwrap();
    let mut updater = file.update(this_device()).unwrap();
    updater.column("values", &column).unwrap();
    assert_eq!(updater.commit().unwrap(), 1, "generation is not bumped");

    assert_eq!(len_of(&path), before);
    assert_eq!(ColumnFile::open(&path).unwrap().generation(), 1);
}

#[test]
fn an_arena_from_another_file_is_re_encoded_rather_than_reused() {
    let dir = TempDir::new("identity");
    let ours = dir.join("ours.dbs");
    let theirs = dir.join("theirs.dbs");
    seed(&ours, &(0..10).collect::<Vec<u32>>());
    seed(&theirs, &(100..110).collect::<Vec<u32>>());

    let foreign = ColumnFile::open(&theirs).unwrap();
    let foreign_column = foreign.column::<u32>("values").unwrap();

    let file = ColumnFile::open(&ours).unwrap();
    let mut updater = file.update(this_device()).unwrap();
    updater.column("values", &foreign_column).unwrap();
    assert_eq!(updater.commit().unwrap(), 2);

    assert_eq!(read_all(&ours), (100..110).collect::<Vec<u32>>());
    ColumnFile::open(&ours).unwrap().verify().unwrap();
}

#[test]
fn a_commit_that_would_drop_a_column_is_refused() {
    let dir = TempDir::new("drop");
    let path = dir.join("values.dbs");

    let mut values = Arena::<u32>::with_chunk_rows(4);
    values.extend_from_slice(&[1, 2, 3]);
    let mut spare = Arena::<u32>::with_chunk_rows(4);
    spare.extend_from_slice(&[9]);
    let mut writer = ColumnFileWriter::create(&path, this_device()).unwrap();
    writer.column("values", &values).unwrap();
    writer.column("spare", &spare).unwrap();
    writer.finish().unwrap();

    let file = ColumnFile::open(&path).unwrap();
    let column = file.column::<u32>("values").unwrap();
    let mut updater = file.update(this_device()).unwrap();
    updater.column("values", &column).unwrap();
    let err = updater.commit().unwrap_err();
    assert!(err.to_string().contains("would drop columns"), "{err}");

    let file = ColumnFile::open(&path).unwrap();
    let column = file.column::<u32>("values").unwrap();
    let mut updater = file.update(this_device()).unwrap();
    updater.remove_column("spare");
    updater.column("values", &column).unwrap();
    assert_eq!(updater.commit().unwrap(), 2);
    assert!(
        ColumnFile::open(&path)
            .unwrap()
            .column_meta("spare")
            .is_none()
    );
}

#[test]
fn a_column_cannot_change_its_chunking() {
    let dir = TempDir::new("chunking");
    let path = dir.join("values.dbs");
    seed(&path, &(0..10).collect::<Vec<u32>>());

    let mut rechunked = Arena::<u32>::with_chunk_rows(8);
    rechunked.extend_from_slice(&(0..10).collect::<Vec<u32>>());

    let file = ColumnFile::open(&path).unwrap();
    let mut updater = file.update(this_device()).unwrap();
    let err = updater.column("values", &rechunked).unwrap_err();
    assert!(
        err.to_string().contains("chunked 4 rows at a time"),
        "{err}"
    );
}

#[test]
fn content_id_identifies_the_data_not_its_layout() {
    let dir = TempDir::new("content_id");
    let straight = dir.join("straight.dbs");
    let detoured = dir.join("detoured.dbs");

    seed(&straight, &(0..10).collect::<Vec<u32>>());

    let mut detour = (0..10).collect::<Vec<u32>>();
    detour[5] = 555;
    seed(&detoured, &detour);

    let file = ColumnFile::open(&detoured).unwrap();
    let mut column = file.column::<u32>("values").unwrap();
    column.try_set(5, 5).unwrap();
    let mut updater = file.update(this_device()).unwrap();
    updater.column("values", &column).unwrap();
    updater.commit().unwrap();

    let a = ColumnFile::open(&straight).unwrap();
    let b = ColumnFile::open(&detoured).unwrap();

    assert_eq!(read_all(&straight), read_all(&detoured));
    assert_eq!(
        a.content_id(),
        b.content_id(),
        "same rows, so the same content id"
    );
    assert_ne!(
        a.column_meta("values").unwrap().chunks[1].offset,
        b.column_meta("values").unwrap().chunks[1].offset,
        "but the edited chunk landed somewhere else"
    );
    assert_ne!(a.generation(), b.generation());
}

#[test]
fn a_commit_leaves_every_earlier_chunk_byte_untouched() {
    let dir = TempDir::new("append_only");
    let path = dir.join("values.dbs");
    seed(&path, &(0..10).collect::<Vec<u32>>());

    let before = std::fs::read(&path).unwrap();
    let live_end = before.len() as u64;

    let file = ColumnFile::open(&path).unwrap();
    let mut column = file.column::<u32>("values").unwrap();
    column.try_set(9, 999).unwrap();
    let mut updater = file.update(this_device()).unwrap();
    updater.column("values", &column).unwrap();
    updater.commit().unwrap();

    let after = std::fs::read(&path).unwrap();
    assert_eq!(
        &before[HEAD_LEN as usize..],
        &after[HEAD_LEN as usize..live_end as usize],
        "everything the previous generation wrote is still byte for byte intact"
    );
}

#[test]
fn every_commit_records_who_wrote_it() {
    let dir = TempDir::new("writer_id");
    let path = dir.join("values.dbs");
    seed(&path, &(0..10).collect::<Vec<u32>>());
    assert_eq!(ColumnFile::open(&path).unwrap().writer_id(), this_device());

    let peer = WriterId::from_device_id("PEERDEV-7PH4NJK-2WQ5LMC");
    let file = ColumnFile::open(&path).unwrap();
    let mut column = file.column::<u32>("values").unwrap();
    column.try_set(0, 1).unwrap();
    let mut updater = file.update(peer).unwrap();
    updater.column("values", &column).unwrap();
    updater.commit().unwrap();

    let after = ColumnFile::open(&path).unwrap();
    assert_eq!(after.writer_id(), peer);
    assert_ne!(after.writer_id(), this_device());
}

#[test]
fn a_lost_slot_cannot_make_a_writer_overwrite_a_live_reader() {
    let dir = TempDir::new("append_mark");
    let path = dir.join("values.dbs");
    seed(&path, &(0..10).collect::<Vec<u32>>());

    let file = ColumnFile::open(&path).unwrap();
    let mut column = file.column::<u32>("values").unwrap();
    column.try_set(5, 555).unwrap();
    let mut updater = file.update(this_device()).unwrap();
    updater.column("values", &column).unwrap();
    assert_eq!(updater.commit().unwrap(), 2);

    let reader = ColumnFile::open(&path).unwrap();
    assert_eq!(reader.generation(), 2);
    let snapshot = reader.column::<u32>("values").unwrap();
    assert_eq!(snapshot.resident_chunks(), 0);

    flip_byte(&path, SLOT_B);
    let fallen_back = ColumnFile::open(&path).unwrap();
    assert_eq!(fallen_back.generation(), 1, "the newest commit is gone");

    let mut column = fallen_back.column::<u32>("values").unwrap();
    column.try_set(0, 111).unwrap();
    let mut updater = fallen_back.update(this_device()).unwrap();
    updater.column("values", &column).unwrap();
    updater.commit().unwrap();

    snapshot.prefetch().unwrap();
    assert_eq!(
        snapshot
            .try_iter()
            .map(|row| row.copied())
            .collect::<io::Result<Vec<_>>>()
            .unwrap(),
        vec![0, 1, 2, 3, 4, 555, 6, 7, 8, 9],
        "generation 2's chunks were not written over"
    );
    assert_eq!(read_all(&path)[0], 111);
}

#[test]
fn an_abandoned_commit_does_not_permanently_waste_space() {
    let dir = TempDir::new("reclaim");
    let clean = dir.join("clean.dbs");
    let abandoned = dir.join("abandoned.dbs");
    seed(&clean, &(0..10).collect::<Vec<u32>>());
    seed(&abandoned, &(0..10).collect::<Vec<u32>>());

    {
        let file = ColumnFile::open(&abandoned).unwrap();
        let mut column = file.column::<u32>("values").unwrap();
        column.try_set(0, 111).unwrap();
        let mut updater = file.update(this_device()).unwrap();
        updater.column("values", &column).unwrap();
    }
    assert!(
        len_of(&abandoned) > len_of(&clean),
        "the abandoned payload reached the file"
    );
    assert_eq!(ColumnFile::open(&abandoned).unwrap().generation(), 1);

    for path in [&clean, &abandoned] {
        let file = ColumnFile::open(path).unwrap();
        let mut column = file.column::<u32>("values").unwrap();
        column.try_set(0, 222).unwrap();
        let mut updater = file.update(this_device()).unwrap();
        updater.column("values", &column).unwrap();
        updater.commit().unwrap();
    }

    assert_eq!(
        len_of(&clean),
        len_of(&abandoned),
        "the abandoned bytes were written over, not skipped"
    );
}

#[test]
fn losing_the_mark_and_the_slot_together_still_cannot_clobber_a_reader() {
    let dir = TempDir::new("eof_floor");
    let path = dir.join("values.dbs");
    seed(&path, &(0..10).collect::<Vec<u32>>());

    let file = ColumnFile::open(&path).unwrap();
    let mut column = file.column::<u32>("values").unwrap();
    column.try_set(5, 555).unwrap();
    let mut updater = file.update(this_device()).unwrap();
    updater.column("values", &column).unwrap();
    assert_eq!(updater.commit().unwrap(), 2);

    let reader = ColumnFile::open(&path).unwrap();
    assert_eq!(reader.generation(), 2);
    let snapshot = reader.column::<u32>("values").unwrap();
    assert_eq!(snapshot.resident_chunks(), 0);

    flip_byte(&path, SLOT_B);
    flip_byte(&path, APPEND_MARK);

    let fallen_back = ColumnFile::open(&path).unwrap();
    assert_eq!(fallen_back.generation(), 1);

    let mut column = fallen_back.column::<u32>("values").unwrap();
    column.try_set(0, 111).unwrap();
    let mut updater = fallen_back.update(this_device()).unwrap();
    updater.column("values", &column).unwrap();
    updater.commit().unwrap();

    snapshot.prefetch().unwrap();
    assert_eq!(
        snapshot
            .try_iter()
            .map(|row| row.copied())
            .collect::<io::Result<Vec<_>>>()
            .unwrap(),
        vec![0, 1, 2, 3, 4, 555, 6, 7, 8, 9],
        "the file length floored the cursor above generation 2"
    );
}

#[test]
fn a_reused_generation_number_is_still_a_conflict() {
    let dir = TempDir::new("reused_generation");
    let path = dir.join("values.dbs");
    seed(&path, &(0..10).collect::<Vec<u32>>());

    let first = ColumnFile::open(&path).unwrap();
    let mut column = first.column::<u32>("values").unwrap();
    column.try_set(5, 555).unwrap();
    let mut updater = first.update(this_device()).unwrap();
    updater.column("values", &column).unwrap();
    assert_eq!(updater.commit().unwrap(), 2);

    let held = ColumnFile::open(&path).unwrap();
    assert_eq!(held.generation(), 2);

    flip_byte(&path, SLOT_B);

    let rival = ColumnFile::open(&path).unwrap();
    assert_eq!(rival.generation(), 1);
    let mut column = rival.column::<u32>("values").unwrap();
    column.try_set(0, 111).unwrap();
    let mut updater = rival.update(this_device()).unwrap();
    updater.column("values", &column).unwrap();
    assert_eq!(updater.commit().unwrap(), 2, "the number gets reused");

    let current = ColumnFile::open(&path).unwrap();
    assert_eq!(current.generation(), held.generation());
    assert_ne!(
        current.content_id(),
        held.content_id(),
        "same number, different commit"
    );

    let err = held.update(this_device()).unwrap_err();
    assert!(err.to_string().contains("moved to generation 2"), "{err}");
}
