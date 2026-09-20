//! How a chunk's bytes are encoded.

use crate::error::{Error, Result};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Codec {
    Zstd = 1,
}

impl Codec {
    pub(super) fn from_u8(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Codec::Zstd),
            other => Err(Error::unsupported(format!("unknown codec {other}"))),
        }
    }
}

#[cfg(test)]
#[path = "codec_test.rs"]
mod codec_test;
