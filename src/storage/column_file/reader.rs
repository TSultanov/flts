//! An open column file, serving lazy arenas over its chunks.

use std::io;
use std::path::Path;
use std::sync::Arc;

use super::codec::Codec;
use super::column_meta::{ColumnMeta, read_directory};
use super::file_source::FileSource;
use super::footer::Footer;
use super::header::Header;
use super::{FOOTER_LEN, HEADER_LEN};
use crate::record::Record;
use crate::storage::arena::{Arena, ChunkSource, invalid};
use crate::storage::hash::fnv1a;

pub struct ColumnFile {
    source: Arc<FileSource>,
    columns: Vec<ColumnMeta>,
}

impl std::fmt::Debug for ColumnFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColumnFile")
            .field("columns", &self.columns)
            .finish()
    }
}

impl ColumnFile {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let source = FileSource::open(path)?;
        let total = source.len()?;
        if total < HEADER_LEN + FOOTER_LEN {
            return Err(invalid("file is too short to be a column file"));
        }

        let mut header = [0u8; Header::SIZE];
        source.read_at(&mut header, 0)?;
        Header::decode(&header).validate()?;

        let mut bytes = [0u8; Footer::SIZE];
        source.read_at(&mut bytes, total - FOOTER_LEN)?;
        let footer = Footer::decode(&bytes);
        footer.validate()?;
        let (dir_offset, dir_len, dir_fnv) = (footer.dir_offset, footer.dir_len, footer.dir_fnv);

        let dir_end = dir_offset
            .checked_add(dir_len)
            .ok_or_else(|| invalid("directory extends outside the file"))?;
        if dir_offset < HEADER_LEN || dir_end > total - FOOTER_LEN {
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
        })
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
            self.source.clone(),
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

#[cfg(test)]
#[path = "reader_test.rs"]
mod reader_test;
