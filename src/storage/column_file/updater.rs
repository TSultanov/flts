//! Appends changed chunks to an existing file and publishes them as a generation.

use std::fs::{File, OpenOptions};
use std::io;
use std::sync::Arc;

use super::append_mark::{AppendMark, read_append_mark};
use super::codec::Codec;
use super::column_meta::{ColumnMeta, content_id, write_directory};
use super::commit_slot::{CommitSlot, newest_slot};
use super::file_source::write_all_at;
use super::reader::ColumnFile;
use super::writer_id::WriterId;
use super::{APPEND_MARK, SLOT_A, SLOT_B};
use crate::record::Record;
use crate::storage::arena::{Arena, ChunkLoc, ChunkSource, invalid};
use crate::storage::hash::fnv1a;

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
}

impl ColumnFileUpdater {
    pub(super) fn open(reader: &ColumnFile, writer_id: WriterId) -> io::Result<Self> {
        let file_source = reader.file_source();
        let source: Arc<dyn ChunkSource> = file_source.clone();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(file_source.path())?;
        file.lock()?;

        let (newest_at, newest) = newest_slot(&file, reader.file_uuid())?
            .ok_or_else(|| invalid("column file has no valid commit slot"))?;
        if newest.generation != reader.generation() {
            return Err(conflict(reader.generation(), newest.generation));
        }
        let target_slot = if newest_at == SLOT_A { SLOT_B } else { SLOT_A };

        let mark = read_append_mark(&file, reader.file_uuid())?;
        let cursor = newest
            .dir_end()
            .max(mark.map_or(0, |mark| mark.append_from));

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
        })
    }

    pub fn column<T: Record>(&mut self, name: &str, arena: &Arena<T>) -> io::Result<()> {
        if self.columns.iter().any(|column| column.name == name) {
            return Err(invalid(format!("duplicate column {name:?}")));
        }
        if let Some(previous) = self.previous.iter().find(|column| column.name == name)
            && previous.chunk_rows != arena.chunk_rows() as u64
        {
            return Err(invalid(format!(
                "column {name:?} is chunked {} rows at a time on disk, not {}",
                previous.chunk_rows,
                arena.chunk_rows()
            )));
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
            write_all_at(&self.file, &bytes, self.cursor)?;
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

    pub fn remove_column(&mut self, name: &str) {
        self.previous.retain(|column| column.name != name);
    }

    pub fn commit(self) -> io::Result<u64> {
        let missing: Vec<&str> = self
            .previous
            .iter()
            .map(|column| column.name.as_str())
            .filter(|name| !self.columns.iter().any(|column| column.name == *name))
            .collect();
        if !missing.is_empty() {
            return Err(invalid(format!(
                "commit would drop columns {missing:?}; supply them or call remove_column"
            )));
        }

        let content = content_id(&self.columns);
        if content == self.previous_content_id {
            return Ok(self.expected_generation);
        }

        let directory = write_directory(&self.columns);
        let dir_offset = self.cursor;
        let dir_len = directory.len() as u64;
        write_all_at(&self.file, &directory, dir_offset)?;
        self.file.sync_data()?;

        let generation = self
            .expected_generation
            .checked_add(1)
            .ok_or_else(|| invalid("generation counter exhausted"))?;
        let slot = CommitSlot::new(
            generation,
            content,
            self.file_uuid,
            self.writer_id,
            dir_offset,
            dir_len,
            fnv1a(&directory),
        );
        write_all_at(&self.file, &slot.to_bytes(), self.target_slot)?;
        self.file.sync_data()?;

        let mark = AppendMark::new(self.file_uuid, dir_offset + dir_len, generation);
        write_all_at(&self.file, &mark.to_bytes(), APPEND_MARK)?;
        Ok(generation)
    }
}

fn conflict(expected: u64, found: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::ResourceBusy,
        format!("column file moved to generation {found} while holding {expected}"),
    )
}

#[cfg(test)]
#[path = "updater_test.rs"]
mod updater_test;
