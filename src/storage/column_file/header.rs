//! Fixed-size head record identifying the format and the file body.

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
    file_uuid: u128,
}

const _: () = assert!(Header::SIZE == 24);

impl Header {
    pub(super) fn new(file_uuid: u128) -> Self {
        Self {
            magic: MAGIC,
            version: FORMAT_VERSION,
            reserved: 0,
            file_uuid,
        }
    }

    pub(super) fn file_uuid(self) -> u128 {
        self.file_uuid
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

pub(super) fn new_file_uuid() -> u128 {
    ((entropy() as u128) << 64) | entropy() as u128
}

fn entropy() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0),
    );
    hasher.finish()
}

impl Record for Header {
    const SIZE: usize = size_of::<Self>();

    fn encode(&self, out: &mut [u8]) {
        let (magic, version, reserved, file_uuid) =
            (self.magic, self.version, self.reserved, self.file_uuid);
        out[..4].copy_from_slice(&magic);
        out[4..6].copy_from_slice(&version.to_le_bytes());
        out[6..8].copy_from_slice(&reserved.to_le_bytes());
        out[8..].copy_from_slice(&file_uuid.to_le_bytes());
    }

    fn decode(src: &[u8]) -> Self {
        Self {
            magic: src[..4].try_into().expect("4 bytes"),
            version: u16::from_le_bytes(src[4..6].try_into().expect("2 bytes")),
            reserved: u16::from_le_bytes(src[6..8].try_into().expect("2 bytes")),
            file_uuid: u128::from_le_bytes(src[8..].try_into().expect("16 bytes")),
        }
    }
}

#[cfg(test)]
#[path = "header_test.rs"]
mod header_test;
