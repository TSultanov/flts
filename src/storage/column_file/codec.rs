//! How a chunk's bytes are encoded.

use std::io;

use crate::storage::arena::invalid;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Codec {
    Raw = 0,
}

impl Codec {
    pub(super) fn from_u8(value: u8) -> io::Result<Self> {
        match value {
            0 => Ok(Codec::Raw),
            other => Err(invalid(format!("unknown codec {other}"))),
        }
    }
}

#[cfg(test)]
#[path = "codec_test.rs"]
mod codec_test;
