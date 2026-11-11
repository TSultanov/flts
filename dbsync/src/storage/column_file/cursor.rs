//! Bounds-checked sequential reader over directory bytes.

use crate::error::{Error, Result};

pub(super) struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    pub(super) fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    pub(super) fn is_empty(&self) -> bool {
        self.at == self.bytes.len()
    }

    pub(super) fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(len)
            .ok_or_else(|| Error::corrupt("directory", "offset overflow"))?;
        if end > self.bytes.len() {
            return Err(Error::corrupt("directory", "ends mid-record"));
        }
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    pub(super) fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub(super) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    pub(super) fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }
}

#[cfg(test)]
#[path = "cursor_test.rs"]
mod cursor_test;
