//! Every payload is a length/depth header followed by exactly one zstd frame.

use crate::{ChunkLoc, Error, Result};

pub(super) const HEADER_LEN: usize = 9;

pub(super) fn encode(bytes: &[u8], byte_len: u32, depth: u8) -> Result<Vec<u8>> {
    let payload_len = u32::try_from(bytes.len())
        .map_err(|_| Error::invalid("compression payload exceeds u32"))?;
    let mut compressor = zstd::bulk::Compressor::new(3)?;
    compressor.set_parameter(zstd::zstd_safe::CParameter::ChecksumFlag(true))?;
    let compressed = compressor.compress(bytes)?;
    let mut out = Vec::with_capacity(HEADER_LEN + compressed.len());
    out.extend_from_slice(&byte_len.to_le_bytes());
    out.extend_from_slice(&payload_len.to_le_bytes());
    out.push(depth);
    out.extend_from_slice(&compressed);
    u32::try_from(out.len()).map_err(|_| Error::invalid("compressed record exceeds u32"))?;
    Ok(out)
}

pub(super) fn header(bytes: &[u8]) -> Result<(u32, u32, u8)> {
    if bytes.len() < HEADER_LEN {
        return Err(Error::corrupt("compression", "short record header"));
    }
    let byte_len = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    let payload_len = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let depth = bytes[8];
    // A delta has a 50-byte fixed part and at most one 8-byte range per byte.
    if depth > super::delta::MAX_DEPTH
        || (depth == 0 && payload_len != byte_len)
        || (depth > 0
            && (payload_len < 50 || u64::from(payload_len) > 50 + 9 * u64::from(byte_len)))
    {
        return Err(Error::corrupt(
            "compression",
            "invalid record lengths or depth",
        ));
    }
    Ok((byte_len, payload_len, depth))
}

pub(super) fn decode(loc: ChunkLoc, bytes: &[u8]) -> Result<Vec<u8>> {
    let (byte_len, payload_len, depth) = header(bytes)?;
    if byte_len != loc.byte_len || depth != loc.depth || bytes.len() != loc.stored_len as usize {
        return Err(Error::corrupt("compression", "record header mismatch"));
    }
    let frame = &bytes[HEADER_LEN..];
    if !frame.starts_with(&[0x28, 0xb5, 0x2f, 0xfd])
        || zstd::zstd_safe::find_frame_compressed_size(frame).ok() != Some(frame.len())
        || zstd::zstd_safe::get_frame_content_size(frame).ok() != Some(Some(u64::from(payload_len)))
    {
        return Err(Error::corrupt(
            "compression",
            "invalid zstd frame or length",
        ));
    }
    let decoded = zstd::bulk::decompress(frame, payload_len as usize)
        .map_err(|err| Error::corrupt("compression", err.to_string()))?;
    if decoded.len() != payload_len as usize {
        return Err(Error::corrupt("compression", "decoded length mismatch"));
    }
    Ok(decoded)
}

#[cfg(test)]
#[path = "compression_test.rs"]
mod tests;
