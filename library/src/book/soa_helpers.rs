use std::{borrow::Cow, marker::PhantomData, ops::Index, sync::Arc};

#[derive(PartialEq, Debug, Eq, Hash)]
pub struct VecSlice<T> {
    pub start: usize,
    pub len: usize,
    phantom: PhantomData<T>,
}

impl<T> Clone for VecSlice<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for VecSlice<T> {}

impl<T> VecSlice<T> {
    pub fn empty() -> VecSlice<T> {
        VecSlice {
            start: 0,
            len: 0,
            phantom: PhantomData,
        }
    }

    pub fn new(start: usize, len: usize) -> VecSlice<T> {
        VecSlice {
            start,
            len,
            phantom: PhantomData,
        }
    }

    pub fn end(&self) -> usize {
        self.start + self.len
    }

    pub fn slice<'a>(&self, source: &'a [T]) -> &'a [T] {
        &source[self.start..self.end()]
    }
}

pub fn push_string(strings: &mut Vec<u8>, string: &str) -> VecSlice<u8> {
    let start = strings.len();
    strings.extend(string.bytes());
    VecSlice {
        start,
        len: string.len(),
        phantom: PhantomData,
    }
}

pub fn push<T: Clone>(items: &mut Vec<T>, slice: &VecSlice<T>, item: T) -> Option<VecSlice<T>> {
    let mut slice = *slice;
    if slice.end() > items.len() {
        return None;
    }

    if slice.end() < items.len() {
        let slice_items_copy = slice.slice(items).to_vec();
        slice.start = items.len();
        items.extend(slice_items_copy);
    }

    items.push(item);
    slice.len += 1;
    Some(slice)
}

/// Append-only arena stored in fixed-size chunks behind `Arc`, so a clone
/// shares every chunk and a later write copies only the chunk it touches.
/// Offsets are global and dense, so `VecSlice`s and the on-disk layout are
/// the same as for a flat `Vec`.
pub struct Arena<T> {
    chunks: Vec<Arc<Vec<T>>>,
    len: usize,
}

impl<T> Arena<T> {
    const CHUNK: usize = {
        let per_chunk = 64 * 1024 / size_of::<T>();
        if per_chunk == 0 { 1 } else { per_chunk }
    };

    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len {
            return None;
        }
        Some(&self.chunks[index / Self::CHUNK][index % Self::CHUNK])
    }

    /// The backing chunks in order; all but the last are full.
    pub fn chunks(&self) -> impl Iterator<Item = &[T]> {
        self.chunks.iter().map(|chunk| chunk.as_slice())
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.chunks.iter().flat_map(|chunk| chunk.iter())
    }

    fn range(&self, start: usize, end: usize) -> impl Iterator<Item = &T> {
        self.iter().skip(start).take(end - start)
    }
}

impl<T: Clone> Arena<T> {
    /// The last chunk, unshared and with room for at least one more item.
    /// `Arc::make_mut` would copy a shared tail at `capacity == len`, so the
    /// very next push reallocates it; copy into a full-size chunk instead.
    fn tail_mut(&mut self) -> &mut Vec<T> {
        if self.len.is_multiple_of(Self::CHUNK) {
            self.chunks.push(Arc::new(Vec::with_capacity(Self::CHUNK)));
        }
        let tail = self.chunks.last_mut().unwrap();
        if Arc::get_mut(tail).is_none_or(|chunk| chunk.capacity() < Self::CHUNK) {
            let mut copy = Vec::with_capacity(Self::CHUNK);
            copy.extend_from_slice(tail);
            *tail = Arc::new(copy);
        }
        Arc::get_mut(tail).unwrap()
    }

    pub fn push(&mut self, item: T) {
        self.tail_mut().push(item);
        self.len += 1;
    }

    pub fn extend_from_slice(&mut self, mut items: &[T]) {
        while !items.is_empty() {
            let room = Self::CHUNK - self.len % Self::CHUNK;
            let (now, rest) = items.split_at(room.min(items.len()));
            self.tail_mut().extend_from_slice(now);
            self.len += now.len();
            items = rest;
        }
    }

    pub fn extend(&mut self, items: impl IntoIterator<Item = T>) {
        for item in items {
            self.push(item);
        }
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index >= self.len {
            return None;
        }
        Some(&mut Arc::make_mut(&mut self.chunks[index / Self::CHUNK])[index % Self::CHUNK])
    }

    pub fn set(&mut self, index: usize, item: T) {
        *self.get_mut(index).expect("index within arena") = item;
    }

    /// Borrowed when the slice sits inside one chunk, copied when it spans a
    /// boundary.
    pub fn slice(&self, vs: VecSlice<T>) -> Cow<'_, [T]> {
        if vs.len == 0 {
            return Cow::Borrowed(&[]);
        }
        assert!(vs.end() <= self.len, "slice outside arena");
        let chunk = vs.start / Self::CHUNK;
        let offset = vs.start % Self::CHUNK;
        if (vs.end() - 1) / Self::CHUNK == chunk {
            Cow::Borrowed(&self.chunks[chunk][offset..offset + vs.len])
        } else {
            Cow::Owned(self.range(vs.start, vs.end()).cloned().collect())
        }
    }

    /// Appends `item` to `slice`, relocating the slice to the tail first if
    /// something else was pushed after it; `None` if the slice is invalid.
    pub fn push_to_slice(&mut self, slice: &VecSlice<T>, item: T) -> Option<VecSlice<T>> {
        let mut slice = *slice;
        if slice.end() > self.len {
            return None;
        }
        if slice.end() < self.len {
            let moved = self.slice(slice).into_owned();
            slice.start = self.len;
            self.extend_from_slice(&moved);
        }
        self.push(item);
        slice.len += 1;
        Some(slice)
    }
}

impl Arena<u8> {
    /// Reads to the end of `reader` straight into chunks.
    pub fn read_from(mut reader: impl std::io::Read) -> std::io::Result<Self> {
        use std::io::Read;
        let mut arena = Self::new();
        loop {
            let mut chunk = Vec::with_capacity(Self::CHUNK);
            reader
                .by_ref()
                .take(Self::CHUNK as u64)
                .read_to_end(&mut chunk)?;
            let read = chunk.len();
            if read > 0 {
                arena.len += read;
                arena.chunks.push(Arc::new(chunk));
            }
            if read < Self::CHUNK {
                return Ok(arena);
            }
        }
    }

    pub fn push_str(&mut self, string: &str) -> VecSlice<u8> {
        let start = self.len;
        self.extend_from_slice(string.as_bytes());
        VecSlice::new(start, string.len())
    }

    pub fn str_at(&self, vs: VecSlice<u8>) -> Cow<'_, str> {
        match self.slice(vs) {
            Cow::Borrowed(bytes) => String::from_utf8_lossy(bytes),
            Cow::Owned(bytes) => Cow::Owned(String::from_utf8_lossy(&bytes).into_owned()),
        }
    }
}

impl<T> Clone for Arena<T> {
    fn clone(&self) -> Self {
        Self {
            chunks: self.chunks.clone(),
            len: self.len,
        }
    }
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Index<usize> for Arena<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        self.get(index).expect("index within arena")
    }
}

impl<T: Clone> From<Vec<T>> for Arena<T> {
    fn from(items: Vec<T>) -> Self {
        let mut arena = Self::new();
        arena.extend_from_slice(&items);
        arena
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_empty() {
        let mut vec = vec![];
        let slice = VecSlice {
            start: 0,
            len: 0,
            phantom: PhantomData,
        };
        let slice = push(&mut vec, &slice, 4).unwrap();
        let slice = slice.slice(&vec);
        assert_eq!(slice, vec![4]);
        assert_eq!(vec, vec![4]);
    }

    #[test]
    fn push_trivial() {
        let mut vec = vec![1, 2, 3];
        let slice = VecSlice {
            start: 1,
            len: 2,
            phantom: PhantomData,
        };
        let slice = push(&mut vec, &slice, 4).unwrap();
        let slice = slice.slice(&vec);
        assert_eq!(slice, vec![2, 3, 4]);
        assert_eq!(vec, vec![1, 2, 3, 4]);
    }

    #[test]
    fn push_beginning() {
        let mut vec = vec![1, 2, 3];
        let slice = VecSlice {
            start: 0,
            len: 1,
            phantom: PhantomData,
        };
        let slice = push(&mut vec, &slice, 4).unwrap();
        let slice = slice.slice(&vec);
        assert_eq!(slice, vec![1, 4]);
        assert_eq!(vec, vec![1, 2, 3, 1, 4]);
    }

    #[test]
    fn push_just_at_end() {
        let mut vec = vec![1, 2, 3];
        let slice = VecSlice {
            start: 2,
            len: 1,
            phantom: PhantomData,
        };
        let slice = push(&mut vec, &slice, 4).unwrap();
        let slice = slice.slice(&vec);
        assert_eq!(slice, vec![3, 4]);
        assert_eq!(vec, vec![1, 2, 3, 4]);
    }

    #[test]
    fn push_outside() {
        let mut vec = vec![1, 2, 3];
        let slice = VecSlice {
            start: 3,
            len: 1,
            phantom: PhantomData,
        };
        let slice = push(&mut vec, &slice, 4);
        assert_eq!(slice, None);
        assert_eq!(vec, vec![1, 2, 3]);
    }

    #[test]
    fn arena_push_and_slice_across_chunk_boundary() {
        let mut arena: Arena<u64> = Arena::new();
        let chunk = Arena::<u64>::CHUNK;
        for i in 0..(chunk as u64 + 10) {
            arena.push(i);
        }
        assert_eq!(arena.len(), chunk + 10);
        assert_eq!(arena[chunk - 1], chunk as u64 - 1);
        assert_eq!(arena[chunk], chunk as u64);

        let inside = arena.slice(VecSlice::new(2, 3));
        assert!(matches!(inside, Cow::Borrowed(_)));
        assert_eq!(&*inside, &[2, 3, 4]);

        let spanning = arena.slice(VecSlice::new(chunk - 2, 4));
        assert!(matches!(spanning, Cow::Owned(_)));
        let expected: Vec<u64> = (chunk as u64 - 2..chunk as u64 + 2).collect();
        assert_eq!(&*spanning, expected.as_slice());
    }

    #[test]
    fn arena_clone_shares_chunks_and_writes_copy_only_the_touched_chunk() {
        let mut arena: Arena<u8> = Arena::new();
        arena.extend_from_slice(&vec![7u8; Arena::<u8>::CHUNK + 1]);
        let snapshot = arena.clone();
        assert_eq!(Arc::strong_count(&arena.chunks[0]), 2);
        assert_eq!(Arc::strong_count(&arena.chunks[1]), 2);

        arena.push(9);
        arena.set(0, 1);
        assert_eq!(Arc::strong_count(&snapshot.chunks[0]), 1);
        assert_eq!(Arc::strong_count(&snapshot.chunks[1]), 1);
        assert_eq!(snapshot.len(), Arena::<u8>::CHUNK + 1);
        assert_eq!(snapshot[0], 7);
        assert_eq!(arena[0], 1);
        assert_eq!(arena[Arena::<u8>::CHUNK + 1], 9);
    }

    #[test]
    fn arena_push_to_slice_relocates_like_vec_push() {
        let mut arena: Arena<u32> = Arena::from(vec![1, 2, 3]);
        let slice = VecSlice::new(0, 1);
        let slice = arena.push_to_slice(&slice, 4).unwrap();
        assert_eq!(&*arena.slice(slice), &[1, 4]);
        assert_eq!(
            arena.iter().copied().collect::<Vec<_>>(),
            vec![1, 2, 3, 1, 4]
        );
        assert!(arena.push_to_slice(&VecSlice::new(5, 1), 0).is_none());
    }

    #[test]
    fn arena_str_at_and_chunks_roundtrip() {
        let mut arena: Arena<u8> = Arena::new();
        let hello = arena.push_str("hello");
        arena.extend_from_slice(&vec![b'x'; Arena::<u8>::CHUNK]);
        let world = arena.push_str("world");
        assert_eq!(arena.str_at(hello), "hello");
        assert_eq!(arena.str_at(world), "world");
        let flat = arena.chunks().collect::<Vec<_>>().concat();
        assert_eq!(flat.len(), arena.len());
        assert_eq!(&flat[..5], b"hello");
    }

    #[test]
    fn arena_read_from_fills_chunks_like_extend() {
        let chunk = Arena::<u8>::CHUNK;
        for len in [0, 1, chunk - 1, chunk, chunk + 1, 3 * chunk, 3 * chunk + 7] {
            let bytes: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let arena = Arena::read_from(bytes.as_slice()).unwrap();
            assert_eq!(arena.len(), len);
            assert_eq!(arena.chunks.len(), len.div_ceil(chunk));
            assert_eq!(arena.chunks().collect::<Vec<_>>().concat(), bytes);
        }
    }

    #[test]
    fn arena_shared_tail_is_copied_once_at_full_chunk_capacity() {
        let chunk = Arena::<u32>::CHUNK;
        let mut arena: Arena<u32> = Arena::new();
        arena.extend(0..10);
        let snapshot = arena.clone();
        arena.push(10);
        let tail = arena.chunks[0].as_ptr();
        assert_eq!(arena.chunks[0].capacity(), chunk);
        arena.extend(11..chunk as u32);
        assert_eq!(
            arena.chunks[0].as_ptr(),
            tail,
            "no reallocation while filling"
        );
        assert_eq!(snapshot.len(), 10);
        assert_eq!(arena.len(), chunk);
    }
}
