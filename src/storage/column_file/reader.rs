//! An open column file, serving lazy arenas over its chunks.

use std::io;
use std::path::Path;
use std::sync::Arc;

use super::HEAD_LEN;
use super::codec::Codec;
use super::column_meta::{ColumnMeta, read_directory};
use super::commit_slot::{CommitSlot, newest_slot};
use super::commit_token::CommitToken;
use super::file_source::FileSource;
use super::header::Header;
use super::writer_id::WriterId;
use crate::record::Record;
use crate::storage::arena::{Arena, ChunkSource, Provenance, invalid};
use crate::storage::hash::fnv1a;

pub struct ColumnFile {
    source: Arc<FileSource>,
    columns: Vec<ColumnMeta>,
    slot: CommitSlot,
}

impl std::fmt::Debug for ColumnFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColumnFile")
            .field("generation", &self.generation())
            .field("columns", &self.columns)
            .finish()
    }
}

impl ColumnFile {
    pub fn read_commit_token(path: impl AsRef<Path>) -> io::Result<CommitToken> {
        let source = FileSource::open(path)?;
        if source.len()? < HEAD_LEN {
            return Err(invalid("file is too short to be a column file"));
        }
        let header = read_header(&source)?;
        let (_, slot) = newest_slot(source.file(), header.file_uuid())?
            .ok_or_else(|| invalid("column file has no valid commit slot"))?;
        Ok(slot.token())
    }

    pub fn commit_token(&self) -> CommitToken {
        self.slot.token()
    }

    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let source = FileSource::open(path)?;
        let total = source.len()?;
        if total < HEAD_LEN {
            return Err(invalid("file is too short to be a column file"));
        }

        let file_uuid = read_header(&source)?.file_uuid();

        let (_, slot) = newest_slot(source.file(), file_uuid)?
            .ok_or_else(|| invalid("column file has no valid commit slot"))?;

        let (dir_offset, dir_len, dir_fnv) = (slot.dir_offset, slot.dir_len, slot.dir_fnv);
        let dir_end = dir_offset
            .checked_add(dir_len)
            .ok_or_else(|| invalid("directory extends outside the file"))?;
        if dir_offset < HEAD_LEN || dir_end > total {
            return Err(invalid("directory extends outside the file"));
        }
        let mut directory = vec![0u8; dir_len as usize];
        source.read_at(&mut directory, dir_offset)?;
        if fnv1a(&directory) != dir_fnv {
            return Err(invalid("column file directory failed checksum"));
        }

        let columns = read_directory(&directory, dir_offset)?;
        Ok(Self {
            source: Arc::new(source),
            columns,
            slot,
        })
    }

    pub fn update(&self, writer_id: WriterId) -> io::Result<super::ColumnFileUpdater> {
        super::ColumnFileUpdater::open(self, writer_id)
    }

    pub(super) fn file_source(&self) -> &Arc<FileSource> {
        &self.source
    }

    pub fn generation(&self) -> u64 {
        self.slot.generation
    }

    pub fn content_id(&self) -> u64 {
        self.slot.content_id
    }

    pub fn writer_id(&self) -> WriterId {
        WriterId::from_raw(self.slot.writer_id)
    }

    pub fn file_uuid(&self) -> u128 {
        self.slot.file_uuid
    }

    pub fn live_bytes(&self) -> u64 {
        self.columns.iter().map(ColumnMeta::byte_len).sum()
    }

    pub fn columns(&self) -> &[ColumnMeta] {
        &self.columns
    }

    pub fn column_meta(&self, name: &str) -> Option<&ColumnMeta> {
        self.columns.iter().find(|column| column.name == name)
    }

    pub fn column<T: Record>(&self, name: &str) -> io::Result<Arena<T>> {
        let meta = self
            .column_meta(name)
            .ok_or_else(|| invalid(format!("no column named {name:?}")))?;
        if meta.elem_size as usize != T::SIZE {
            return Err(invalid(format!(
                "column {name:?} holds {}-byte rows, requested type is {} bytes",
                meta.elem_size,
                T::SIZE
            )));
        }
        if meta.codec != Codec::Raw {
            return Err(invalid(format!(
                "column {name:?} uses an unsupported codec"
            )));
        }

        let arena = Arena::from_locations(
            meta.chunks.clone(),
            meta.chunk_rows as usize,
            Provenance::new(self.source.clone(), self.slot.generation),
        )?;
        if arena.len() as u64 != meta.rows {
            return Err(invalid(format!(
                "column {name:?} declares {} rows but its chunks hold {}",
                meta.rows,
                arena.len()
            )));
        }
        Ok(arena)
    }

    pub fn verify(&self) -> io::Result<()> {
        for meta in &self.columns {
            for (index, loc) in meta.chunks.iter().enumerate() {
                let bytes = self.source.read_chunk(*loc)?;
                if fnv1a(&bytes) != loc.fnv {
                    return Err(invalid(format!(
                        "column {:?} chunk {index} failed checksum",
                        meta.name
                    )));
                }
            }
        }
        Ok(())
    }
}

fn read_header(source: &FileSource) -> io::Result<Header> {
    let mut bytes = [0u8; Header::SIZE];
    source.read_at(&mut bytes, 0)?;
    let header = Header::decode(&bytes);
    header.validate()?;
    Ok(header)
}

#[cfg(test)]
#[path = "reader_test.rs"]
mod reader_test;
