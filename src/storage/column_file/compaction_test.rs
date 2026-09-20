use super::*;
use crate::test_utils::TempDir;
use crate::{Arena, WriterId};

fn seed(path: &std::path::Path) -> Result<ColumnFile> {
    let mut arena = Arena::<u64>::new();
    arena.extend_from_slice(&vec![0; 16384])?;
    let mut writer = ColumnFileWriter::create(path, WriterId::from_raw(1))?;
    writer.column("values", &arena)?;
    writer.finish()?;
    ColumnFile::open(path)
}

fn varied_seed(path: &std::path::Path, chunks: usize) -> Result<ColumnFile> {
    use rand::{Rng, SeedableRng};
    let mut rng = rand::rngs::StdRng::seed_from_u64(17);
    let mut arena = Arena::<u64>::new();
    for _ in 0..8192 * chunks {
        arena.push(rng.random())?;
    }
    let mut writer = ColumnFileWriter::create(path, WriterId::from_raw(1))?;
    writer.column("values", &arena)?;
    writer.finish()?;
    ColumnFile::open(path)
}

fn change(file: &ColumnFile, value: u64) -> Result<ColumnFile> {
    let mut arena = file.column::<u64>("values")?;
    arena.set(0, value)?;
    let mut updater = file.update(WriterId::from_raw(1))?;
    updater.column("values", &arena)?;
    updater.commit_file()
}

#[test]
fn sparse_updates_and_reverts_do_not_write_full_chunks() -> Result<()> {
    let dir = TempDir::new("sparse_delta");
    let file = seed(&dir.join("db"))?;
    let initial = file.storage_stats()?.file_bytes;
    let changed = change(&file, 1)?;
    assert!(changed.storage_stats()?.file_bytes - initial < 1024);
    let mut arena = changed.column::<u64>("values")?;
    arena.set(0, 2)?;
    arena.set(0, 1)?;
    let size = changed.storage_stats()?.file_bytes;
    let mut updater = changed.update(WriterId::from_raw(1))?;
    updater.column("values", &arena)?;
    let same = updater.commit_file()?;
    assert_eq!(same.commit_token(), changed.commit_token());
    assert_eq!(same.storage_stats()?.file_bytes, size);
    let mut rebuilt = Arena::new();
    rebuilt.extend_from_slice(
        &arena
            .iter()
            .map(|v| v.copied())
            .collect::<Result<Vec<_>>>()?,
    )?;
    let mut updater = same.update(WriterId::from_raw(1))?;
    updater.column("values", &rebuilt)?;
    assert_eq!(updater.commit_file()?.storage_stats()?.file_bytes, size);
    Ok(())
}

#[test]
fn reverting_a_delta_reuses_the_compressed_base() -> Result<()> {
    let dir = TempDir::new("compressed_revert");
    let path = dir.join("db");
    let original = varied_seed(&path, 2)?;
    let initial_value = *original.column::<u64>("values")?.row(0)?;
    let base = original.columns()[0].chunks[0];
    let changed = change(&original, 42)?;
    assert_eq!(changed.columns()[0].chunks[0].depth, 1);
    let reverted = change(&changed, initial_value)?;
    assert_eq!(reverted.columns()[0].chunks[0], base);
    assert_eq!(reverted.content_id(), original.content_id());
    assert_eq!(
        reverted.columns()[0].chunks[1],
        original.columns()[0].chunks[1]
    );
    ColumnFile::open(&path)?.verify()?;
    Ok(())
}

#[test]
fn a_compressed_full_chunk_can_be_smaller_than_a_delta() -> Result<()> {
    let dir = TempDir::new("compressed_full");
    let original = seed(&dir.join("db"))?;
    let stats = original.storage_stats()?;
    assert!(stats.file_bytes < stats.retained_value_bytes / 4);
    assert_eq!(stats.consolidated_bytes, stats.file_bytes);
    assert_eq!(stats.obsolete_bytes, 0);
    let changed = change(&original, 42)?;
    let loc = changed.columns()[0].chunks[0];
    assert_eq!(loc.depth, 0);
    assert!(loc.physical_len() < 100);
    changed.verify()?;
    Ok(())
}

#[test]
fn consolidation_bounds_chains_and_compaction_preserves_snapshots() -> Result<()> {
    let dir = TempDir::new("compact_snapshot");
    let path = dir.join("db");
    let snapshot = seed(&path)?;
    let mut file = snapshot.clone();
    for value in 1..=30 {
        file = change(&file, value)?;
        assert!(file.columns()[0].chunks[0].depth <= super::super::delta::MAX_DEPTH);
        assert!(file.directory_loc()?.depth <= super::super::delta::MAX_DEPTH);
    }
    let token = file.commit_token();
    let stats = file.storage_stats()?;
    assert!(stats.obsolete_bytes > 0);
    let (packed, report) = file.compact()?;
    assert!(report.reclaimed_bytes() > 0);
    assert_eq!(packed.storage_stats()?.obsolete_bytes, 0);
    assert_eq!(report.after_bytes, stats.consolidated_bytes);
    assert_eq!(packed.content_id(), file.content_id());
    assert_ne!(packed.file_uuid(), token.file_uuid);
    assert_eq!(*packed.column::<u64>("values")?.row(0)?, 30);
    assert_eq!(*snapshot.column::<u64>("values")?.row(0)?, 0);
    assert_eq!(*file.column::<u64>("values")?.row(0)?, 30);
    assert!(matches!(
        file.update(WriterId::from_raw(1)),
        Err(Error::Conflict { .. })
    ));
    let latest = change(&packed, 31)?;
    latest.verify()?;
    Ok(())
}

#[test]
fn replacement_failures_leave_a_verified_published_file() -> Result<()> {
    let dir = TempDir::new("compact_failure");
    let path = dir.join("db");
    let file = change(&varied_seed(&path, 2)?, 42)?;
    FAULT.set(Some("before_replace"));
    assert!(file.compact().is_err());
    assert_eq!(ColumnFile::open(&path)?.commit_token(), file.commit_token());
    FAULT.set(Some("after_replace"));
    let err = file.compact().unwrap_err();
    let Error::CompactionUncertain { attempted, .. } = err else {
        panic!("{err}");
    };
    let recovered = ColumnFile::open(&path)?;
    assert_eq!(recovered.commit_token(), attempted);
    recovered.verify()?;
    assert_eq!(*recovered.column::<u64>("values")?.row(0)?, 42);
    assert_eq!(std::fs::read_dir(path.parent().unwrap())?.count(), 1);
    Ok(())
}

#[test]
fn dense_updates_are_full_and_corrupt_delta_bases_are_rejected() -> Result<()> {
    let dir = TempDir::new("delta_corruption");
    let path = dir.join("db");
    let file = seed(&path)?;
    let mut arena = file.column::<u64>("values")?;
    for row in 0..8192 {
        arena.set(row, row as u64)?;
    }
    let mut updater = file.update(WriterId::from_raw(1))?;
    updater.column("values", &arena)?;
    let dense = updater.commit_file()?;
    assert_eq!(dense.columns()[0].chunks[0].depth, 0);
    let sparse = change(&dense, 8)?;
    assert_eq!(sparse.columns()[0].chunks[0].depth, 1);
    let base = dense.columns()[0].chunks[0];
    crate::test_utils::flip_byte(&path, base.offset);
    assert!(sparse.verify().is_err());
    assert!(sparse.compact().is_err());
    Ok(())
}

#[test]
fn every_byte_of_a_chunk_delta_is_checksummed_before_it_is_applied() -> Result<()> {
    let dir = TempDir::new("chunk_delta_checksum");
    let path = dir.join("db");
    let file = change(&varied_seed(&path, 2)?, 42)?;
    let loc = file.columns()[0].chunks[0];
    assert_eq!(loc.depth, 1);
    for byte in 0..loc.physical_len() {
        crate::test_utils::flip_byte(&path, loc.offset + byte);
        assert!(file.verify().is_err(), "delta byte {byte}");
        assert!(
            file.column::<u64>("values")?.row(0).is_err(),
            "lazy read of delta byte {byte}"
        );
        crate::test_utils::flip_byte(&path, loc.offset + byte);
    }
    file.verify()?;
    Ok(())
}

#[test]
fn directory_deltas_and_their_bases_are_checksummed_on_open() -> Result<()> {
    let dir = TempDir::new("directory_delta_checksum");
    let path = dir.join("db");
    let original = varied_seed(&path, 64)?;
    let changed = change(&original, 42)?;
    let loc = changed.directory_loc()?;
    assert_eq!(loc.depth, 1);
    for byte in 0..loc.physical_len() {
        crate::test_utils::flip_byte(&path, loc.offset + byte);
        assert!(
            ColumnFile::open(&path).is_err(),
            "directory delta byte {byte}"
        );
        assert!(changed.verify().is_err());
        crate::test_utils::flip_byte(&path, loc.offset + byte);
    }
    let base = original.directory_loc()?;
    crate::test_utils::flip_byte(&path, base.offset + 4);
    assert!(ColumnFile::open(&path).is_err());
    crate::test_utils::flip_byte(&path, base.offset + 4);
    ColumnFile::open(&path)?.verify()?;
    Ok(())
}

#[test]
fn reconstructed_bytes_are_verified_even_when_the_delta_checksum_matches() -> Result<()> {
    use crate::storage::column_file::{compression, file_source::write_all_at};
    let dir = TempDir::new("delta_result_checksum");
    let path = dir.join("db");
    let file = change(&varied_seed(&path, 2)?, 42)?;
    let mut loc = file.columns()[0].chunks[0];
    assert_eq!(loc.depth, 1);
    let mut bytes = file.file_source().read_payload(loc)?;
    // Change a replacement value, then repair both the delta and zstd checksums.
    let last_payload = bytes.len() - 9;
    bytes[last_payload] ^= 1;
    let checksum_at = bytes.len() - 8;
    let checksum = crate::fnv1a(&bytes[..checksum_at]);
    bytes[checksum_at..].copy_from_slice(&checksum.to_le_bytes());
    let payload = compression::encode(&bytes, loc.byte_len, loc.depth)?;
    loc.offset = file.file_bytes()?;
    loc.stored_len = payload.len() as u32;
    let handle = OpenOptions::new().write(true).open(&path)?;
    write_all_at(&handle, &payload, loc.offset)?;
    let err = file.file_source().read_chunk(loc).unwrap_err();
    assert!(err.to_string().contains("failed checksum"));
    Ok(())
}

#[cfg(feature = "macros")]
#[test]
fn a_failed_automatic_compaction_does_not_fail_the_committed_write() -> Result<()> {
    use crate::schema::Store;
    use crate::test_utils::library::Library;
    let dir = TempDir::new("automatic_compaction_failure");
    let path = dir.join("db");
    let writer = WriterId::from_raw(1);
    let mut db = Library::create(&path, writer)?;
    db.set_compaction_policy(CompactionPolicy::Automatic {
        min_reclaim_bytes: 0,
        min_reclaim_fraction: 0.0,
    })?;
    let id = db.books.push("before", "author")?;
    FAULT.set(Some("before_replace"));
    db.commit(writer)?;
    assert!(matches!(
        db.maintenance_outcome(),
        Some(MaintenanceOutcome::Failed(_))
    ));
    assert_eq!(Library::open(&path)?.books.title(id)?, "before");
    FAULT.set(Some("after_replace"));
    db.books.write_over(id, "after", "author")?;
    db.commit(writer)?;
    assert!(matches!(
        db.maintenance_outcome(),
        Some(MaintenanceOutcome::Failed(_))
    ));
    assert_eq!(Library::open(&path)?.books.title(id)?, "after");
    db.commit(writer)?;
    db.verify()?;
    Ok(())
}

#[test]
fn concurrent_writers_and_compaction_do_not_lose_updates() -> Result<()> {
    let dir = TempDir::new("concurrent_compaction");
    let path = dir.join("db");
    seed(&path)?;
    let start = Arc::new(std::sync::Barrier::new(3));
    let mut workers = Vec::new();
    for worker in 0..3 {
        let path = path.clone();
        let start = start.clone();
        workers.push(std::thread::spawn(move || -> Result<()> {
            start.wait();
            for _ in 0..8 {
                loop {
                    let file = ColumnFile::open(&path)?;
                    let result = if worker == 2 {
                        file.compact().map(|_| ())
                    } else {
                        let value = *file.column::<u64>("values")?.row(0)?;
                        change(&file, value + 1).map(|_| ())
                    };
                    match result {
                        Ok(()) => break,
                        Err(Error::Conflict { .. }) => continue,
                        Err(err) => return Err(err),
                    }
                }
            }
            Ok(())
        }));
    }
    for worker in workers {
        worker.join().unwrap()?;
    }
    let file = ColumnFile::open(path)?;
    file.verify()?;
    assert_eq!(*file.column::<u64>("values")?.row(0)?, 16);
    Ok(())
}
