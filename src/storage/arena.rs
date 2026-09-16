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

struct ChunkCell<T> {
    data: OnceLock<Vec<T>>,
    loc: Option<ChunkLoc>,
}

impl<T> ChunkCell<T> {
    fn resident(rows: Vec<T>) -> Self {
        let data = OnceLock::new();
        let _ = data.set(rows);
        Self { data, loc: None }
    }

    fn on_disk(loc: ChunkLoc) -> Self {
        Self {
            data: OnceLock::new(),
            loc: Some(loc),
        }
    }
}

impl<T: Clone> Clone for ChunkCell<T> {
    // An unloaded chunk must clone as unloaded, or a write faults in chunks it never touched.
    fn clone(&self) -> Self {
        let data = OnceLock::new();
        if let Some(rows) = self.data.get() {
            let _ = data.set(rows.clone());
        }
        Self {
            data,
            loc: self.loc,
        }
    }
}

pub struct Arena<T> {
    chunks: Vec<Arc<ChunkCell<T>>>,
    chunk_rows: usize,
    len: usize,
    source: Option<Arc<dyn ChunkSource>>,
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
            .filter(|cell| cell.data.get().is_some())
            .count()
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
        source: Arc<dyn ChunkSource>,
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

        if let Some(rows) = cell.data.get() {
            return Ok(rows);
        }

        let loc = cell
            .loc
            .expect("a chunk with no location is always resident");
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| invalid("arena has an on-disk chunk but no source"))?;

        let bytes = source.read_chunk(loc)?;
        let rows = decode_chunk::<T>(&bytes, loc, index)?;
        // A concurrent reader may have won the race; either decoding is valid.
        let _ = cell.data.set(rows);
        Ok(cell.data.get().expect("just set"))
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
                .push(Arc::new(ChunkCell::resident(Vec::with_capacity(
                    self.chunk_rows,
                ))));
        } else {
            // Fault in before unsharing: make_mut on an unloaded chunk clones it
            // as unloaded, leaving nothing to append to.
            self.chunk(self.chunks.len() - 1)?;
        }

        let cell = Arc::make_mut(self.chunks.last_mut().expect("just ensured"));
        cell.loc = None;
        Ok(cell.data.get_mut().expect("resident"))
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
        self.chunk(chunk_index)?;
        let cell = Arc::make_mut(&mut self.chunks[chunk_index]);
        cell.loc = None;
        cell.data.get_mut().expect("resident")[index % self.chunk_rows] = item;
        Ok(())
    }

    #[allow(clippy::chunks_exact_to_as_chunks)]
    pub fn chunk_bytes(&self, index: usize) -> io::Result<Vec<u8>> {
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
fn decode_chunk<T: Record>(bytes: &[u8], loc: ChunkLoc, index: usize) -> io::Result<Vec<T>> {
    let actual = fnv1a(bytes);
    if actual != loc.fnv {
        return Err(invalid(format!(
            "chunk {index} at offset {} failed checksum: stored {:#018x}, computed {actual:#018x}",
            loc.offset, loc.fnv
        )));
    }
    let rows = loc.rows as usize;
    if bytes.len() != rows * T::SIZE {
        return Err(invalid(format!(
            "chunk {index} read {} bytes for {rows} rows of {}",
            bytes.len(),
            T::SIZE
        )));
    }
    Ok(bytes.chunks_exact(T::SIZE).map(T::decode).collect())
}

pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
#[path = "arena_test.rs"]
mod arena_test;
