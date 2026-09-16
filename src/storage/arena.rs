//! Chunked, copy-on-write, lazily-loaded column storage.

use std::borrow::Cow;
use std::io;
use std::sync::{Arc, OnceLock};

use super::blob::Blob;
use super::hash::fnv1a;
use crate::record::Record;

pub const TARGET_CHUNK_BYTES: usize = 64 * 1024;

pub const fn default_chunk_rows(elem_size: usize) -> usize {
    if elem_size == 0 {
        return 1;
    }
    let rows = TARGET_CHUNK_BYTES / elem_size;
    if rows == 0 { 1 } else { rows }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChunkLoc {
    pub offset: u64,
    pub byte_len: u32,
    pub rows: u32,
    pub fnv: u64,
}

pub trait ChunkSource: Send + Sync {
    fn read_chunk(&self, loc: ChunkLoc) -> io::Result<Vec<u8>>;
}

enum ChunkCell<T> {
    Clean {
        loc: ChunkLoc,
        rows: OnceLock<Vec<T>>,
    },
    Dirty(Vec<T>),
}

impl<T> ChunkCell<T> {
    fn on_disk(loc: ChunkLoc) -> Self {
        Self::Clean {
            loc,
            rows: OnceLock::new(),
        }
    }

    fn resident_rows(&self) -> Option<&[T]> {
        match self {
            Self::Clean { rows, .. } => rows.get().map(Vec::as_slice),
            Self::Dirty(rows) => Some(rows),
        }
    }

    fn reusable_loc(&self) -> Option<ChunkLoc> {
        match self {
            Self::Clean { loc, .. } => Some(*loc),
            Self::Dirty(_) => None,
        }
    }

    fn still_on_disk(&self) -> Option<ChunkLoc> {
        match self {
            Self::Clean { loc, rows } if rows.get().is_none() => Some(*loc),
            _ => None,
        }
    }

    fn owned_rows(&mut self) -> &mut Vec<T> {
        if let Self::Clean { rows, .. } = self {
            let taken = rows
                .take()
                .expect("chunk must be resident before it is owned");
            *self = Self::Dirty(taken);
        }
        let Self::Dirty(rows) = self else {
            unreachable!("just converted to Dirty")
        };
        rows
    }
}

impl<T: Clone> Clone for ChunkCell<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Clean { loc, rows } => Self::Clean {
                loc: *loc,
                rows: rows.get().cloned().map(OnceLock::from).unwrap_or_default(),
            },
            Self::Dirty(rows) => Self::Dirty(rows.clone()),
        }
    }
}

#[derive(Clone)]
pub struct Provenance {
    source: Arc<dyn ChunkSource>,
    generation: u64,
}

impl Provenance {
    pub fn new(source: Arc<dyn ChunkSource>, generation: u64) -> Self {
        Self { source, generation }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_from(&self, source: &Arc<dyn ChunkSource>, generation: u64) -> bool {
        self.generation == generation && Arc::ptr_eq(&self.source, source)
    }
}

pub struct Arena<T> {
    chunks: Vec<Arc<ChunkCell<T>>>,
    chunk_rows: usize,
    len: usize,
    source: Option<Provenance>,
}

impl<T> Clone for Arena<T> {
    fn clone(&self) -> Self {
        Self {
            chunks: self.chunks.clone(),
            chunk_rows: self.chunk_rows,
            len: self.len,
            source: self.source.clone(),
        }
    }
}

impl<T> Arena<T> {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn chunk_rows(&self) -> usize {
        self.chunk_rows
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn resident_chunks(&self) -> usize {
        self.chunks
            .iter()
            .filter(|cell| cell.resident_rows().is_some())
            .count()
    }

    pub fn provenance(&self) -> Option<&Provenance> {
        self.source.as_ref()
    }

    pub fn reusable_loc(&self, index: usize) -> Option<ChunkLoc> {
        self.chunks.get(index).and_then(|cell| cell.reusable_loc())
    }
}

impl<T> std::fmt::Debug for Arena<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Arena")
            .field("rows", &self.len)
            .field("chunk_rows", &self.chunk_rows)
            .field("chunks", &self.chunks.len())
            .field("resident", &self.resident_chunks())
            .field("lazy", &self.source.is_some())
            .finish()
    }
}

impl<T: Record> Default for Arena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Record> Arena<T> {
    pub fn new() -> Self {
        Self::with_chunk_rows(default_chunk_rows(T::SIZE))
    }

    pub fn with_chunk_rows(chunk_rows: usize) -> Self {
        assert!(chunk_rows > 0, "chunk_rows must be positive");
        Self {
            chunks: Vec::new(),
            chunk_rows,
            len: 0,
            source: None,
        }
    }

    pub fn from_locations(
        locations: Vec<ChunkLoc>,
        chunk_rows: usize,
        source: Provenance,
    ) -> io::Result<Self> {
        assert!(chunk_rows > 0, "chunk_rows must be positive");
        let mut len = 0usize;
        for (index, loc) in locations.iter().enumerate() {
            let last = index + 1 == locations.len();
            if !last && loc.rows as usize != chunk_rows {
                return Err(invalid(format!(
                    "chunk {index} holds {} rows, expected {chunk_rows}; \
                     only the last chunk may be short",
                    loc.rows
                )));
            }
            if loc.byte_len as usize != loc.rows as usize * T::SIZE {
                return Err(invalid(format!(
                    "chunk {index} is {} bytes for {} rows of {} bytes",
                    loc.byte_len,
                    loc.rows,
                    T::SIZE
                )));
            }
            len += loc.rows as usize;
        }

        Ok(Self {
            chunks: locations
                .into_iter()
                .map(|loc| Arc::new(ChunkCell::on_disk(loc)))
                .collect(),
            chunk_rows,
            len,
            source: Some(source),
        })
    }

    pub fn chunk(&self, index: usize) -> io::Result<&[T]> {
        let cell = self
            .chunks
            .get(index)
            .ok_or_else(|| invalid(format!("chunk {index} out of range")))?;

        let (loc, rows) = match &**cell {
            ChunkCell::Dirty(rows) => return Ok(rows),
            ChunkCell::Clean { loc, rows } => (loc, rows),
        };
        if let Some(loaded) = rows.get() {
            return Ok(loaded);
        }

        let bytes = self.read_verified(*loc, index)?;
        Ok(rows.get_or_init(|| decode_rows::<T>(&bytes)))
    }

    fn owned_chunk(&mut self, index: usize) -> io::Result<&mut Vec<T>> {
        self.chunk(index)?;
        Ok(Arc::make_mut(&mut self.chunks[index]).owned_rows())
    }

    fn read_verified(&self, loc: ChunkLoc, index: usize) -> io::Result<Vec<u8>> {
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| invalid("arena has an on-disk chunk but no source"))?;
        let bytes = source.source.read_chunk(loc)?;

        let actual = fnv1a(&bytes);
        if actual != loc.fnv {
            return Err(invalid(format!(
                "chunk {index} at offset {} failed checksum: stored {:#018x}, computed {actual:#018x}",
                loc.offset, loc.fnv
            )));
        }
        if bytes.len() != loc.rows as usize * T::SIZE {
            return Err(invalid(format!(
                "chunk {index} read {} bytes for {} rows of {}",
                bytes.len(),
                loc.rows,
                T::SIZE
            )));
        }
        Ok(bytes)
    }

    pub fn prefetch(&self) -> io::Result<()> {
        for index in 0..self.chunks.len() {
            self.chunk(index)?;
        }
        Ok(())
    }

    fn row(&self, index: usize) -> io::Result<&T> {
        let chunk = self.chunk(index / self.chunk_rows)?;
        Ok(&chunk[index % self.chunk_rows])
    }

    pub fn try_get(&self, index: usize) -> io::Result<Option<&T>> {
        if index >= self.len {
            return Ok(None);
        }
        self.row(index).map(Some)
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.try_get(index).expect("chunk read failed")
    }

    pub fn try_range(&self, start: usize, len: usize) -> io::Result<Cow<'_, [T]>> {
        let end = start
            .checked_add(len)
            .ok_or_else(|| invalid("range overflows"))?;
        if end > self.len {
            return Err(invalid(format!(
                "range {start}..{end} past end {}",
                self.len
            )));
        }
        if len == 0 {
            return Ok(Cow::Borrowed(&[]));
        }

        let first = start / self.chunk_rows;
        let last = (end - 1) / self.chunk_rows;
        if first == last {
            let offset = start % self.chunk_rows;
            return Ok(Cow::Borrowed(&self.chunk(first)?[offset..offset + len]));
        }

        let mut out = Vec::with_capacity(len);
        for index in first..=last {
            let chunk = self.chunk(index)?;
            let base = index * self.chunk_rows;
            let lo = start.saturating_sub(base);
            let hi = (end - base).min(chunk.len());
            out.extend_from_slice(&chunk[lo..hi]);
        }
        Ok(Cow::Owned(out))
    }

    pub fn try_iter(&self) -> impl Iterator<Item = io::Result<&T>> + '_ {
        (0..self.len).map(move |index| self.row(index))
    }

    fn tail_mut(&mut self) -> io::Result<&mut Vec<T>> {
        if self.len.is_multiple_of(self.chunk_rows) {
            self.chunks
                .push(Arc::new(ChunkCell::Dirty(Vec::with_capacity(
                    self.chunk_rows,
                ))));
        }
        self.owned_chunk(self.chunks.len() - 1)
    }

    pub fn try_push(&mut self, item: T) -> io::Result<usize> {
        self.tail_mut()?.push(item);
        self.len += 1;
        Ok(self.len - 1)
    }

    pub fn push(&mut self, item: T) -> usize {
        self.try_push(item).expect("chunk read failed")
    }

    pub fn try_extend_from_slice(&mut self, mut items: &[T]) -> io::Result<()> {
        while !items.is_empty() {
            let room = self.chunk_rows - (self.len % self.chunk_rows);
            let take = room.min(items.len());
            let (now, rest) = items.split_at(take);
            self.tail_mut()?.extend_from_slice(now);
            self.len += take;
            items = rest;
        }
        Ok(())
    }

    pub fn extend_from_slice(&mut self, items: &[T]) {
        self.try_extend_from_slice(items)
            .expect("chunk read failed")
    }

    pub fn try_set(&mut self, index: usize, item: T) -> io::Result<()> {
        if index >= self.len {
            return Err(invalid(format!("index {index} past end {}", self.len)));
        }
        let chunk_index = index / self.chunk_rows;
        let offset = index % self.chunk_rows;
        self.owned_chunk(chunk_index)?[offset] = item;
        Ok(())
    }

    #[allow(clippy::chunks_exact_to_as_chunks)]
    pub fn chunk_bytes(&self, index: usize) -> io::Result<Vec<u8>> {
        let cell = self
            .chunks
            .get(index)
            .ok_or_else(|| invalid(format!("chunk {index} out of range")))?;

        if let Some(loc) = cell.still_on_disk() {
            return self.read_verified(loc, index);
        }

        let rows = self.chunk(index)?;
        let mut bytes = vec![0u8; rows.len() * T::SIZE];
        for (row, slot) in rows.iter().zip(bytes.chunks_exact_mut(T::SIZE)) {
            row.encode(slot);
        }
        Ok(bytes)
    }
}

impl Arena<u8> {
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Blob {
        let start = self.len() as u64;
        self.extend_from_slice(bytes);
        Blob {
            start,
            len: bytes.len() as u32,
        }
    }

    pub fn push_str(&mut self, text: &str) -> Blob {
        self.push_bytes(text.as_bytes())
    }

    pub fn try_bytes(&self, blob: Blob) -> io::Result<Cow<'_, [u8]>> {
        self.try_range(blob.start as usize, blob.len as usize)
    }

    pub fn try_str(&self, blob: Blob) -> io::Result<Cow<'_, str>> {
        Ok(match self.try_bytes(blob)? {
            Cow::Borrowed(bytes) => String::from_utf8_lossy(bytes),
            Cow::Owned(bytes) => Cow::Owned(String::from_utf8_lossy(&bytes).into_owned()),
        })
    }
}

#[allow(clippy::chunks_exact_to_as_chunks)]
fn decode_rows<T: Record>(bytes: &[u8]) -> Vec<T> {
    bytes.chunks_exact(T::SIZE).map(T::decode).collect()
}

pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
#[path = "arena_test.rs"]
mod arena_test;
