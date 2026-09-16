//! Per-column directory entry, and the directory's encoding.

use std::io;

use super::HEAD_LEN;
use super::codec::Codec;
use super::cursor::Cursor;
use crate::storage::arena::{ChunkLoc, invalid};
use crate::storage::hash::fnv1a;

#[derive(Clone, Debug)]
pub struct ColumnMeta {
    pub name: String,
    pub elem_size: u32,
    pub chunk_rows: u64,
    pub rows: u64,
    pub codec: Codec,
    pub chunks: Vec<ChunkLoc>,
}

impl ColumnMeta {
    pub fn byte_len(&self) -> u64 {
        self.chunks.iter().map(|chunk| chunk.byte_len as u64).sum()
    }
}

pub(super) fn content_id(columns: &[ColumnMeta]) -> u64 {
    let mut bytes = Vec::new();
    for column in columns {
        bytes.extend_from_slice(&(column.name.len() as u32).to_le_bytes());
        bytes.extend_from_slice(column.name.as_bytes());
        bytes.extend_from_slice(&column.elem_size.to_le_bytes());
        bytes.extend_from_slice(&column.chunk_rows.to_le_bytes());
        bytes.extend_from_slice(&column.rows.to_le_bytes());
        bytes.push(column.codec as u8);
        bytes.extend_from_slice(&(column.chunks.len() as u32).to_le_bytes());
        for chunk in &column.chunks {
            bytes.extend_from_slice(&chunk.rows.to_le_bytes());
            bytes.extend_from_slice(&chunk.fnv.to_le_bytes());
        }
    }
    fnv1a(&bytes)
}

pub(super) fn write_directory(columns: &[ColumnMeta]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(columns.len() as u32).to_le_bytes());
    for column in columns {
        out.extend_from_slice(&(column.name.len() as u32).to_le_bytes());
        out.extend_from_slice(column.name.as_bytes());
        out.extend_from_slice(&column.elem_size.to_le_bytes());
        out.extend_from_slice(&column.chunk_rows.to_le_bytes());
        out.extend_from_slice(&column.rows.to_le_bytes());
        out.push(column.codec as u8);
        out.extend_from_slice(&(column.chunks.len() as u32).to_le_bytes());
        for chunk in &column.chunks {
            out.extend_from_slice(&chunk.offset.to_le_bytes());
            out.extend_from_slice(&chunk.byte_len.to_le_bytes());
            out.extend_from_slice(&chunk.rows.to_le_bytes());
            out.extend_from_slice(&chunk.fnv.to_le_bytes());
        }
    }
    out
}

pub(super) fn read_directory(bytes: &[u8], dir_offset: u64) -> io::Result<Vec<ColumnMeta>> {
    let mut cursor = Cursor::new(bytes);
    let count = cursor.u32()? as usize;
    let mut columns = Vec::with_capacity(count);
    for _ in 0..count {
        let name_len = cursor.u32()? as usize;
        let name = String::from_utf8(cursor.take(name_len)?.to_vec())
            .map_err(|_| invalid("column name is not utf-8"))?;
        let elem_size = cursor.u32()?;
        let chunk_rows = cursor.u64()?;
        let rows = cursor.u64()?;
        let codec = Codec::from_u8(cursor.u8()?)?;
        let chunk_count = cursor.u32()? as usize;
        let mut chunks = Vec::with_capacity(chunk_count);
        for _ in 0..chunk_count {
            let loc = ChunkLoc {
                offset: cursor.u64()?,
                byte_len: cursor.u32()?,
                rows: cursor.u32()?,
                fnv: cursor.u64()?,
            };
            let end = loc.offset.checked_add(loc.byte_len as u64).ok_or_else(|| {
                invalid(format!("column {name:?} has a chunk range that overflows"))
            })?;
            if loc.offset < HEAD_LEN || end > dir_offset {
                return Err(invalid(format!(
                    "column {name:?} points a chunk outside the payload region"
                )));
            }
            chunks.push(loc);
        }
        columns.push(ColumnMeta {
            name,
            elem_size,
            chunk_rows,
            rows,
            codec,
            chunks,
        });
    }
    check_disjoint(&columns)?;
    Ok(columns)
}

fn check_disjoint(columns: &[ColumnMeta]) -> io::Result<()> {
    let mut ranges: Vec<(u64, u64, &str)> = Vec::new();
    for column in columns {
        for chunk in &column.chunks {
            if chunk.byte_len > 0 {
                let end = chunk.offset + chunk.byte_len as u64;
                ranges.push((chunk.offset, end, column.name.as_str()));
            }
        }
    }
    ranges.sort_by_key(|(start, _, _)| *start);

    for pair in ranges.windows(2) {
        let (_, previous_end, previous) = pair[0];
        let (start, _, current) = pair[1];
        if start < previous_end {
            return Err(invalid(format!(
                "chunks of {previous:?} and {current:?} overlap at offset {start}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "column_meta_test.rs"]
mod column_meta_test;
