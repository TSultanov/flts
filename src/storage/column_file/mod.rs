//! A file of named columns: chunk payloads plus a directory.
//!
//! ```text
//! Header | SlotA@4096 | SlotB@8192 | payloads and directories from 12288, append-only
//! ```

use crate::record::Record;
use commit_slot::CommitSlot;
use header::Header;

mod codec;
mod column_meta;
mod commit_slot;
mod commit_token;
mod cursor;
mod file_source;
mod header;
mod reader;
mod updater;
mod writer;
mod writer_id;

pub use codec::Codec;
pub use column_meta::ColumnMeta;
pub use commit_token::CommitToken;
pub use reader::ColumnFile;
pub use updater::ColumnFileUpdater;
pub use writer::ColumnFileWriter;
pub use writer_id::WriterId;

const MAGIC: [u8; 4] = *b"DBS0";
const SLOT_MAGIC: [u8; 4] = *b"DBSC";
const FORMAT_VERSION: u16 = 1;

const SLOT_A: u64 = 4096;
const SLOT_B: u64 = 8192;
const HEAD_LEN: u64 = 12288;

const _: () = assert!(Header::SIZE as u64 <= SLOT_A);
const _: () = assert!(SLOT_A + CommitSlot::SIZE as u64 <= SLOT_B);
const _: () = assert!(SLOT_B + CommitSlot::SIZE as u64 <= HEAD_LEN);

#[cfg(test)]
mod column_file_test;
