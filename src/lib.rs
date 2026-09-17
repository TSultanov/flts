//! Disk-backed columnar storage with copy-on-write frozen views.

extern crate self as dbsync;

pub mod error;
pub mod id;
pub mod record;
pub mod schema;
pub mod storage;

#[cfg(test)]
mod test_utils;

pub use error::{Error, Result};
pub use id::Id;
pub use record::Record;
pub use storage::{
    Arena, Blob, ChunkLoc, ChunkSource, ColumnFile, ColumnFileUpdater, ColumnFileWriter,
    ColumnMeta, CommitToken, WriterId, default_chunk_rows, fnv1a,
};
