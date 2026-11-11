//! A byte-string field: blob offsets in one column, the bytes they name in another.

use std::borrow::Cow;

use super::column::{Column, ColumnKind, ColumnSpec, Residency, Row};
use super::names::{heap_name, qualified};
use super::text::heap_garbage;
use crate::error::{Error, Result};
use crate::record::Record;
use crate::storage::blob::Blob;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater};

#[derive(Clone, Default)]
pub struct BytesColumn {
    blobs: Arena<Blob>,
    heap: Arena<u8>,
}

impl BytesColumn {
    pub fn heap_len(&self) -> usize {
        self.heap.len()
    }

    pub fn push(&mut self, value: &[u8]) -> Result<usize> {
        let blob = self.heap.push_bytes(value)?;
        self.blobs.push(blob)
    }

    pub fn get(&self, row: Row) -> Result<Cow<'_, [u8]>> {
        self.heap.bytes(*self.blobs.row(row.index())?)
    }

    pub fn set(&mut self, row: Row, value: &[u8]) -> Result<()> {
        let blob = self.heap.push_bytes(value)?;
        self.blobs.set(row.index(), blob)
    }
}

impl Column for BytesColumn {
    fn unused_heap_bytes(&self) -> Result<u64> {
        heap_garbage(
            self.heap.len(),
            self.blobs
                .iter()
                .map(|blob| blob.copied())
                .collect::<Result<Vec<_>>>()?,
        )
    }

    fn compact_storage(&mut self) -> Result<bool> {
        let mut compacted = Self::default();
        let mut shared = std::collections::HashMap::<Vec<u8>, Blob>::new();
        for index in 0..self.blobs.len() {
            let value = self.get(Row::trusted(index))?;
            let blob = match shared.get(value.as_ref()) {
                Some(blob) => *blob,
                None => {
                    let blob = compacted.heap.push_bytes(&value)?;
                    shared.insert(value.into_owned(), blob);
                    blob
                }
            };
            compacted.blobs.push(blob)?;
        }
        *self = compacted;
        Ok(true)
    }

    fn append_value_bytes(&mut self, bytes: &[u8]) -> Result<usize> {
        self.push(decode_bytes(bytes)?)
    }

    fn value_bytes(&self, row: Row) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        super::encode_bytes(&self.get(row)?, &mut out);
        Ok(out)
    }

    fn schema_type(&self) -> String {
        "bytes".to_owned()
    }

    fn rows(&self) -> usize {
        self.blobs.len()
    }

    fn touch_tail(&self) -> Result<()> {
        self.blobs.touch_tail()?;
        self.heap.touch_tail()
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: qualified(entity, field),
            elem_size: <Blob as Record>::SIZE,
            kind: ColumnKind::Bytes,
            counts_rows: true,
        });
        out.push(ColumnSpec {
            name: heap_name(entity, field),
            elem_size: <u8 as Record>::SIZE,
            kind: ColumnKind::Heap,
            counts_rows: false,
        });
    }

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        self.blobs.residency(entity, field, out);
        out.push(Residency {
            name: heap_name(entity, field),
            resident: self.heap.resident_chunks(),
            chunks: self.heap.chunk_count(),
        });
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.blobs = file.column(&qualified(entity, field))?;
        self.heap = file.column(&heap_name(entity, field))?;
        Ok(())
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        updater.column(&qualified(entity, field), &self.blobs)?;
        updater.column(&heap_name(entity, field), &self.heap)
    }

    fn rebase(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        file.rebase(&qualified(entity, field), &mut self.blobs)?;
        file.rebase(&heap_name(entity, field), &mut self.heap)
    }
}

#[derive(Clone, Default)]
pub struct NullableBytesColumn {
    blobs: Arena<Option<Blob>>,
    heap: Arena<u8>,
}

impl NullableBytesColumn {
    pub fn heap_len(&self) -> usize {
        self.heap.len()
    }

    pub fn push(&mut self, value: Option<&[u8]>) -> Result<usize> {
        let blob = self.store(value)?;
        self.blobs.push(blob)
    }

    pub fn get(&self, row: Row) -> Result<Option<Cow<'_, [u8]>>> {
        match *self.blobs.row(row.index())? {
            Some(blob) => self.heap.bytes(blob).map(Some),
            None => Ok(None),
        }
    }

    pub fn set(&mut self, row: Row, value: Option<&[u8]>) -> Result<()> {
        let blob = self.store(value)?;
        self.blobs.set(row.index(), blob)
    }

    fn store(&mut self, value: Option<&[u8]>) -> Result<Option<Blob>> {
        match value {
            Some(bytes) => self.heap.push_bytes(bytes).map(Some),
            None => Ok(None),
        }
    }
}

impl Column for NullableBytesColumn {
    fn unused_heap_bytes(&self) -> Result<u64> {
        heap_garbage(
            self.heap.len(),
            self.blobs
                .iter()
                .map(|blob| blob.copied())
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect(),
        )
    }

    fn compact_storage(&mut self) -> Result<bool> {
        let mut compacted = Self::default();
        let mut shared = std::collections::HashMap::<Vec<u8>, Blob>::new();
        for index in 0..self.blobs.len() {
            let blob = match self.get(Row::trusted(index))? {
                None => None,
                Some(value) => Some(match shared.get(value.as_ref()) {
                    Some(blob) => *blob,
                    None => {
                        let blob = compacted.heap.push_bytes(&value)?;
                        shared.insert(value.into_owned(), blob);
                        blob
                    }
                }),
            };
            compacted.blobs.push(blob)?;
        }
        *self = compacted;
        Ok(true)
    }

    fn append_value_bytes(&mut self, bytes: &[u8]) -> Result<usize> {
        match bytes {
            [0] => self.push(None),
            [1, value @ ..] => self.push(Some(decode_bytes(value)?)),
            _ => Err(Error::corrupt("bytes", "invalid optional bytes")),
        }
    }

    fn value_bytes(&self, row: Row) -> Result<Vec<u8>> {
        let value = self.get(row)?;
        let mut out = vec![u8::from(value.is_some())];
        if let Some(value) = value {
            super::encode_bytes(&value, &mut out);
        }
        Ok(out)
    }

    fn schema_type(&self) -> String {
        "optional bytes".to_owned()
    }

    fn rows(&self) -> usize {
        self.blobs.len()
    }

    fn touch_tail(&self) -> Result<()> {
        self.blobs.touch_tail()?;
        self.heap.touch_tail()
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: qualified(entity, field),
            elem_size: <Option<Blob> as Record>::SIZE,
            kind: ColumnKind::Nullable(Box::new(ColumnKind::Bytes)),
            counts_rows: true,
        });
        out.push(ColumnSpec {
            name: heap_name(entity, field),
            elem_size: <u8 as Record>::SIZE,
            kind: ColumnKind::Heap,
            counts_rows: false,
        });
    }

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        self.blobs.residency(entity, field, out);
        out.push(Residency {
            name: heap_name(entity, field),
            resident: self.heap.resident_chunks(),
            chunks: self.heap.chunk_count(),
        });
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.blobs = file.column(&qualified(entity, field))?;
        self.heap = file.column(&heap_name(entity, field))?;
        Ok(())
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        updater.column(&qualified(entity, field), &self.blobs)?;
        updater.column(&heap_name(entity, field), &self.heap)
    }

    fn rebase(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        file.rebase(&qualified(entity, field), &mut self.blobs)?;
        file.rebase(&heap_name(entity, field), &mut self.heap)
    }
}

fn decode_bytes(bytes: &[u8]) -> Result<&[u8]> {
    let length = bytes
        .get(..8)
        .ok_or_else(|| Error::corrupt("bytes", "missing length"))?;
    let length = u64::from_le_bytes(length.try_into().expect("8 bytes"));
    let value = &bytes[8..];
    if length != value.len() as u64 {
        return Err(Error::corrupt("bytes", "length mismatch"));
    }
    Ok(value)
}

#[cfg(test)]
#[path = "bytes_test.rs"]
mod bytes_test;
