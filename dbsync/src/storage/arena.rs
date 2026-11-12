//! Chunked, copy-on-write, lazily-loaded column storage.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use super::blob::Blob;
use super::hash::fnv1a;
use crate::error::{Error, Result};
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
    /// Reconstructed byte length, after decompression and any delta application.
    pub byte_len: u32,
    pub rows: u32,
    /// Checksum of the reconstructed bytes.
    pub fnv: u64,
    /// Physical record length, including its header and compressed frame.
    pub stored_len: u32,
    pub depth: u8,
}

impl ChunkLoc {
    pub fn physical_len(self) -> u64 {
        self.stored_len as u64
    }
}

pub trait ChunkSource: Send + Sync {
    fn read_chunk(&self, loc: ChunkLoc) -> Result<Vec<u8>>;

    /// The record at `loc` as it is stored, where it stands by itself.
    fn read_stored(&self, _loc: ChunkLoc) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

#[derive(Clone)]
struct Damage {
    record: &'static str,
    detail: String,
}

impl Damage {
    fn to_error(&self) -> Error {
        Error::corrupt(self.record, self.detail.clone())
    }
}

type Settled<T> = std::result::Result<Vec<T>, Damage>;

enum ChunkCell<T> {
    Clean {
        loc: ChunkLoc,
        rows: OnceLock<Settled<T>>,
        loading: Mutex<()>,
    },
    Dirty(Vec<T>),
}

impl<T> ChunkCell<T> {
    fn on_disk(loc: ChunkLoc) -> Self {
        Self::Clean {
            loc,
            rows: OnceLock::new(),
            loading: Mutex::new(()),
        }
    }

    fn resident_rows(&self) -> Option<&[T]> {
        match self {
            Self::Clean { rows, .. } => rows
                .get()
                .and_then(|settled| settled.as_ref().ok())
                .map(Vec::as_slice),
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
            Self::Clean { loc, rows, .. } if rows.get().is_none() => Some(*loc),
            _ => None,
        }
    }

    fn settled(loc: ChunkLoc, resident: Option<Vec<T>>) -> Self {
        let rows = OnceLock::new();
        if let Some(resident) = resident {
            let _ = rows.set(Ok(resident));
        }
        Self::Clean {
            loc,
            rows,
            loading: Mutex::new(()),
        }
    }

    fn owned_rows(&mut self) -> &mut Vec<T> {
        if let Self::Clean { rows, .. } = self {
            let taken = rows
                .take()
                .and_then(|settled| settled.ok())
                .expect("a chunk is owned only after it loads cleanly");
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
            Self::Clean { loc, rows, .. } => Self::Clean {
                loc: *loc,
                rows: rows.get().cloned().map(OnceLock::from).unwrap_or_default(),
                loading: Mutex::new(()),
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

    pub fn stored(&self, loc: ChunkLoc) -> Result<Option<Vec<u8>>> {
        self.source.read_stored(loc)
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
    ) -> Result<Self> {
        assert!(chunk_rows > 0, "chunk_rows must be positive");
        let mut len = 0usize;
        for (index, loc) in locations.iter().enumerate() {
            let last = index + 1 == locations.len();
            let rows = loc.rows as usize;
            let fits = if last {
                rows <= chunk_rows
            } else {
                rows == chunk_rows
            };
            if !fits {
                return Err(Error::corrupt(
                    "chunk",
                    format!(
                        "chunk {index} holds {rows} rows, expected {chunk_rows}; \
                         only the last chunk may be short"
                    ),
                ));
            }
            if loc.byte_len as usize != loc.rows as usize * T::SIZE {
                return Err(Error::corrupt(
                    "chunk",
                    format!(
                        "chunk {index} is {} bytes for {} rows of {} bytes",
                        loc.byte_len,
                        loc.rows,
                        T::SIZE
                    ),
                ));
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

    pub fn rebase(&mut self, locations: &[ChunkLoc], source: Provenance) -> Result<()> {
        if locations.len() != self.chunks.len() {
            return Err(Error::invalid(format!(
                "a column of {} chunks cannot settle onto {} locations",
                self.chunks.len(),
                locations.len()
            )));
        }
        for (cell, loc) in self.chunks.iter_mut().zip(locations) {
            if cell.reusable_loc() == Some(*loc) {
                continue;
            }
            let resident = cell.resident_rows().map(<[T]>::to_vec);
            if resident
                .as_ref()
                .is_some_and(|rows| rows.len() != loc.rows as usize)
            {
                return Err(Error::invalid(
                    "a chunk cannot settle onto a location holding different rows",
                ));
            }
            *cell = Arc::new(ChunkCell::settled(*loc, resident));
        }
        self.source = Some(source);
        Ok(())
    }

    pub fn chunk(&self, index: usize) -> Result<&[T]> {
        let cell = &self.chunks[index];

        let (loc, rows, loading) = match &**cell {
            ChunkCell::Dirty(rows) => return Ok(rows),
            ChunkCell::Clean { loc, rows, loading } => (loc, rows, loading),
        };
        if let Some(settled) = rows.get() {
            return settled
                .as_ref()
                .map(Vec::as_slice)
                .map_err(Damage::to_error);
        }

        let _loading = loading.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(settled) = rows.get() {
            return settled
                .as_ref()
                .map(Vec::as_slice)
                .map_err(Damage::to_error);
        }

        let settled = match self
            .read_verified(*loc, index)
            .map(|bytes| decode_rows::<T>(&bytes))
        {
            Ok(rows) => Ok(rows),
            Err(Error::Corrupt { record, detail }) => Err(Damage { record, detail }),
            Err(passing) => return Err(passing),
        };
        rows.get_or_init(|| settled)
            .as_ref()
            .map(Vec::as_slice)
            .map_err(Damage::to_error)
    }

    fn owned_chunk(&mut self, index: usize) -> Result<&mut Vec<T>> {
        self.chunk(index)?;
        Ok(Arc::make_mut(&mut self.chunks[index]).owned_rows())
    }

    fn read_verified(&self, loc: ChunkLoc, index: usize) -> Result<Vec<u8>> {
        let source = self
            .source
            .as_ref()
            .expect("a chunk with a location has a source");
        let bytes = source.source.read_chunk(loc)?;

        let actual = fnv1a(&bytes);
        if actual != loc.fnv {
            return Err(Error::corrupt(
                "chunk",
                format!(
                    "chunk {index} at offset {} failed checksum: stored {:#018x}, computed {actual:#018x}",
                    loc.offset, loc.fnv
                ),
            ));
        }
        if bytes.len() != loc.rows as usize * T::SIZE {
            return Err(Error::corrupt(
                "chunk",
                format!(
                    "chunk {index} read {} bytes for {} rows of {}",
                    bytes.len(),
                    loc.rows,
                    T::SIZE
                ),
            ));
        }
        Ok(bytes)
    }

    pub fn prefetch(&self) -> Result<()> {
        for index in 0..self.chunks.len() {
            self.chunk(index)?;
        }
        Ok(())
    }

    pub fn row(&self, index: usize) -> Result<&T> {
        let chunk = self.chunk(index / self.chunk_rows)?;
        Ok(&chunk[index % self.chunk_rows])
    }

    pub fn get(&self, index: usize) -> Result<Option<&T>> {
        if index >= self.len {
            return Ok(None);
        }
        self.row(index).map(Some)
    }

    pub fn range(&self, start: usize, len: usize) -> Result<Cow<'_, [T]>> {
        let end = start.checked_add(len).expect("range overflows");
        assert!(
            end <= self.len,
            "range {start}..{end} past end {}",
            self.len
        );
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

    pub fn iter(&self) -> impl Iterator<Item = Result<&T>> + '_ {
        (0..self.len).map(move |index| self.row(index))
    }

    fn tail_mut(&mut self) -> Result<&mut Vec<T>> {
        if self.len.is_multiple_of(self.chunk_rows) {
            self.chunks
                .push(Arc::new(ChunkCell::Dirty(Vec::with_capacity(
                    self.chunk_rows,
                ))));
        }
        self.owned_chunk(self.chunks.len() - 1)
    }

    pub fn push(&mut self, item: T) -> Result<usize> {
        self.tail_mut()?.push(item);
        self.len += 1;
        Ok(self.len - 1)
    }

    pub fn extend_from_slice(&mut self, mut items: &[T]) -> Result<()> {
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

    pub fn set(&mut self, index: usize, item: T) -> Result<()> {
        assert!(index < self.len, "index {index} past end {}", self.len);
        let chunk_index = index / self.chunk_rows;
        let offset = index % self.chunk_rows;
        self.owned_chunk(chunk_index)?[offset] = item;
        Ok(())
    }

    #[allow(clippy::chunks_exact_to_as_chunks)]
    pub fn chunk_bytes(&self, index: usize) -> Result<Vec<u8>> {
        let cell = &self.chunks[index];

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
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Result<Blob> {
        let start = self.len() as u64;
        self.extend_from_slice(bytes)?;
        Ok(Blob {
            start,
            len: bytes.len() as u32,
        })
    }

    pub fn push_str(&mut self, text: &str) -> Result<Blob> {
        self.push_bytes(text.as_bytes())
    }

    pub fn bytes(&self, blob: Blob) -> Result<Cow<'_, [u8]>> {
        let within = blob.end().is_some_and(|end| end <= self.len() as u64);
        if !within {
            return Err(Error::corrupt(
                "blob",
                format!(
                    "blob at {} for {} bytes runs past the {} the column holds",
                    blob.start,
                    blob.len,
                    self.len()
                ),
            ));
        }
        self.range(blob.start as usize, blob.len as usize)
    }

    pub fn text(&self, blob: Blob) -> Result<Cow<'_, str>> {
        Ok(match self.bytes(blob)? {
            Cow::Borrowed(bytes) => String::from_utf8_lossy(bytes),
            Cow::Owned(bytes) => Cow::Owned(String::from_utf8_lossy(&bytes).into_owned()),
        })
    }
}

#[allow(clippy::chunks_exact_to_as_chunks)]
fn decode_rows<T: Record>(bytes: &[u8]) -> Vec<T> {
    bytes.chunks_exact(T::SIZE).map(T::decode).collect()
}

#[cfg(test)]
#[path = "arena_test.rs"]
mod arena_test;
