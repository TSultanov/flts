//! The record that publishes a generation, written to one of two slots.

use std::fs::File;

use super::commit_token::CommitToken;
use super::file_source::read_exact_at;
use super::writer_id::WriterId;
use super::{SLOT_A, SLOT_B, SLOT_MAGIC};
use crate::error::Result;
use crate::record::Record;
use crate::storage::hash::fnv1a;

#[derive(Clone, Copy)]
#[repr(C, packed)]
pub(super) struct CommitSlot {
    magic: [u8; 4],
    reserved: u32,
    pub(super) generation: u64,
    pub(super) content_id: u64,
    pub(super) file_uuid: u128,
    pub(super) writer_id: u64,
    pub(super) dir_offset: u64,
    pub(super) dir_len: u64,
    pub(super) dir_fnv: u64,
    slot_fnv: u64,
}

const _: () = assert!(CommitSlot::SIZE == 80);

const SIGNED_LEN: usize = CommitSlot::SIZE - 8;

impl CommitSlot {
    pub(super) fn new(
        generation: u64,
        content_id: u64,
        file_uuid: u128,
        writer_id: WriterId,
        dir_offset: u64,
        dir_len: u64,
        dir_fnv: u64,
    ) -> Self {
        Self {
            magic: SLOT_MAGIC,
            reserved: 0,
            generation,
            content_id,
            file_uuid,
            writer_id: writer_id.raw(),
            dir_offset,
            dir_len,
            dir_fnv,
            slot_fnv: 0,
        }
    }

    pub(super) fn to_bytes(self) -> [u8; CommitSlot::SIZE] {
        let mut bytes = [0u8; CommitSlot::SIZE];
        self.encode(&mut bytes);
        let signature = fnv1a(&bytes[..SIGNED_LEN]);
        bytes[SIGNED_LEN..].copy_from_slice(&signature.to_le_bytes());
        bytes
    }

    pub(super) fn dir_end(self) -> Option<u64> {
        self.dir_offset.checked_add(self.dir_len)
    }

    pub(super) fn token(self) -> CommitToken {
        CommitToken {
            generation: self.generation,
            content_id: self.content_id,
            writer_id: WriterId::from_raw(self.writer_id),
            file_uuid: self.file_uuid,
        }
    }

    pub(super) fn from_bytes(bytes: &[u8], file_uuid: u128) -> Option<Self> {
        let slot = Self::decode(bytes);
        if slot.magic != SLOT_MAGIC {
            return None;
        }
        if fnv1a(&bytes[..SIGNED_LEN]) != slot.slot_fnv {
            return None;
        }
        if slot.generation == 0 || slot.file_uuid != file_uuid {
            return None;
        }
        Some(slot)
    }
}

pub(super) type FoundSlot = Option<(u64, CommitSlot)>;

pub(super) fn newest_slot(file: &File, file_uuid: u128) -> Result<FoundSlot> {
    let mut newest: FoundSlot = None;
    for at in [SLOT_A, SLOT_B] {
        let mut bytes = [0u8; CommitSlot::SIZE];
        read_exact_at(file, &mut bytes, at)?;
        if let Some(slot) = CommitSlot::from_bytes(&bytes, file_uuid)
            && newest.is_none_or(|(_, current)| slot.generation > current.generation)
        {
            newest = Some((at, slot));
        }
    }
    Ok(newest)
}

pub(super) fn slots_newest_first(file: &File, file_uuid: u128) -> Result<Vec<CommitSlot>> {
    let mut held = Vec::new();
    for at in [SLOT_A, SLOT_B] {
        let mut bytes = [0u8; CommitSlot::SIZE];
        read_exact_at(file, &mut bytes, at)?;
        if let Some(slot) = CommitSlot::from_bytes(&bytes, file_uuid) {
            held.push(slot);
        }
    }
    held.sort_by_key(|slot| std::cmp::Reverse(slot.generation));
    Ok(held)
}

impl Record for CommitSlot {
    const SIZE: usize = size_of::<Self>();

    fn encode(&self, out: &mut [u8]) {
        let magic = self.magic;
        out[..4].copy_from_slice(&magic);
        out[4..8].copy_from_slice(&{ self.reserved }.to_le_bytes());
        out[8..16].copy_from_slice(&{ self.generation }.to_le_bytes());
        out[16..24].copy_from_slice(&{ self.content_id }.to_le_bytes());
        out[24..40].copy_from_slice(&{ self.file_uuid }.to_le_bytes());
        out[40..48].copy_from_slice(&{ self.writer_id }.to_le_bytes());
        out[48..56].copy_from_slice(&{ self.dir_offset }.to_le_bytes());
        out[56..64].copy_from_slice(&{ self.dir_len }.to_le_bytes());
        out[64..72].copy_from_slice(&{ self.dir_fnv }.to_le_bytes());
        out[72..].copy_from_slice(&{ self.slot_fnv }.to_le_bytes());
    }

    fn decode(src: &[u8]) -> Self {
        Self {
            magic: src[..4].try_into().expect("4 bytes"),
            reserved: u32::from_le_bytes(src[4..8].try_into().expect("4 bytes")),
            generation: u64::from_le_bytes(src[8..16].try_into().expect("8 bytes")),
            content_id: u64::from_le_bytes(src[16..24].try_into().expect("8 bytes")),
            file_uuid: u128::from_le_bytes(src[24..40].try_into().expect("16 bytes")),
            writer_id: u64::from_le_bytes(src[40..48].try_into().expect("8 bytes")),
            dir_offset: u64::from_le_bytes(src[48..56].try_into().expect("8 bytes")),
            dir_len: u64::from_le_bytes(src[56..64].try_into().expect("8 bytes")),
            dir_fnv: u64::from_le_bytes(src[64..72].try_into().expect("8 bytes")),
            slot_fnv: u64::from_le_bytes(src[72..].try_into().expect("8 bytes")),
        }
    }
}

#[cfg(test)]
#[path = "commit_slot_test.rs"]
mod commit_slot_test;
