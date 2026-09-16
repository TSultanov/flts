//! Fixed-size tail record locating the directory.

use std::io;

use super::MAGIC;
use crate::record::Record;
use crate::storage::arena::invalid;

#[derive(Clone, Copy)]
#[repr(C, packed)]
pub(super) struct Footer {
    pub(super) dir_offset: u64,
    pub(super) dir_len: u64,
    pub(super) dir_fnv: u64,
    magic: [u8; 4],
}

const _: () = assert!(Footer::SIZE == 28);

impl Footer {
    pub(super) fn new(dir_offset: u64, dir_len: u64, dir_fnv: u64) -> Self {
        Self {
            dir_offset,
            dir_len,
            dir_fnv,
            magic: MAGIC,
        }
    }

    pub(super) fn validate(self) -> io::Result<()> {
        if self.magic != MAGIC {
            return Err(invalid("bad column file footer magic; file is truncated"));
        }
        Ok(())
    }
}

impl Record for Footer {
    const SIZE: usize = size_of::<Self>();

    fn encode(&self, out: &mut [u8]) {
        let (dir_offset, dir_len, dir_fnv, magic) =
            (self.dir_offset, self.dir_len, self.dir_fnv, self.magic);
        out[..8].copy_from_slice(&dir_offset.to_le_bytes());
        out[8..16].copy_from_slice(&dir_len.to_le_bytes());
        out[16..24].copy_from_slice(&dir_fnv.to_le_bytes());
        out[24..].copy_from_slice(&magic);
    }

    fn decode(src: &[u8]) -> Self {
        Self {
            dir_offset: u64::from_le_bytes(src[..8].try_into().expect("8 bytes")),
            dir_len: u64::from_le_bytes(src[8..16].try_into().expect("8 bytes")),
            dir_fnv: u64::from_le_bytes(src[16..24].try_into().expect("8 bytes")),
            magic: src[24..].try_into().expect("4 bytes"),
        }
    }
}

#[cfg(test)]
#[path = "footer_test.rs"]
mod footer_test;
