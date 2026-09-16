//! Where the next commit may start writing, kept out of reach of a lost slot.

use std::fs::File;

use super::file_source::read_exact_at;
use super::{APPEND_MARK, MARK_MAGIC};
use crate::error::Result;
use crate::record::Record;
use crate::storage::hash::fnv1a;

#[derive(Clone, Copy)]
#[repr(C, packed)]
pub(super) struct AppendMark {
    magic: [u8; 4],
    reserved: u32,
    file_uuid: u128,
    pub(super) append_from: u64,
    pub(super) generation: u64,
    mark_fnv: u64,
}

const _: () = assert!(AppendMark::SIZE == 48);

const SIGNED_LEN: usize = AppendMark::SIZE - 8;

impl AppendMark {
    pub(super) fn new(file_uuid: u128, append_from: u64, generation: u64) -> Self {
        Self {
            magic: MARK_MAGIC,
            reserved: 0,
            file_uuid,
            append_from,
            generation,
            mark_fnv: 0,
        }
    }

    pub(super) fn to_bytes(self) -> [u8; AppendMark::SIZE] {
        let mut bytes = [0u8; AppendMark::SIZE];
        self.encode(&mut bytes);
        let signature = fnv1a(&bytes[..SIGNED_LEN]);
        bytes[SIGNED_LEN..].copy_from_slice(&signature.to_le_bytes());
        bytes
    }

    pub(super) fn from_bytes(bytes: &[u8], file_uuid: u128) -> Option<Self> {
        let mark = Self::decode(bytes);
        if mark.magic != MARK_MAGIC {
            return None;
        }
        if fnv1a(&bytes[..SIGNED_LEN]) != mark.mark_fnv {
            return None;
        }
        if mark.generation == 0 || mark.file_uuid != file_uuid {
            return None;
        }
        Some(mark)
    }
}

pub(super) fn read_append_mark(file: &File, file_uuid: u128) -> Result<Option<AppendMark>> {
    let mut bytes = [0u8; AppendMark::SIZE];
    read_exact_at(file, &mut bytes, APPEND_MARK)?;
    Ok(AppendMark::from_bytes(&bytes, file_uuid))
}

impl Record for AppendMark {
    const SIZE: usize = size_of::<Self>();

    fn encode(&self, out: &mut [u8]) {
        let magic = self.magic;
        out[..4].copy_from_slice(&magic);
        out[4..8].copy_from_slice(&{ self.reserved }.to_le_bytes());
        out[8..24].copy_from_slice(&{ self.file_uuid }.to_le_bytes());
        out[24..32].copy_from_slice(&{ self.append_from }.to_le_bytes());
        out[32..40].copy_from_slice(&{ self.generation }.to_le_bytes());
        out[40..].copy_from_slice(&{ self.mark_fnv }.to_le_bytes());
    }

    fn decode(src: &[u8]) -> Self {
        Self {
            magic: src[..4].try_into().expect("4 bytes"),
            reserved: u32::from_le_bytes(src[4..8].try_into().expect("4 bytes")),
            file_uuid: u128::from_le_bytes(src[8..24].try_into().expect("16 bytes")),
            append_from: u64::from_le_bytes(src[24..32].try_into().expect("8 bytes")),
            generation: u64::from_le_bytes(src[32..40].try_into().expect("8 bytes")),
            mark_fnv: u64::from_le_bytes(src[40..].try_into().expect("8 bytes")),
        }
    }
}

#[cfg(test)]
#[path = "append_mark_test.rs"]
mod append_mark_test;
