//! An open column file, serving lazy arenas over its chunks.

use std::path::Path;
use std::sync::Arc;

use super::HEAD_LEN;
use super::codec::Codec;
use super::column_meta::{ColumnMeta, read_directory};
use super::commit_slot::{CommitSlot, newest_slot, slots_newest_first};
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
        let mut said = Error::corrupt("commit slot", "no valid commit slot");
        for slot in slots_newest_first(source.file(), file_uuid)? {
            match Self::at_slot(&source, slot, total) {
                Ok(columns) => {
                    return Ok(Self {
                        source,
                        columns,
                        slot,
                    });
                }
                Err(err) if err.is_corrupt() => said = err,
                Err(err) => return Err(err),
            }
        }
        Err(said)
    }

    fn at_slot(source: &FileSource, slot: CommitSlot, total: u64) -> Result<Vec<ColumnMeta>> {
        let (dir_offset, dir_len) = (slot.dir_offset, slot.dir_len);
        let dir_end = dir_offset
            .checked_add(dir_len)
            .ok_or_else(|| Error::corrupt("directory", "extends outside the file"))?;
        if dir_offset < HEAD_LEN || dir_end > total {
            return Err(Error::corrupt("directory", "extends outside the file"));
        }
        let loc = directory_loc(source, slot)?;
        let directory = source.read_chunk(loc)?;
        let columns = read_directory(&directory, dir_offset)?;
        for column in &columns {
            for loc in &column.chunks {
                if loc
                    .offset
                    .checked_add(loc.physical_len())
                    .is_none_or(|end| end > total)
                {
                    return Err(Error::corrupt("chunk", "extends outside the file"));
                }
            }
        }
        Ok(columns)
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
        if meta.codec != Codec::Zstd {
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

    pub fn rebase<T: Record>(&self, name: &str, arena: &mut Arena<T>) -> Result<()> {
        match self.column_meta(name) {
            Some(meta) if meta.elem_size as usize == T::SIZE && meta.rows == arena.len() as u64 => {
                arena.rebase(
                    &meta.chunks,
                    Provenance::new(self.source.clone(), self.slot.generation),
                )
            }
            Some(_) => Err(Error::invalid(format!(
                "column {name:?} in the file is not the column held in memory"
            ))),
            None if arena.is_empty() => Ok(()),
            None => Err(Error::invalid(format!("no column named {name:?}"))),
        }
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
    if slot.dir_len <= super::compression::HEADER_LEN as u64 {
        return Err(Error::corrupt("directory", "short record"));
    }
    let mut header = [0; super::compression::HEADER_LEN];
    source.read_at(&mut header, slot.dir_offset)?;
    let (byte_len, _, depth) = super::compression::header(&header)?;
    Ok(crate::ChunkLoc {
        offset: slot.dir_offset,
        byte_len,
        rows: 0,
        fnv: slot.dir_fnv,
        stored_len: u32::try_from(slot.dir_len)
            .map_err(|_| Error::corrupt("directory", "too large"))?,
        depth,
    })
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
