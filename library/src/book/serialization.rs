use ringbuffer::{AllocRingBuffer, RingBuffer};

use super::soa_helpers::VecSlice;
use std::{
    hash::Hasher,
    io::{self, ErrorKind},
};

pub trait Serializable {
    fn serialize<TWriter: io::Write>(&self, output_stream: &mut TWriter) -> io::Result<()>;
    fn deserialize<TReader: io::Seek + io::Read>(input_stream: &mut TReader) -> io::Result<Self>
    where
        Self: Sized;
}

// Common binary helpers (little-endian)
pub fn write_u8(w: &mut dyn io::Write, v: u8) -> io::Result<()> {
    w.write_all(&[v])
}
pub fn write_u64(w: &mut dyn io::Write, v: u64) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
pub fn write_var_u64(w: &mut dyn io::Write, mut v: u64) -> io::Result<()> {
    while v >= 0x80 {
        // Set continuation bit
        let b = ((v as u8) & 0x7F) | 0x80;
        w.write_all(&[b])?;
        v >>= 7;
    }
    w.write_all(&[v as u8])?;
    Ok(())
}
pub fn write_len_prefixed_bytes(w: &mut dyn io::Write, bytes: &[u8]) -> io::Result<()> {
    write_var_u64(w, bytes.len() as u64)?;
    w.write_all(bytes)
}
pub fn write_len_prefixed_str(w: &mut dyn io::Write, s: &str) -> io::Result<()> {
    write_len_prefixed_bytes(w, s.as_bytes())
}
pub fn write_opt(w: &mut dyn io::Write, slice: &Option<VecSlice<u8>>) -> io::Result<()> {
    match slice {
        Some(s) => {
            w.write_all(&[1])?;
            w.write_all(&(s.start as u64).to_le_bytes())?;
            w.write_all(&(s.len as u64).to_le_bytes())?;
        }
        None => {
            w.write_all(&[0])?;
        }
    }
    Ok(())
}

pub fn read_u8(r: &mut dyn io::Read) -> io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}
pub fn read_u64(r: &mut dyn io::Read) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}
pub fn read_var_u64(r: &mut dyn io::Read) -> io::Result<u64> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        if shift >= 64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Varint too long",
            ));
        }
        let mut b = [0u8; 1];
        r.read_exact(&mut b)?;
        let byte = b[0];
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    Ok(result)
}
pub fn read_exact_array<const N: usize>(r: &mut dyn io::Read) -> io::Result<[u8; N]> {
    let mut buf = [0u8; N];
    r.read_exact(&mut buf)?;
    Ok(buf)
}
pub fn read_len_prefixed_vec(r: &mut dyn io::Read) -> io::Result<Vec<u8>> {
    let len = read_var_u64(r)? as usize;
    let mut v = vec![0u8; len];
    r.read_exact(&mut v)?;
    Ok(v)
}
pub fn read_len_prefixed_string(r: &mut dyn io::Read) -> io::Result<String> {
    let v = read_len_prefixed_vec(r)?;
    String::from_utf8(v).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid UTF-8"))
}
pub fn read_opt(r: &mut dyn io::Read) -> io::Result<Option<VecSlice<u8>>> {
    let has = read_u8(r)?;
    if has == 1 {
        let s = read_var_u64(r)? as usize;
        let l = read_var_u64(r)? as usize;
        Ok(Some(VecSlice::new(s, l)))
    } else {
        Ok(None)
    }
}

// Generic slice helpers (for VecSlice<T>)
pub fn write_vec_slice<T>(w: &mut dyn io::Write, slice: &VecSlice<T>) -> io::Result<()> {
    write_var_u64(w, slice.start as u64)?;
    write_var_u64(w, slice.len as u64)
}
pub fn read_vec_slice<T>(r: &mut dyn io::Read) -> io::Result<VecSlice<T>> {
    let start = read_var_u64(r)? as usize;
    let len = read_var_u64(r)? as usize;
    Ok(VecSlice::new(start, len))
}

// Magic identifiers for binary blobs (4 bytes)
pub enum Magic {
    Book,
    Translation,
}

impl Magic {
    pub fn as_bytes(&self) -> &'static [u8; 4] {
        match self {
            Magic::Book => b"BK01", // includes version indicator but still treat version separately
            Magic::Translation => b"TR01",
        }
    }

    pub fn write(&self, w: &mut dyn io::Write) -> io::Result<()> {
        w.write_all(self.as_bytes())
    }

    pub fn read(expected: Magic, r: &mut dyn io::Read) -> io::Result<()> {
        let mut buf = [0u8; 4];
        r.read_exact(&mut buf)?;
        if &buf != expected.as_bytes() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid magic"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Version {
    V1,
}

impl Version {
    pub fn write_version(&self, w: &mut dyn io::Write) -> io::Result<()> {
        write_u8(w, 1)
    }
    pub fn read_version(r: &mut dyn io::Read) -> io::Result<Self> {
        let v = read_u8(r)?;
        if v == 1 {
            Ok(Version::V1)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unsupported version",
            ))
        }
    }
}

pub struct ChecksumedWriter<'a> {
    backing_writer: &'a mut dyn io::Write,
    hasher: fnv::FnvHasher,
}

impl<'a> ChecksumedWriter<'a> {
    pub fn create(backing_writer: &'a mut dyn io::Write) -> Self {
        ChecksumedWriter {
            backing_writer: backing_writer,
            hasher: fnv::FnvHasher::default(),
        }
    }

    pub fn current_hash(&self) -> u64 {
        self.hasher.finish()
    }
}

impl<'a> io::Write for ChecksumedWriter<'a> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.hasher.write(buf);

        self.backing_writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.backing_writer.flush()
    }
}

pub fn validate_hash<T: io::Seek + io::Read>(reader: &mut T) -> io::Result<bool> {
    let mut hasher = fnv::FnvHasher::default();

    let mut last_u64 = AllocRingBuffer::new(8);
    let mut last_hashes = AllocRingBuffer::new(9);
    let mut b = [0u8; 1];
    loop {
        let data = reader.read_exact(&mut b);
        if let Some(err) = data.err() {
            match err.kind() {
                ErrorKind::UnexpectedEof => {
                    break;
                }
                _ => {
                    return Err(err);
                }
            }
        }

        last_u64.enqueue(b[0]);
        hasher.write(&b);
        last_hashes.enqueue(hasher.finish());
    }

    reader.seek(io::SeekFrom::Start(0));

    if last_u64.len() < 8 || last_hashes.len() < 9 {
        return Err(io::Error::new(ErrorKind::InvalidData, "Not enough data"));
    }

    let read_hash =
        u64::from_le_bytes(last_u64.into_iter().collect::<Vec<_>>().try_into().unwrap());
    let computed_hash = *last_hashes.front().unwrap();

    Ok(read_hash == computed_hash)
}

#[cfg(test)]
mod serialization_tests {
    use super::*;
    use std::io::Cursor;

    fn encode(v: u64) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        write_var_u64(&mut buf, v).unwrap();
        buf
    }

    fn decode(bytes: &[u8]) -> u64 {
        let mut cur = Cursor::new(bytes.to_vec());
        read_var_u64(&mut cur).unwrap()
    }

    #[test]
    fn test_known_encodings() {
        // (value, expected_bytes)
        let cases: &[(u64, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (2, &[0x02]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]), // 0b1_0000000 -> continuation then 1
            (129, &[0x81, 0x01]),
            (255, &[0xFF, 0x01]), // 0b11111111 -> low7 bits 0x7F + next byte 0x01
            (300, &[0xAC, 0x02]), // standard varint example
            (16384, &[0x80, 0x80, 0x01]), // 2^14
            // 4-byte maximum where each 7-bit group is all ones: (1 << 28) - 1
            (0x0FFF_FFFFu64, &[0xFF, 0xFF, 0xFF, 0x7F]),
            // 5-byte example: exactly 1 << 28 requires five groups
            (1u64 << 28, &[0x80, 0x80, 0x80, 0x80, 0x01]),
        ];
        for (v, expected) in cases.iter() {
            assert_eq!(&encode(*v), expected, "encoding mismatch for {v}");
            assert_eq!(
                decode(expected),
                *v,
                "decoding mismatch for bytes {:?}",
                expected
            );
        }
    }

    #[test]
    fn test_roundtrip_powers_of_two_and_boundaries() {
        let mut values = vec![0u64, 1, 2, 3, 127, 128, 129];
        // Add powers of two around 7-bit boundaries
        for shift in (7..=63).step_by(7) {
            // 7,14,21,...,63
            let base = 1u64 << shift;
            values.push(base - 1);
            values.push(base);
            values.push(base + 1);
        }
        values.push(u64::MAX);
        for v in values {
            let enc = encode(v);
            let dec = decode(&enc);
            assert_eq!(dec, v, "roundtrip failed for {v} -> {:?}", enc);
        }
    }

    #[test]
    fn test_streaming_multiple_varints_back_to_back() {
        let nums = [0u64, 1, 127, 128, 300, 16384, u32::MAX as u64, u64::MAX];
        let mut buf: Vec<u8> = Vec::new();
        for n in nums.iter() {
            write_var_u64(&mut buf, *n).unwrap();
        }
        let mut cursor = Cursor::new(buf);
        for expected in nums.iter() {
            let v = read_var_u64(&mut cursor).unwrap();
            assert_eq!(&v, expected);
        }
        // Ensure EOF afterwards
        let eof = read_var_u64(&mut cursor);
        assert!(eof.is_err());
        assert_eq!(eof.err().unwrap().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn test_incomplete_varint() {
        // 0x80 indicates continuation but stream ends
        let bytes = [0x80u8];
        let mut cur = Cursor::new(bytes);
        let r = read_var_u64(&mut cur);
        assert!(r.is_err());
        assert_eq!(r.err().unwrap().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn test_varint_too_long() {
        // Construct 11 bytes where the first 10 have continuation bit set; for valid u64 max is 10 bytes
        // This will create shift >= 64 and should error with InvalidData
        let bytes = vec![0x80u8; 11];
        // Last byte also has continuation bit to force loop past 64 bits
        let mut cur = Cursor::new(bytes);
        let r = read_var_u64(&mut cur);
        assert!(r.is_err());
        assert_eq!(r.err().unwrap().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_max_u64_encoding() {
        let v = u64::MAX; // 0xFFFF_FFFF_FFFF_FFFF
        let enc = encode(v);
        // Ensure no panic and decode back
        let dec = decode(&enc);
        assert_eq!(dec, v);
        // Length should be 10 bytes per standard LEB128 for 64-bit max
        assert_eq!(enc.len(), 10);
    }
}
