//! Writes columns sequentially into a new column file.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

use super::append_mark::AppendMark;
use super::codec::Codec;
use super::column_meta::{ColumnMeta, content_id, write_directory};
use super::commit_slot::CommitSlot;
use super::header::{Header, new_file_uuid};
use super::writer_id::WriterId;
use super::{APPEND_MARK, HEAD_LEN, SLOT_A};
use crate::error::{Error, Result};
use crate::record::Record;
use crate::storage::arena::{Arena, ChunkLoc};
use crate::storage::hash::fnv1a;

pub struct ColumnFileWriter<W: Write + Seek> {
    out: W,
    offset: u64,
    columns: Vec<ColumnMeta>,
    file_uuid: u128,
    writer_id: WriterId,
}

impl ColumnFileWriter<io::BufWriter<File>> {
    pub fn create(path: impl AsRef<Path>, writer_id: WriterId) -> Result<Self> {
        Self::new(io::BufWriter::new(File::create(path)?), writer_id)
    }
}

impl<W: Write + Seek> ColumnFileWriter<W> {
    pub fn new(mut out: W, writer_id: WriterId) -> Result<Self> {
        let file_uuid = new_file_uuid();
        let mut head = vec![0u8; HEAD_LEN as usize];
        Header::new(file_uuid).encode(&mut head[..Header::SIZE]);
        out.write_all(&head)?;
        Ok(Self {
            out,
            offset: HEAD_LEN,
            columns: Vec::new(),
            file_uuid,
            writer_id,
        })
    }

    pub fn column<T: Record>(&mut self, name: &str, arena: &Arena<T>) -> Result<()> {
        if self.columns.iter().any(|column| column.name == name) {
            return Err(Error::invalid(format!("duplicate column {name:?}")));
        }

        let mut chunks = Vec::with_capacity(arena.chunk_count());
        for index in 0..arena.chunk_count() {
            let bytes = arena.chunk_bytes(index)?;
            let byte_len =
                u32::try_from(bytes.len()).map_err(|_| Error::invalid("chunk exceeds u32"))?;
            let payload = super::compression::encode(&bytes, byte_len, 0)?;
            let loc = ChunkLoc {
                stored_len: payload.len() as u32,
                depth: 0,
                offset: self.offset,
                byte_len,
                rows: (bytes.len() / T::SIZE.max(1)) as u32,
                fnv: fnv1a(&bytes),
            };
            self.out.write_all(&payload)?;
            self.offset += payload.len() as u64;
            chunks.push(loc);
        }

        self.columns.push(ColumnMeta {
            name: name.to_owned(),
            elem_size: T::SIZE as u32,
            chunk_rows: arena.chunk_rows() as u64,
            rows: arena.len() as u64,
            codec: Codec::Zstd,
            chunks,
        });
        Ok(())
    }

    pub fn finish(mut self) -> Result<W> {
        let directory = write_directory(&self.columns);
        let byte_len =
            u32::try_from(directory.len()).map_err(|_| Error::invalid("directory exceeds u32"))?;
        let payload = super::compression::encode(&directory, byte_len, 0)?;
        let dir_offset = self.offset;
        let dir_len = payload.len() as u64;
        self.out.write_all(&payload)?;

        let slot = CommitSlot::new(
            1,
            content_id(&self.columns),
            self.file_uuid,
            self.writer_id,
            dir_offset,
            dir_len,
            fnv1a(&directory),
        );
        self.out.seek(SeekFrom::Start(SLOT_A))?;
        self.out.write_all(&slot.to_bytes())?;

        let mark = AppendMark::new(self.file_uuid, dir_offset + dir_len, 1);
        self.out.seek(SeekFrom::Start(APPEND_MARK))?;
        self.out.write_all(&mark.to_bytes())?;
        self.out.flush()?;
        Ok(self.out)
    }
}

#[cfg(test)]
#[path = "writer_test.rs"]
mod writer_test;
