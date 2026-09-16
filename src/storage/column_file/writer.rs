//! Writes columns sequentially into a new column file.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

use super::codec::Codec;
use super::column_meta::{ColumnMeta, content_id, write_directory};
use super::commit_slot::CommitSlot;
use super::header::{Header, new_file_uuid};
use super::writer_id::WriterId;
use super::{HEAD_LEN, SLOT_A};
use crate::record::Record;
use crate::storage::arena::{Arena, ChunkLoc, invalid};
use crate::storage::hash::fnv1a;

pub struct ColumnFileWriter<W: Write + Seek> {
    out: W,
    offset: u64,
    columns: Vec<ColumnMeta>,
    file_uuid: u128,
    writer_id: WriterId,
}

impl ColumnFileWriter<io::BufWriter<File>> {
    pub fn create(path: impl AsRef<Path>, writer_id: WriterId) -> io::Result<Self> {
        Self::new(io::BufWriter::new(File::create(path)?), writer_id)
    }
}

impl<W: Write + Seek> ColumnFileWriter<W> {
    pub fn new(mut out: W, writer_id: WriterId) -> io::Result<Self> {
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

    pub fn column<T: Record>(&mut self, name: &str, arena: &Arena<T>) -> io::Result<()> {
        if self.columns.iter().any(|column| column.name == name) {
            return Err(invalid(format!("duplicate column {name:?}")));
        }

        let mut chunks = Vec::with_capacity(arena.chunk_count());
        for index in 0..arena.chunk_count() {
            let bytes = arena.chunk_bytes(index)?;
            let loc = ChunkLoc {
                offset: self.offset,
                byte_len: bytes.len() as u32,
                rows: (bytes.len() / T::SIZE.max(1)) as u32,
                fnv: fnv1a(&bytes),
            };
            self.out.write_all(&bytes)?;
            self.offset += bytes.len() as u64;
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

    pub fn finish(mut self) -> io::Result<W> {
        let directory = write_directory(&self.columns);
        let dir_offset = self.offset;
        let dir_len = directory.len() as u64;
        self.out.write_all(&directory)?;

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
        self.out.flush()?;
        Ok(self.out)
    }
}

#[cfg(test)]
#[path = "writer_test.rs"]
mod writer_test;
