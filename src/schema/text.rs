//! A string field: blob offsets in one column, the bytes they name in another.

use std::borrow::Cow;

use super::column::{Column, ColumnKind, ColumnSpec, Residency, Row};
use super::index::{IndexKey, Keys};
use super::indexed::Keyed;
use super::names::{heap_name, qualified};
use crate::error::{Error, Result};
use crate::record::Record;
use crate::storage::blob::Blob;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater, fnv1a};

const KEY_SIZE: usize = 24;
const INLINE: usize = KEY_SIZE - size_of::<u32>();
const ABSENT: u32 = u32::MAX;
const NO_START: u64 = u64::MAX;

const _: () = assert!(INLINE >= 2 * size_of::<u64>());

#[derive(Clone, Copy, Debug)]
pub struct TextKey {
    len: u32,
    payload: [u8; INLINE],
}

impl TextKey {
    pub const ABSENT: Self = Self {
        len: ABSENT,
        payload: [0; INLINE],
    };

    pub fn of(value: &str) -> Self {
        Self::held(value.as_bytes(), NO_START)
    }

    pub fn stored(value: &str, at: Blob) -> Self {
        Self::held(value.as_bytes(), at.start)
    }

    pub fn maybe(value: Option<&str>) -> Self {
        value.map_or(Self::ABSENT, Self::of)
    }

    fn held(bytes: &[u8], start: u64) -> Self {
        let mut payload = [0u8; INLINE];
        if bytes.len() <= INLINE {
            payload[..bytes.len()].copy_from_slice(bytes);
        } else {
            payload[..8].copy_from_slice(&fnv1a(bytes).to_le_bytes());
            payload[8..16].copy_from_slice(&start.to_le_bytes());
        }
        Self {
            len: bytes.len() as u32,
            payload,
        }
    }

    pub fn is_absent(self) -> bool {
        self.len == ABSENT
    }

    pub fn is_inline(self) -> bool {
        self.len as usize <= INLINE
    }

    fn digest(self) -> u64 {
        u64::from_le_bytes(self.payload[..8].try_into().expect("8 bytes"))
    }

    pub fn blob(self) -> Blob {
        Blob::new(
            u64::from_le_bytes(self.payload[8..16].try_into().expect("8 bytes")),
            self.len,
        )
    }
}

impl PartialEq for TextKey {
    fn eq(&self, other: &Self) -> bool {
        if self.len != other.len {
            return false;
        }
        match self.is_inline() {
            true => self.payload == other.payload,
            false => self.digest() == other.digest(),
        }
    }
}

impl Eq for TextKey {}

impl Record for TextKey {
    const SIZE: usize = KEY_SIZE;

    fn encode(&self, out: &mut [u8]) {
        out[..4].copy_from_slice(&self.len.to_le_bytes());
        out[4..].copy_from_slice(&self.payload);
    }

    fn decode(src: &[u8]) -> Self {
        let mut payload = [0u8; INLINE];
        payload.copy_from_slice(&src[4..]);
        Self {
            len: u32::from_le_bytes(src[..4].try_into().expect("4 bytes")),
            payload,
        }
    }
}

impl IndexKey for TextKey {
    fn raw(self) -> u128 {
        match self.is_inline() {
            true => fnv1a(&self.payload[..self.len as usize]) as u128,
            false => self.digest() as u128,
        }
    }
}

#[derive(Clone, Default)]
pub struct TextColumn {
    blobs: Arena<Blob>,
    heap: Arena<u8>,
}

impl TextColumn {
    pub fn heap_len(&self) -> usize {
        self.heap.len()
    }

    pub fn push(&mut self, value: &str) -> Result<usize> {
        let blob = self.heap.push_str(value)?;
        self.blobs.push(blob)
    }

    pub fn get(&self, row: Row) -> Result<Cow<'_, str>> {
        self.heap.text(*self.blobs.row(row.index())?)
    }

    pub fn set(&mut self, row: Row, value: &str) -> Result<()> {
        let blob = self.heap.push_str(value)?;
        self.blobs.set(row.index(), blob)
    }
}

impl TextColumn {
    fn bytes_of(&mut self, value: &str, shared: Option<TextKey>) -> Result<Blob> {
        match shared {
            Some(known) => Ok(known.blob()),
            None => self.heap.push_str(value),
        }
    }
}

impl Column for TextColumn {
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
        let mut shared = std::collections::HashMap::<String, Blob>::new();
        for index in 0..self.blobs.len() {
            let value = self.get(Row::trusted(index))?;
            let blob = match shared.get(value.as_ref()) {
                Some(blob) => *blob,
                None => {
                    let blob = compacted.heap.push_str(&value)?;
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
        self.push(decode_text(bytes)?)
    }

    fn value_bytes(&self, row: Row) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        super::encode_text(&self.get(row)?, &mut out);
        Ok(out)
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
            kind: ColumnKind::Text,
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
}

#[derive(Clone, Default)]
pub struct NullableTextColumn {
    blobs: Arena<Option<Blob>>,
    heap: Arena<u8>,
}

impl NullableTextColumn {
    pub fn heap_len(&self) -> usize {
        self.heap.len()
    }

    pub fn push(&mut self, value: Option<&str>) -> Result<usize> {
        let blob = self.store(value)?;
        self.blobs.push(blob)
    }

    pub fn get(&self, row: Row) -> Result<Option<Cow<'_, str>>> {
        match *self.blobs.row(row.index())? {
            Some(blob) => self.heap.text(blob).map(Some),
            None => Ok(None),
        }
    }

    pub fn set(&mut self, row: Row, value: Option<&str>) -> Result<()> {
        let blob = self.store(value)?;
        self.blobs.set(row.index(), blob)
    }

    fn store(&mut self, value: Option<&str>) -> Result<Option<Blob>> {
        match value {
            Some(text) => self.heap.push_str(text).map(Some),
            None => Ok(None),
        }
    }

    fn bytes_of(&mut self, value: Option<&str>, shared: Option<TextKey>) -> Result<Option<Blob>> {
        match shared {
            Some(known) => Ok(Some(known.blob())),
            None => self.store(value),
        }
    }
}

impl Column for NullableTextColumn {
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
        let mut shared = std::collections::HashMap::<String, Blob>::new();
        for index in 0..self.blobs.len() {
            let blob = match self.get(Row::trusted(index))? {
                None => None,
                Some(value) => Some(match shared.get(value.as_ref()) {
                    Some(blob) => *blob,
                    None => {
                        let blob = compacted.heap.push_str(&value)?;
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
            [1, value @ ..] => self.push(Some(decode_text(value)?)),
            _ => Err(Error::corrupt("text", "invalid optional text")),
        }
    }

    fn value_bytes(&self, row: Row) -> Result<Vec<u8>> {
        let value = self.get(row)?;
        let mut out = vec![u8::from(value.is_some())];
        if let Some(value) = value {
            super::encode_text(&value, &mut out);
        }
        Ok(out)
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
            kind: ColumnKind::Nullable(Box::new(ColumnKind::Text)),
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
}

fn decode_text(bytes: &[u8]) -> Result<&str> {
    let length = bytes
        .get(..8)
        .ok_or_else(|| Error::corrupt("text", "missing text length"))?;
    let length = u64::from_le_bytes(length.try_into().expect("8 bytes"));
    let value = &bytes[8..];
    if length != value.len() as u64 {
        return Err(Error::corrupt("text", "text length mismatch"));
    }
    std::str::from_utf8(value).map_err(|_| Error::corrupt("text", "invalid UTF-8"))
}

fn no_row(row: usize, rows: usize) -> Error {
    Error::invalid(format!("a column of {rows} keys has no row {row}"))
}

impl Keys for TextColumn {
    type Key = TextKey;

    fn key_at(&self, row: usize) -> Result<TextKey> {
        let Some(blob) = self.blobs.get(row)?.copied() else {
            return Err(no_row(row, self.blobs.len()));
        };
        Ok(TextKey::held(&self.heap.bytes(blob)?, blob.start))
    }
}

impl Keyed for TextColumn {
    type Seek<'a> = &'a str;

    fn key_for(value: &str) -> TextKey {
        TextKey::of(value)
    }

    fn shares_bytes(key: TextKey) -> bool {
        !key.is_inline() && !key.is_absent()
    }

    fn append(&mut self, value: &str, shared: Option<TextKey>) -> Result<TextKey> {
        let blob = self.bytes_of(value, shared)?;
        self.blobs.push(blob)?;
        Ok(TextKey::stored(value, blob))
    }

    fn write(&mut self, row: Row, value: &str, shared: Option<TextKey>) -> Result<TextKey> {
        let blob = self.bytes_of(value, shared)?;
        self.blobs.set(row.index(), blob)?;
        Ok(TextKey::stored(value, blob))
    }

    fn holds(&self, key: TextKey, value: &str) -> Result<bool> {
        match key.is_inline() {
            true => Ok(true),
            false => Ok(*self.heap.bytes(key.blob())? == *value.as_bytes()),
        }
    }
}

impl Keys for NullableTextColumn {
    type Key = TextKey;

    fn key_at(&self, row: usize) -> Result<TextKey> {
        match self.blobs.get(row)?.copied() {
            Some(Some(blob)) => Ok(TextKey::held(&self.heap.bytes(blob)?, blob.start)),
            Some(None) => Ok(TextKey::ABSENT),
            None => Err(no_row(row, self.blobs.len())),
        }
    }
}

impl Keyed for NullableTextColumn {
    type Seek<'a> = Option<&'a str>;

    fn key_for(value: Option<&str>) -> TextKey {
        TextKey::maybe(value)
    }

    fn shares_bytes(key: TextKey) -> bool {
        !key.is_inline() && !key.is_absent()
    }

    fn append(&mut self, value: Option<&str>, shared: Option<TextKey>) -> Result<TextKey> {
        let blob = self.bytes_of(value, shared)?;
        self.blobs.push(blob)?;
        Ok(perhaps(value, blob))
    }

    fn write(&mut self, row: Row, value: Option<&str>, shared: Option<TextKey>) -> Result<TextKey> {
        let blob = self.bytes_of(value, shared)?;
        self.blobs.set(row.index(), blob)?;
        Ok(perhaps(value, blob))
    }

    fn holds(&self, key: TextKey, value: Option<&str>) -> Result<bool> {
        match value {
            Some(text) if !key.is_inline() => Ok(*self.heap.bytes(key.blob())? == *text.as_bytes()),
            _ => Ok(true),
        }
    }
}

fn perhaps(value: Option<&str>, blob: Option<Blob>) -> TextKey {
    match (value, blob) {
        (Some(text), Some(at)) => TextKey::stored(text, at),
        _ => TextKey::ABSENT,
    }
}

#[cfg(test)]
#[path = "text_test.rs"]
mod text_test;

fn heap_garbage(length: usize, blobs: Vec<Blob>) -> Result<u64> {
    let mut ranges = Vec::with_capacity(blobs.len());
    for blob in blobs {
        let end = blob
            .start
            .checked_add(blob.len as u64)
            .filter(|end| *end <= length as u64)
            .ok_or_else(|| Error::corrupt("heap", "blob outside heap"))?;
        ranges.push((blob.start, end));
    }
    ranges.sort_unstable();
    let mut covered = 0;
    let mut previous = 0;
    for (start, end) in ranges {
        covered += end.saturating_sub(previous.max(start));
        previous = previous.max(end);
    }
    Ok(length as u64 - covered)
}
