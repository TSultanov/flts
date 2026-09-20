use super::cursor::Cursor;
use crate::storage::{ChunkLoc, fnv1a};
use crate::{Error, Result};

pub(super) const MAGIC: u32 = u32::MAX;
pub(super) const MAX_DEPTH: u8 = 1;

pub(super) fn put_loc(out: &mut Vec<u8>, loc: ChunkLoc) {
    out.extend_from_slice(&loc.offset.to_le_bytes());
    out.extend_from_slice(&loc.byte_len.to_le_bytes());
    out.extend_from_slice(&loc.rows.to_le_bytes());
    out.extend_from_slice(&loc.fnv.to_le_bytes());
    out.extend_from_slice(&loc.stored_len.to_le_bytes());
    out.push(loc.depth);
}

pub(super) fn get_loc(cursor: &mut Cursor<'_>) -> Result<ChunkLoc> {
    let loc = ChunkLoc {
        offset: cursor.u64()?,
        byte_len: cursor.u32()?,
        rows: cursor.u32()?,
        fnv: cursor.u64()?,
        stored_len: cursor.u32()?,
        depth: cursor.u8()?,
    };
    if loc.depth > MAX_DEPTH || loc.stored_len <= super::compression::HEADER_LEN as u32 {
        return Err(Error::corrupt("delta", "invalid encoding"));
    }
    Ok(loc)
}

pub(super) fn encode(base: ChunkLoc, before: &[u8], after: &[u8], width: usize) -> Option<Vec<u8>> {
    if base.depth >= MAX_DEPTH || after.len() > u32::MAX as usize {
        return None;
    }
    let width = width.max(1);
    let mut ranges = Vec::new();
    let mut at = 0;
    while at < after.len() {
        let end = (at + width).min(after.len());
        if before.get(at..end) == Some(&after[at..end]) {
            at = end;
            continue;
        }
        let start = at;
        at = end;
        while at < after.len() {
            let end = (at + width).min(after.len());
            if before.get(at..end) == Some(&after[at..end]) {
                break;
            }
            at = end;
        }
        ranges.push((start, at));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&(after.len() as u32).to_le_bytes());
    out.push(base.depth + 1);
    put_loc(&mut out, base);
    out.extend_from_slice(&(ranges.len() as u32).to_le_bytes());
    for (start, end) in ranges {
        out.extend_from_slice(&(start as u32).to_le_bytes());
        out.extend_from_slice(&((end - start) as u32).to_le_bytes());
        out.extend_from_slice(&after[start..end]);
    }
    out.extend_from_slice(&fnv1a(&out).to_le_bytes());
    Some(out)
}

pub(super) fn parse<'a>(loc: ChunkLoc, bytes: &'a [u8]) -> Result<(ChunkLoc, Cursor<'a>)> {
    let payload = bytes
        .len()
        .checked_sub(8)
        .ok_or_else(|| Error::corrupt("delta", "short record"))?;
    let checksum = u64::from_le_bytes(bytes[payload..].try_into().unwrap());
    if fnv1a(&bytes[..payload]) != checksum {
        return Err(Error::corrupt("delta", "failed checksum"));
    }
    let mut cursor = Cursor::new(&bytes[..payload]);
    if cursor.u32()? != MAGIC || cursor.u32()? != loc.byte_len || cursor.u8()? != loc.depth {
        return Err(Error::corrupt("delta", "header mismatch"));
    }
    let base = get_loc(&mut cursor)?;
    if base.depth + 1 != loc.depth
        || base.offset < super::HEAD_LEN
        || base
            .offset
            .checked_add(base.physical_len())
            .is_none_or(|end| end > loc.offset)
    {
        return Err(Error::corrupt("delta", "invalid base"));
    }
    Ok((base, cursor))
}

pub(super) fn apply(mut base: Vec<u8>, len: usize, mut cursor: Cursor<'_>) -> Result<Vec<u8>> {
    let old_len = base.len();
    if len > old_len.saturating_add(cursor.remaining()) {
        return Err(Error::corrupt("delta", "unrepresented length"));
    }
    base.resize(len, 0);
    let count = cursor.u32()?;
    let mut end = 0usize;
    let mut covered = old_len.min(len);
    for _ in 0..count {
        let start = cursor.u32()? as usize;
        let size = cursor.u32()? as usize;
        let next = start
            .checked_add(size)
            .ok_or_else(|| Error::corrupt("delta", "range overflow"))?;
        if size == 0 || start < end || next > len || start > covered {
            return Err(Error::corrupt("delta", "invalid range"));
        }
        base[start..next].copy_from_slice(cursor.take(size)?);
        covered = covered.max(next);
        end = next;
    }
    if covered != len || !cursor.is_empty() {
        return Err(Error::corrupt("delta", "incomplete record"));
    }
    Ok(base)
}

#[cfg(test)]
#[path = "delta_test.rs"]
mod tests;
