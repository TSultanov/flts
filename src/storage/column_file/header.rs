//! Fixed-size head record identifying the format.

use std::io;

use super::{FORMAT_VERSION, MAGIC};
use crate::record::Record;
use crate::storage::arena::invalid;

#[derive(Clone, Copy)]
#[repr(C, packed)]
pub(super) struct Header {
    magic: [u8; 4],
    version: u16,
    reserved: u16,
}

const _: () = assert!(Header::SIZE == 8);

impl Header {
    pub(super) fn current() -> Self {
        Self {
            magic: MAGIC,
            version: FORMAT_VERSION,
            reserved: 0,
        }
    }

    pub(super) fn validate(self) -> io::Result<()> {
        if self.magic != MAGIC {
            return Err(invalid("bad column file magic"));
        }
        let version = self.version;
        if version != FORMAT_VERSION {
            return Err(invalid(format!(
                "unsupported column file version {version}"
            )));
        }
        Ok(())
    }
}

impl Record for Header {
    const SIZE: usize = size_of::<Self>();

    fn encode(&self, out: &mut [u8]) {
        let (magic, version, reserved) = (self.magic, self.version, self.reserved);
        out[..4].copy_from_slice(&magic);
        out[4..6].copy_from_slice(&version.to_le_bytes());
        out[6..].copy_from_slice(&reserved.to_le_bytes());
    }

    fn decode(src: &[u8]) -> Self {
        Self {
            magic: src[..4].try_into().expect("4 bytes"),
            version: u16::from_le_bytes(src[4..6].try_into().expect("2 bytes")),
            reserved: u16::from_le_bytes(src[6..].try_into().expect("2 bytes")),
        }
    }
}

#[cfg(test)]
#[path = "header_test.rs"]
mod header_test;
