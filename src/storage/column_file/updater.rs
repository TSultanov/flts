//! Appends changed chunks to an existing file and publishes them as a generation.

use std::fs::{File, OpenOptions};
use std::sync::Arc;

use super::append_mark::{AppendMark, read_append_mark};
use super::codec::Codec;
use super::column_meta::{ColumnMeta, content_id, write_directory};
use super::commit_slot::{CommitSlot, newest_slot};
use super::commit_token::CommitToken;
use super::file_source::FileSource;
use super::file_source::write_all_at;
use super::reader::ColumnFile;
use super::writer_id::WriterId;
use super::{APPEND_MARK, HEAD_LEN, SLOT_A, SLOT_B};
use crate::error::{Error, Result};
use crate::record::Record;
use crate::storage::arena::{Arena, ChunkLoc, ChunkSource};
use crate::storage::hash::fnv1a;

trait CommitIo: std::borrow::Borrow<File> {
    fn write_payload(&self, bytes: &[u8], offset: u64) -> Result<()> {
        write_all_at(self.borrow(), bytes, offset)
    }

    fn sync_directory(&self) -> std::io::Result<()> {
        self.borrow().sync_data()
    }

    fn prepare_publication(&self) -> std::io::Result<File> {
        self.borrow().try_clone()
    }

    fn write_slot(&self, bytes: &[u8], offset: u64) -> Result<()> {
        write_all_at(self.borrow(), bytes, offset)
    }

    fn sync_slot(&self) -> std::io::Result<()> {
        self.borrow().sync_data()
    }

    fn write_mark(&self, bytes: &[u8]) -> Result<()> {
        write_all_at(self.borrow(), bytes, APPEND_MARK)
    }
}

#[cfg(not(test))]
impl CommitIo for File {}

#[cfg(all(test, feature = "derive"))]
pub(crate) use updater_test::{CommitFault, fail_commit_at, fail_commit_times};

impl std::fmt::Debug for ColumnFileUpdater {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColumnFileUpdater")
            .field("generation", &self.expected_generation)
            .field("cursor", &self.cursor)
            .field("columns", &self.columns.len())
            .finish()
    }
}

pub struct ColumnFileUpdater {
    file: File,
    source: Arc<dyn ChunkSource>,
    file_uuid: u128,
    writer_id: WriterId,
    expected_generation: u64,
    target_slot: u64,
    cursor: u64,
    previous: Vec<ColumnMeta>,
    previous_content_id: u64,
    columns: Vec<ColumnMeta>,
    original: ColumnFile,
}

#[derive(Clone)]
pub struct CommitAttempt {
    pub expected: CommitToken,
    pub attempted: CommitToken,
    source: Arc<FileSource>,
}

impl std::fmt::Debug for CommitAttempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommitAttempt")
            .field("expected", &self.expected)
            .field("attempted", &self.attempted)
            .finish()
    }
}

impl CommitAttempt {
    pub(crate) fn published(file: &ColumnFile, expected: CommitToken) -> Self {
        Self {
            expected,
            attempted: file.commit_token(),
            source: file.file_source().clone(),
        }
    }

    pub fn inspect(&self) -> Result<ColumnFile> {
        self.source.file().lock()?;
        let result: Result<ColumnFile> = (|| {
            self.source.file().sync_data()?;
            let file = ColumnFile::from_source(self.source.clone())?;
            file.verify()?;
            Ok(file)
        })();
        let unlocked = self.source.file().unlock();
        let file = result?;
        unlocked?;
        Ok(file)
    }
}

impl ColumnFileUpdater {
    pub(super) fn open(reader: &ColumnFile, writer_id: WriterId) -> Result<Self> {
        let file_source = reader.file_source();
        let source: Arc<dyn ChunkSource> = file_source.clone();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(file_source.path())?;
        file.lock()?;

        let (newest_at, newest) = newest_slot(&file, reader.file_uuid())?
            .ok_or_else(|| Error::corrupt("commit slot", "no valid commit slot"))?;
        if newest.token() != reader.commit_token() {
            return Err(conflict(reader.commit_token(), newest.token()));
        }
        let target_slot = if newest_at == SLOT_A { SLOT_B } else { SLOT_A };

        let dir_end = newest.dir_end().ok_or_else(|| {
            Error::corrupt(
                "commit slot",
                "directory ends past the end of the file offsets",
            )
        })?;
        let cursor = match read_append_mark(&file, reader.file_uuid())? {
            Some(mark) => dir_end.max(mark.append_from),
            None => dir_end.max(file.metadata()?.len()),
        }
        .max(HEAD_LEN);

        Ok(Self {
            file,
            source,
            file_uuid: reader.file_uuid(),
            writer_id,
            expected_generation: newest.generation,
            target_slot,
            cursor,
            previous: reader.columns().to_vec(),
            previous_content_id: newest.content_id,
            columns: Vec::new(),
            original: reader.clone(),
        })
    }

    pub fn column<T: Record>(&mut self, name: &str, arena: &Arena<T>) -> Result<()> {
        if self.columns.iter().any(|column| column.name == name) {
            return Err(Error::invalid(format!("duplicate column {name:?}")));
        }
        if let Some(previous) = self.previous.iter().find(|column| column.name == name) {
            if previous.chunk_rows != arena.chunk_rows() as u64 {
                return Err(Error::invalid(format!(
                    "column {name:?} is chunked {} rows at a time on disk, not {}",
                    previous.chunk_rows,
                    arena.chunk_rows()
                )));
            }
            if previous.elem_size as usize != T::SIZE {
                return Err(Error::invalid(format!(
                    "column {name:?} holds {}-byte rows on disk, not {}",
                    previous.elem_size,
                    T::SIZE
                )));
            }
        }

        let reusable = arena
            .provenance()
            .is_some_and(|from| from.is_from(&self.source, self.expected_generation));

        let mut chunks = Vec::with_capacity(arena.chunk_count());
        for index in 0..arena.chunk_count() {
            if reusable && let Some(loc) = arena.reusable_loc(index) {
                chunks.push(loc);
                continue;
            }

            let bytes = arena.chunk_bytes(index)?;
            let loc = ChunkLoc {
                offset: self.cursor,
                byte_len: bytes.len() as u32,
                rows: (bytes.len() / T::SIZE.max(1)) as u32,
                fnv: fnv1a(&bytes),
            };
            self.file.write_payload(&bytes, self.cursor)?;
            self.cursor += bytes.len() as u64;
            chunks.push(loc);
        }

        self.columns.push(ColumnMeta {
            name: name.to_owned(),
            elem_size: T::SIZE as u32,
            chunk_rows: arena.chunk_rows() as u64,
            rows: arena.len() as u64,
            codec: Codec::Raw,
            chunks,
        });
        Ok(())
    }

    pub fn keep_column(&mut self, name: &str) -> Result<()> {
        if self.columns.iter().any(|column| column.name == name) {
            return Err(Error::invalid(format!("duplicate column {name:?}")));
        }
        let kept = self
            .previous
            .iter()
            .find(|column| column.name == name)
            .ok_or_else(|| Error::invalid(format!("no column named {name:?} to keep")))?;
        self.columns.push(kept.clone());
        Ok(())
    }

    pub fn remove_column(&mut self, name: &str) {
        self.previous.retain(|column| column.name != name);
    }

    pub fn commit(self) -> Result<u64> {
        self.commit_file().map(|file| file.generation())
    }

    pub fn commit_file(self) -> Result<ColumnFile> {
        let missing: Vec<&str> = self
            .previous
            .iter()
            .map(|column| column.name.as_str())
            .filter(|name| !self.columns.iter().any(|column| column.name == *name))
            .collect();
        if !missing.is_empty() {
            return Err(Error::invalid(format!(
                "commit would drop columns {missing:?}; supply them or call remove_column"
            )));
        }

        let content = content_id(&self.columns);
        if content == self.previous_content_id {
            self.file.sync_slot()?;
            return Ok(self.original);
        }

        let directory = write_directory(&self.columns);
        let dir_offset = self.cursor;
        let dir_len = directory.len() as u64;
        write_all_at(&self.file, &directory, dir_offset)?;
        self.file.sync_directory()?;

        let generation = self
            .expected_generation
            .checked_add(1)
            .ok_or_else(|| Error::invalid("generation counter exhausted"))?;
        let slot = CommitSlot::new(
            generation,
            content,
            self.file_uuid,
            self.writer_id,
            dir_offset,
            dir_len,
            fnv1a(&directory),
        );
        let source = Arc::new(FileSource::from_file(
            self.file.prepare_publication()?,
            self.original.file_source().path().to_owned(),
        ));
        let attempt = CommitAttempt {
            expected: self.original.commit_token(),
            attempted: slot.token(),
            source: source.clone(),
        };
        let publication = (|| -> std::io::Result<()> {
            self.file
                .write_slot(&slot.to_bytes(), self.target_slot)
                .map_err(|e| match e {
                    Error::Io(e) => e,
                    other => std::io::Error::other(other.to_string()),
                })?;
            self.file.sync_slot()
        })();
        if let Err(source) = publication {
            let _ = self.file.unlock();
            return Err(Error::CommitUncertain {
                attempt: Box::new(attempt),
                source,
            });
        }

        let mark = AppendMark::new(self.file_uuid, dir_offset + dir_len, generation);
        let _ = self.file.write_mark(&mark.to_bytes());
        let _ = self.file.unlock();
        Ok(ColumnFile::published(source, self.columns, slot))
    }
}

fn conflict(expected: CommitToken, found: CommitToken) -> Error {
    Error::Conflict { expected, found }
}

#[cfg(test)]
#[path = "updater_test.rs"]
mod updater_test;
