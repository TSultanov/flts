use ringbuffer::{AllocRingBuffer, RingBuffer};

use super::soa_helpers::VecSlice;
use std::{hash::Hasher, io::{self, ErrorKind}};

pub trait Serializable {
    fn serialize<TWriter: io::Write>(&self, output_stream: &mut TWriter) -> io::Result<()>;
    fn deserialize<TReader: io::Read + Clone>(input_stream: &mut TReader) -> io::Result<Self>
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
pub fn write_len_prefixed_bytes(w: &mut dyn io::Write, bytes: &[u8]) -> io::Result<()> {
    write_u64(w, bytes.len() as u64)?;
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
pub fn read_exact_array<const N: usize>(r: &mut dyn io::Read) -> io::Result<[u8; N]> {
    let mut buf = [0u8; N];
    r.read_exact(&mut buf)?;
    Ok(buf)
}
pub fn read_len_prefixed_vec(r: &mut dyn io::Read) -> io::Result<Vec<u8>> {
    let len = read_u64(r)? as usize;
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
        let s = read_u64(r)? as usize;
        let l = read_u64(r)? as usize;
        Ok(Some(VecSlice::new(s, l)))
    } else {
        Ok(None)
    }
}

// Generic slice helpers (for VecSlice<T>)
pub fn write_vec_slice<T>(w: &mut dyn io::Write, slice: &VecSlice<T>) -> io::Result<()> {
    write_u64(w, slice.start as u64)?;
    write_u64(w, slice.len as u64)
}
pub fn read_vec_slice<T>(r: &mut dyn io::Read) -> io::Result<VecSlice<T>> {
    let start = read_u64(r)? as usize;
    let len = read_u64(r)? as usize;
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

pub struct ChecksumedWriter<'a, T: io::Write> {
    backing_writer: &'a mut T,
    hasher: fnv::FnvHasher,
}

impl<'a, T: io::Write> ChecksumedWriter<'a, T> {
    pub fn create(backing_writer: &'a mut T) -> Self {
        ChecksumedWriter {
            backing_writer: backing_writer,
            hasher: fnv::FnvHasher::default(),
        }
    }

    pub fn current_hash(&self) -> u64 {
        self.hasher.finish()
    }
}

impl<'a, T: io::Write> io::Write for ChecksumedWriter<'a, T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.hasher.write(buf);

        self.backing_writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.backing_writer.flush()
    }
}

pub fn validate_hash<T: io::Read + Clone>(reader: &mut T) -> io::Result<bool> {
    let mut reader = reader.clone();
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

    if last_u64.len() < 8 || last_hashes.len() < 9 {
        return Err(io::Error::new(ErrorKind::InvalidData, "Not enough data"));
    }

    let read_hash = u64::from_le_bytes(last_u64.into_iter().collect::<Vec<_>>().try_into().unwrap());
    let computed_hash = *last_hashes.front().unwrap();

    Ok(read_hash == computed_hash)
}
