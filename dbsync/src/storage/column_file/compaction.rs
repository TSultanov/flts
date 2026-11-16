use crate::file_lock::FileLock;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::Arc;

use super::column_meta::write_directory;
use super::file_source::FileSource;
use super::{ColumnFile, ColumnFileUpdater, ColumnFileWriter, HEAD_LEN};
use crate::{ChunkLoc, ChunkSource, Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageStats {
    pub file_bytes: u64,
    pub reachable_bytes: u64,
    pub obsolete_bytes: u64,
    /// Compressed rewrite size, excluding savings from schema-level heap cleanup.
    pub consolidated_bytes: u64,
    /// Logical value bytes, before compression.
    pub retained_value_bytes: u64,
    /// Logical heap bytes that schema-level compaction can remove.
    pub unused_heap_bytes: u64,
}

impl StorageStats {
    pub fn estimated_savings(self) -> u64 {
        self.file_bytes.saturating_sub(self.consolidated_bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompactionReport {
    pub before_bytes: u64,
    pub after_bytes: u64,
}

impl CompactionReport {
    pub fn reclaimed_bytes(self) -> u64 {
        self.before_bytes.saturating_sub(self.after_bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CompactionPolicy {
    Disabled,
    Automatic {
        min_reclaim_bytes: u64,
        min_reclaim_fraction: f64,
    },
}

impl CompactionPolicy {
    pub fn validate(self) -> Result<()> {
        if let Self::Automatic {
            min_reclaim_fraction,
            ..
        } = self
            && (!min_reclaim_fraction.is_finite() || !(0.0..=1.0).contains(&min_reclaim_fraction))
        {
            return Err(Error::invalid(
                "compaction fraction must be between zero and one",
            ));
        }
        Ok(())
    }

    /// Decides from the bytes nothing refers to any more. That is what a
    /// rewrite surely reclaims, and unlike its exact size it costs no chunk
    /// reads, so it can be asked on every commit.
    pub fn should_compact(self, file_bytes: u64, obsolete_bytes: u64) -> bool {
        match self {
            Self::Disabled => false,
            Self::Automatic {
                min_reclaim_bytes,
                min_reclaim_fraction,
            } => {
                obsolete_bytes > 0
                    && obsolete_bytes >= min_reclaim_bytes
                    && obsolete_bytes as f64 >= file_bytes as f64 * min_reclaim_fraction
            }
        }
    }
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        // compaction_bench: reclaiming 1 MiB cost about as much as 64 KiB.
        // Requiring half the file to be reclaimable bounds rewrite bytes per byte saved.
        Self::Automatic {
            min_reclaim_bytes: 1024 * 1024,
            min_reclaim_fraction: 0.5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaintenanceOutcome {
    Compacted(CompactionReport),
    Failed(String),
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl ColumnFile {
    /// Bytes the directory still leads to. Reads only the delta headers.
    fn reachable_bytes(&self) -> Result<u64> {
        let mut ranges = BTreeMap::new();
        collect(self.file_source(), self.directory_loc()?, &mut ranges)?;
        for column in self.columns() {
            for &loc in &column.chunks {
                collect(self.file_source(), loc, &mut ranges)?;
            }
        }
        let mut reachable_bytes = HEAD_LEN;
        let mut end = HEAD_LEN;
        for (start, stop) in ranges {
            reachable_bytes += stop.saturating_sub(start.max(end));
            end = end.max(stop);
        }
        Ok(reachable_bytes)
    }

    /// Bytes of superseded chunks: what a rewrite reclaims.
    pub fn obsolete_bytes(&self) -> Result<u64> {
        Ok(self.file_bytes()?.saturating_sub(self.reachable_bytes()?))
    }

    /// Exact, and as costly as a rewrite: every chunk is read and re-encoded.
    pub fn storage_stats(&self) -> Result<StorageStats> {
        let reachable_bytes = self.reachable_bytes()?;
        let file_bytes = self.file_source().len()?;
        let retained_value_bytes = self.live_bytes();
        let mut consolidated = self.columns().to_vec();
        let mut cursor = HEAD_LEN;
        for column in &mut consolidated {
            for loc in &mut column.chunks {
                let bytes = self.file_source().read_chunk(*loc)?;
                let payload = super::compression::encode(&bytes, loc.byte_len, 0)?;
                loc.offset = cursor;
                loc.depth = 0;
                loc.stored_len = payload.len() as u32;
                cursor += loc.physical_len();
            }
        }
        let directory = write_directory(&consolidated);
        let byte_len =
            u32::try_from(directory.len()).map_err(|_| Error::invalid("directory exceeds u32"))?;
        let consolidated_bytes =
            cursor + super::compression::encode(&directory, byte_len, 0)?.len() as u64;
        Ok(StorageStats {
            file_bytes,
            reachable_bytes,
            obsolete_bytes: file_bytes.saturating_sub(reachable_bytes),
            consolidated_bytes,
            retained_value_bytes,
            unused_heap_bytes: 0,
        })
    }

    pub fn compact(&self) -> Result<(Self, CompactionReport)> {
        self.rewrite(
            |update| {
                for column in self.columns() {
                    update.copy_column(column, self.file_source().as_ref())?;
                }
                Ok(())
            },
            |_| Ok(()),
        )
    }

    pub(crate) fn rewrite(
        &self,
        fill: impl FnOnce(&mut ColumnFileUpdater) -> Result<()>,
        validate: impl FnOnce(&ColumnFile) -> Result<()>,
    ) -> Result<(Self, CompactionReport)> {
        self.rewritten(true, fill, validate)
    }

    /// A rewrite that reads no more of either file than a commit would.
    pub(crate) fn replace(
        &self,
        fill: impl FnOnce(&mut ColumnFileUpdater) -> Result<()>,
    ) -> Result<Self> {
        Ok(self.rewritten(false, fill, |_| Ok(()))?.0)
    }

    fn rewritten(
        &self,
        checked: bool,
        fill: impl FnOnce(&mut ColumnFileUpdater) -> Result<()>,
        validate: impl FnOnce(&ColumnFile) -> Result<()>,
    ) -> Result<(Self, CompactionReport)> {
        let path = self.file_source().path();
        let original = OpenOptions::new().read(true).write(true).open(path)?;
        original.lock_exclusive()?;
        let found = Self::read_commit_token(path)?;
        if found != self.commit_token() {
            return Err(Error::Conflict {
                expected: self.commit_token(),
                found,
            });
        }
        if checked {
            self.verify()?;
        }
        let before_bytes = original.metadata()?.len();
        let parent = path
            .parent()
            .ok_or_else(|| Error::invalid("file has no parent directory"))?;
        let temp_path = parent.join(format!(".dbsync-{}.tmp", crate::Uuid::now_v7()));
        let handle = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        let temp = Temporary(temp_path);
        handle.set_permissions(original.metadata()?.permissions())?;
        ColumnFileWriter::new(handle, self.writer_id())?.finish()?;
        let fresh = Self::open(&temp.0)?;
        let mut update = fresh.update(self.writer_id())?;
        update.prepare_rewrite(self.generation())?;
        fill(&mut update)?;
        let ready = update.commit_file()?;
        if checked {
            ready.verify()?;
        }
        validate(&ready)?;
        let replacement = OpenOptions::new().read(true).write(true).open(&temp.0)?;
        replacement.lock_exclusive()?;
        replacement.sync_all()?;
        let after_bytes = replacement.metadata()?.len();
        compaction_fault("before_replace")?;
        let published = Self::from_source(Arc::new(FileSource::from_file(
            replacement.try_clone()?,
            path.to_owned(),
        )))?;
        std::fs::rename(&temp.0, path)?;
        let sync = (|| -> std::io::Result<()> {
            compaction_fault("after_replace")?;
            #[cfg(unix)]
            File::open(parent)?.sync_all()?;
            compaction_fault("before_unlock")?;
            replacement.release_lock()?;
            Ok(())
        })();
        if let Err(source) = sync {
            return Err(Error::CompactionUncertain {
                expected: self.commit_token(),
                attempted: published.commit_token(),
                source,
            });
        }
        Ok((
            published,
            CompactionReport {
                before_bytes,
                after_bytes,
            },
        ))
    }
}

fn collect(source: &FileSource, loc: ChunkLoc, ranges: &mut BTreeMap<u64, u64>) -> Result<()> {
    if loc.depth > super::delta::MAX_DEPTH {
        return Err(Error::corrupt("delta", "chain too deep"));
    }
    let end = loc
        .offset
        .checked_add(loc.physical_len())
        .filter(|end| *end <= source.len().unwrap_or(0))
        .ok_or_else(|| Error::corrupt("chunk", "invalid extent"))?;
    if ranges.insert(loc.offset, end).is_some() {
        return Ok(());
    }
    if loc.depth > 0 {
        let bytes = source.read_payload(loc)?;
        let (base, _) = super::delta::parse(loc, &bytes)?;
        collect(source, base, ranges)?;
    }
    Ok(())
}

#[cfg(test)]
thread_local! { static FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; }

#[cfg(all(test, feature = "macros"))]
pub(crate) fn fail_rewrite_at(point: &'static str) {
    FAULT.set(Some(point));
}

fn compaction_fault(_point: &str) -> std::io::Result<()> {
    #[cfg(test)]
    if FAULT.get() == Some(_point) {
        FAULT.set(None);
        return Err(std::io::Error::other("injected compaction failure"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "compaction_test.rs"]
mod tests;
