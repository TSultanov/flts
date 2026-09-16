//! Writes columns sequentially into a new column file.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use super::HEADER_LEN;
use super::codec::Codec;
use super::column_meta::{ColumnMeta, write_directory};
use super::footer::Footer;
use super::header::Header;
use crate::record::Record;
use crate::storage::arena::{Arena, ChunkLoc, invalid};
use crate::storage::hash::fnv1a;

pub struct ColumnFileWriter<W: Write> {
    out: W,
    offset: u64,
    columns: Vec<ColumnMeta>,
}

impl ColumnFileWriter<io::BufWriter<File>> {
    pub fn create(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::new(io::BufWriter::new(File::create(path)?))
    }
}

impl<W: Write> ColumnFileWriter<W> {
    pub fn new(mut out: W) -> io::Result<Self> {
        let mut header = [0u8; Header::SIZE];
        Header::current().encode(&mut header);
        out.write_all(&header)?;
        Ok(Self {
            out,
            offset: HEADER_LEN,
            columns: Vec::new(),
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
        let footer = Footer::new(self.offset, directory.len() as u64, fnv1a(&directory));
        self.out.write_all(&directory)?;

        let mut bytes = [0u8; Footer::SIZE];
        footer.encode(&mut bytes);
        self.out.write_all(&bytes)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

#[cfg(test)]
#[path = "writer_test.rs"]
mod writer_test;
