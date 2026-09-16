//! A file of named columns: chunk payloads plus a directory.
//!
//! ```text
//! Header | chunk payloads | directory | Footer
//! ```

use footer::Footer;
use header::Header;

use crate::record::Record;

mod codec;
mod column_meta;
mod cursor;
mod file_source;
mod footer;
mod header;
mod reader;
mod writer;

pub use codec::Codec;
pub use column_meta::ColumnMeta;
pub use reader::ColumnFile;
pub use writer::ColumnFileWriter;

const MAGIC: [u8; 4] = *b"DBS0";
const FORMAT_VERSION: u16 = 1;
const HEADER_LEN: u64 = Header::SIZE as u64;
const FOOTER_LEN: u64 = Footer::SIZE as u64;

#[cfg(test)]
mod column_file_test;
