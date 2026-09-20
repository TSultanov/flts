//! An open column file, serving lazy arenas over its chunks.

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
use crate::error::{Error, Result};
use crate::record::Record;
use crate::storage::arena::{Arena, ChunkSource, Provenance};

#[derive(Clone)]
pub struct ColumnFile {
    source: Arc<FileSource>,
    columns: Vec<ColumnMeta>,
    pub(super) slot: CommitSlot,
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
    pub fn read_commit_token(path: impl AsRef<Path>) -> Result<CommitToken> {
        let source = FileSource::open(path)?;
        if source.len()? < HEAD_LEN {
            return Err(Error::corrupt("file", "too short to be a column file"));
        }
        let header = read_header(&source)?;
        let (_, slot) = newest_slot(source.file(), header.file_uuid())?
            .ok_or_else(|| Error::corrupt("commit slot", "no valid commit slot"))?;
        Ok(slot.token())
    }

    pub fn commit_token(&self) -> CommitToken {
        self.slot.token()
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_source(Arc::new(FileSource::open(path)?))
    }

    pub(super) fn from_source(source: Arc<FileSource>) -> Result<Self> {
        let total = source.len()?;
        if total < HEAD_LEN {
            return Err(Error::corrupt("file", "too short to be a column file"));
        }

        let file_uuid = read_header(&source)?.file_uuid();

        let (_, slot) = newest_slot(source.file(), file_uuid)?
            .ok_or_else(|| Error::corrupt("commit slot", "no valid commit slot"))?;

        let (dir_offset, dir_len) = (slot.dir_offset, slot.dir_len);
        let dir_end = dir_offset
            .checked_add(dir_len)
            .ok_or_else(|| Error::corrupt("directory", "extends outside the file"))?;
        if dir_offset < HEAD_LEN || dir_end > total {
            return Err(Error::corrupt("directory", "extends outside the file"));
        }
        let loc = directory_loc(&source, slot)?;
        let directory = source.read_chunk(loc)?;

        let columns = read_directory(&directory, dir_offset)?;
        Ok(Self {
            source,
            columns,
            slot,
        })
    }

    pub(super) fn directory_loc(&self) -> Result<crate::ChunkLoc> {
        directory_loc(&self.source, self.slot)
    }

    pub fn update(&self, writer_id: WriterId) -> Result<super::ColumnFileUpdater> {
        super::ColumnFileUpdater::open(self, writer_id)
    }

    pub(super) fn published(
        source: Arc<FileSource>,
        columns: Vec<ColumnMeta>,
        slot: CommitSlot,
    ) -> Self {
        Self {
            source,
            columns,
            slot,
        }
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

    pub fn file_bytes(&self) -> Result<u64> {
        self.source.len()
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

    pub fn column<T: Record>(&self, name: &str) -> Result<Arena<T>> {
        let meta = self
            .column_meta(name)
            .ok_or_else(|| Error::invalid(format!("no column named {name:?}")))?;
        if meta.elem_size as usize != T::SIZE {
            return Err(Error::invalid(format!(
                "column {name:?} holds {}-byte rows, requested type is {} bytes",
                meta.elem_size,
                T::SIZE
            )));
        }
        if meta.codec != Codec::Raw {
            return Err(Error::unsupported(format!(
                "column {name:?} uses an unsupported codec"
            )));
        }

        let arena = Arena::from_locations(
            meta.chunks.clone(),
            meta.chunk_rows as usize,
            Provenance::new(self.source.clone(), self.slot.generation),
        )?;
        if arena.len() as u64 != meta.rows {
            return Err(Error::corrupt(
                "directory",
                format!(
                    "column {name:?} declares {} rows but its chunks hold {}",
                    meta.rows,
                    arena.len()
                ),
            ));
        }
        Ok(arena)
    }

    pub fn verify(&self) -> Result<()> {
        self.source.read_chunk(self.directory_loc()?)?;
        for meta in &self.columns {
            for loc in &meta.chunks {
                self.source.read_chunk(*loc)?;
            }
        }
        Ok(())
    }
}

fn directory_loc(source: &FileSource, slot: CommitSlot) -> Result<crate::ChunkLoc> {
    let mut prefix = [0; 4];
    if slot.dir_len < 4 {
        return Err(Error::corrupt("directory", "short record"));
    }
    source.read_at(&mut prefix, slot.dir_offset)?;
    let mut loc = crate::ChunkLoc {
        offset: slot.dir_offset,
        byte_len: u32::try_from(slot.dir_len)
            .map_err(|_| Error::corrupt("directory", "too large"))?,
        rows: 0,
        fnv: slot.dir_fnv,
        stored_len: 0,
        depth: 0,
    };
    if u32::from_le_bytes(prefix) == super::delta::MAGIC {
        if slot.dir_len < 9 {
            return Err(Error::corrupt("directory", "short delta"));
        }
        let mut header = [0; 5];
        source.read_at(&mut header, slot.dir_offset + 4)?;
        loc.stored_len = loc.byte_len;
        loc.byte_len = u32::from_le_bytes(header[..4].try_into().unwrap());
        loc.depth = header[4];
        if loc.depth == 0 || loc.depth > super::delta::MAX_DEPTH {
            return Err(Error::corrupt("directory", "invalid depth"));
        }
    }
    Ok(loc)
}

fn read_header(source: &FileSource) -> Result<Header> {
    let mut bytes = [0u8; Header::SIZE];
    source.read_at(&mut bytes, 0)?;
    let header = Header::decode(&bytes);
    header.validate()?;
    Ok(header)
}

#[cfg(test)]
#[path = "reader_test.rs"]
mod reader_test;
