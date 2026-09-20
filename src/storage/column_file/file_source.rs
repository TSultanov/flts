//! A `ChunkSource` backed by a file, read by offset.

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::storage::arena::{ChunkLoc, ChunkSource};

pub(super) struct FileSource {
    file: File,
    path: PathBuf,
}

impl FileSource {
    pub(super) fn from_file(file: File, path: PathBuf) -> Self {
        Self { file, path }
    }

    pub(super) fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = std::fs::canonicalize(path.as_ref())?;
        Ok(Self {
            file: File::open(&path)?,
            path,
        })
    }

    pub(super) fn raw_base(&self, mut loc: ChunkLoc) -> Result<ChunkLoc> {
        if loc.depth > super::delta::MAX_DEPTH {
            return Err(crate::Error::corrupt("delta", "chain too deep"));
        }
        while loc.depth > 0 {
            if loc
                .offset
                .checked_add(loc.physical_len())
                .is_none_or(|end| end > self.len().unwrap_or(0))
            {
                return Err(crate::Error::corrupt("delta", "invalid extent"));
            }
            let mut bytes = vec![0; loc.physical_len() as usize];
            self.read_at(&mut bytes, loc.offset)?;
            loc = super::delta::parse(loc, &bytes)?.0;
        }
        Ok(loc)
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn file(&self) -> &File {
        &self.file
    }

    pub(super) fn len(&self) -> Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    pub(super) fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<()> {
        read_exact_at(&self.file, buf, offset)
    }
}

impl ChunkSource for FileSource {
    fn read_chunk(&self, loc: ChunkLoc) -> Result<Vec<u8>> {
        if loc.depth > super::delta::MAX_DEPTH
            || loc
                .offset
                .checked_add(loc.physical_len())
                .is_none_or(|end| end > self.len().unwrap_or(0))
        {
            return Err(crate::Error::corrupt("chunk", "invalid location"));
        }
        let mut buf = vec![0u8; loc.physical_len() as usize];
        self.read_at(&mut buf, loc.offset)?;
        let bytes = if loc.depth == 0 {
            buf
        } else {
            let (base, cursor) = super::delta::parse(loc, &buf)?;
            super::delta::apply(self.read_chunk(base)?, loc.byte_len as usize, cursor)?
        };
        if crate::storage::fnv1a(&bytes) != loc.fnv {
            return Err(crate::Error::corrupt("chunk", "failed checksum"));
        }
        Ok(bytes)
    }
}

#[cfg(unix)]
pub(super) fn write_all_at(file: &File, buf: &[u8], offset: u64) -> Result<()> {
    use std::os::unix::fs::FileExt;
    Ok(file.write_all_at(buf, offset)?)
}

#[cfg(windows)]
pub(super) fn write_all_at(file: &File, buf: &[u8], offset: u64) -> Result<()> {
    use std::io::{Error, ErrorKind};
    use std::os::windows::fs::FileExt;

    let mut rest = buf;
    let mut at = offset;
    while !rest.is_empty() {
        match file.seek_write(rest, at) {
            Ok(0) => {
                return Err(Error::new(ErrorKind::WriteZero, "short positioned write").into());
            }
            Ok(written) => {
                rest = &rest[written..];
                at += written as u64;
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> Result<()> {
    use std::os::unix::fs::FileExt;
    Ok(file.read_exact_at(buf, offset)?)
}

#[cfg(windows)]
pub(super) fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> Result<()> {
    use std::io::{Error, ErrorKind};
    use std::os::windows::fs::FileExt;

    let mut rest = buf;
    let mut at = offset;
    while !rest.is_empty() {
        match file.seek_read(rest, at) {
            Ok(0) => {
                return Err(Error::new(ErrorKind::UnexpectedEof, "short chunk read").into());
            }
            Ok(read) => {
                let (_, tail) = std::mem::take(&mut rest).split_at_mut(read);
                rest = tail;
                at += read as u64;
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "file_source_test.rs"]
mod file_source_test;
