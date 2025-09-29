use super::soa_helpers::VecSlice;

pub trait Serializable {
    fn serialize(&self, output_stream: &mut dyn std::io::Write) -> std::io::Result<()>;
    fn deserialize(input_stream: &mut dyn std::io::Read) -> std::io::Result<Self>
    where
        Self: Sized;
}

// Common binary helpers (little-endian)
pub fn write_u8(w: &mut dyn std::io::Write, v: u8) -> std::io::Result<()> {
    w.write_all(&[v])
}
pub fn write_u64(w: &mut dyn std::io::Write, v: u64) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
pub fn write_len_prefixed_bytes(w: &mut dyn std::io::Write, bytes: &[u8]) -> std::io::Result<()> {
    write_u64(w, bytes.len() as u64)?;
    w.write_all(bytes)
}
pub fn write_len_prefixed_str(w: &mut dyn std::io::Write, s: &str) -> std::io::Result<()> {
    write_len_prefixed_bytes(w, s.as_bytes())
}

pub fn read_u8(r: &mut dyn std::io::Read) -> std::io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}
pub fn read_u64(r: &mut dyn std::io::Read) -> std::io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}
pub fn read_exact_array<const N: usize>(r: &mut dyn std::io::Read) -> std::io::Result<[u8; N]> {
    let mut buf = [0u8; N];
    r.read_exact(&mut buf)?;
    Ok(buf)
}
pub fn read_len_prefixed_vec(r: &mut dyn std::io::Read) -> std::io::Result<Vec<u8>> {
    let len = read_u64(r)? as usize;
    let mut v = vec![0u8; len];
    r.read_exact(&mut v)?;
    Ok(v)
}
pub fn read_len_prefixed_string(r: &mut dyn std::io::Read) -> std::io::Result<String> {
    let v = read_len_prefixed_vec(r)?;
    String::from_utf8(v)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid UTF-8"))
}

// Generic slice helpers (for VecSlice<T>)
pub fn write_vec_slice<T>(w: &mut dyn std::io::Write, slice: &VecSlice<T>) -> std::io::Result<()> {
    write_u64(w, slice.start as u64)?; write_u64(w, slice.len as u64)
}
pub fn read_vec_slice<T>(r: &mut dyn std::io::Read) -> std::io::Result<VecSlice<T>> {
    let start = read_u64(r)? as usize; let len = read_u64(r)? as usize; Ok(VecSlice::new(start,len))
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

    pub fn write(&self, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        w.write_all(self.as_bytes())
    }

    pub fn read(expected: Magic, r: &mut dyn std::io::Read) -> std::io::Result<()> {
        let mut buf = [0u8; 4];
        r.read_exact(&mut buf)?;
        if &buf != expected.as_bytes() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid magic",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Version {
    V1,
}

impl Version {
    pub fn write_version(&self, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        write_u8(w, 1)
    }
    pub fn read_version(r: &mut dyn std::io::Read) -> std::io::Result<Self> {
        let v = read_u8(r)?;
        if v == 1 {
            Ok(Version::V1)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Unsupported version",
            ))
        }
    }
}
