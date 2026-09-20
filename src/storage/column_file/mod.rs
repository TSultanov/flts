//! A file of named columns: chunk payloads plus a directory.
//!
//! ```text
//! Header | SlotA@4096 | SlotB@8192 | Mark@12288 | payloads from 16384, append-only
//! ```
//!
//! Chunk and directory records contain two little-endian u32 lengths (reconstructed
//! bytes, decoded payload bytes), a u8 delta depth, and one checksummed zstd frame.
//! Full records have depth zero; delta records reconstruct against a compressed base.

use crate::record::Record;
use append_mark::AppendMark;
use commit_slot::CommitSlot;
use header::Header;

mod append_mark;
mod codec;
mod compaction;
#[cfg(all(test, feature = "macros"))]
pub(crate) use compaction::fail_rewrite_at;
mod compression;
mod delta;
pub use compaction::{CompactionPolicy, CompactionReport, MaintenanceOutcome, StorageStats};
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
pub use updater::{ColumnFileUpdater, CommitAttempt};

#[cfg(all(test, feature = "macros"))]
pub(crate) use updater::{CommitFault, fail_commit_at, fail_commit_times};
pub use writer::ColumnFileWriter;
pub use writer_id::WriterId;

const MAGIC: [u8; 4] = *b"DBS0";
const SLOT_MAGIC: [u8; 4] = *b"DBSC";
const MARK_MAGIC: [u8; 4] = *b"DBSA";
const FORMAT_VERSION: u16 = 1;

const SLOT_A: u64 = 4096;
const SLOT_B: u64 = 8192;
const APPEND_MARK: u64 = 12288;
const HEAD_LEN: u64 = 16384;

const _: () = assert!(Header::SIZE as u64 <= SLOT_A);
const _: () = assert!(SLOT_A + CommitSlot::SIZE as u64 <= SLOT_B);
const _: () = assert!(SLOT_B + CommitSlot::SIZE as u64 <= APPEND_MARK);
const _: () = assert!(APPEND_MARK + AppendMark::SIZE as u64 <= HEAD_LEN);

#[cfg(test)]
mod column_file_test;
