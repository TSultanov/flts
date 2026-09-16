//! A byte range in an `Arena<u8>`, for variable-width payloads.

use crate::record::Record;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Blob {
    pub start: u64,
    pub len: u32,
}

impl Blob {
    pub const EMPTY: Blob = Blob { start: 0, len: 0 };

    pub const fn new(start: u64, len: u32) -> Self {
        Self { start, len }
    }

    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    pub const fn end(self) -> u64 {
        self.start + self.len as u64
    }
}

impl Record for Blob {
    const SIZE: usize = 12;

    fn encode(&self, out: &mut [u8]) {
        out[..8].copy_from_slice(&self.start.to_le_bytes());
        out[8..].copy_from_slice(&self.len.to_le_bytes());
    }

    fn decode(src: &[u8]) -> Self {
        let mut start = [0u8; 8];
        start.copy_from_slice(&src[..8]);
        let mut len = [0u8; 4];
        len.copy_from_slice(&src[8..]);
        Blob {
            start: u64::from_le_bytes(start),
            len: u32::from_le_bytes(len),
        }
    }
}

#[cfg(test)]
#[path = "blob_test.rs"]
mod blob_test;
