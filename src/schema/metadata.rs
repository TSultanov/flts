//! Application schema identity, independent of the column file's format and generation.

use std::path::Path;

use crate::{ColumnFile, Error, Result};

const MAGIC: &[u8; 8] = b"DBSCHEMA";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaMetadata {
    /// Version of this metadata encoding, independent of application schema versions.
    pub format_version: u32,
    /// Stable application schema family declared on the container.
    pub name: String,
    /// Positive, monotonically increasing application version.
    pub version: u64,
    /// Exact schema declaration, used to reject edits without a version bump.
    pub descriptor: Vec<u8>,
}

impl SchemaMetadata {
    /// Reads metadata without opening a typed store or modifying the file.
    pub fn inspect(path: impl AsRef<Path>) -> Result<Self> {
        Self::read(&ColumnFile::open(path)?)
    }

    pub(crate) fn read(file: &ColumnFile) -> Result<Self> {
        let bytes = file.column::<u8>(super::SCHEMA)?;
        let bytes = bytes
            .iter()
            .map(|b| b.copied())
            .collect::<Result<Vec<_>>>()?;
        Self::decode(&bytes)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.format_version != 1 {
            return Err(Error::unsupported(format!(
                "schema metadata format {}",
                self.format_version
            )));
        }
        if self.name.trim().is_empty() || self.version == 0 {
            return Err(Error::invalid(
                "schema requires a nonempty name and a positive version",
            ));
        }
        Ok(())
    }

    pub(crate) fn check(&self, actual: Self) -> Result<()> {
        self.validate()?;
        if *self != actual {
            return Err(Error::SchemaMismatch {
                expected: Box::new(self.clone()),
                actual: Box::new(actual),
            });
        }
        Ok(())
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let name_len =
            u32::try_from(self.name.len()).map_err(|_| Error::invalid("schema name too long"))?;
        let descriptor_len = u32::try_from(self.descriptor.len())
            .map_err(|_| Error::invalid("schema descriptor too long"))?;
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&self.format_version.to_le_bytes());
        bytes.extend_from_slice(&self.version.to_le_bytes());
        bytes.extend_from_slice(&name_len.to_le_bytes());
        bytes.extend_from_slice(&descriptor_len.to_le_bytes());
        bytes.extend_from_slice(self.name.as_bytes());
        bytes.extend_from_slice(&self.descriptor);
        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.get(..8) != Some(MAGIC.as_slice()) {
            return Err(Error::unsupported("unrecognized schema metadata format"));
        }
        let bad = || Error::corrupt("schema", "truncated or malformed metadata");
        let header = bytes.get(..28).ok_or_else(bad)?;
        let format_version = u32::from_le_bytes(header[8..12].try_into().unwrap());
        if format_version != 1 {
            return Err(Error::unsupported(format!(
                "schema metadata format {format_version}"
            )));
        }
        let version = u64::from_le_bytes(header[12..20].try_into().unwrap());
        let name_len = u32::from_le_bytes(header[20..24].try_into().unwrap()) as usize;
        let descriptor_len = u32::from_le_bytes(header[24..28].try_into().unwrap()) as usize;
        let name_end = 28usize.checked_add(name_len).ok_or_else(bad)?;
        let end = name_end.checked_add(descriptor_len).ok_or_else(bad)?;
        if end != bytes.len() {
            return Err(bad());
        }
        let name = std::str::from_utf8(bytes.get(28..name_end).ok_or_else(bad)?)
            .map_err(|_| bad())?
            .to_owned();
        let metadata = Self {
            format_version,
            name,
            version,
            descriptor: bytes[name_end..].to_vec(),
        };
        metadata
            .validate()
            .map_err(|err| Error::corrupt("schema", err.to_string()))?;
        Ok(metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_is_length_delimited_and_rejects_damage() {
        let metadata = SchemaMetadata {
            format_version: 1,
            name: "library".into(),
            version: 2,
            descriptor: b"row ancestry 1".to_vec(),
        };
        let bytes = metadata.encode().unwrap();
        assert_eq!(SchemaMetadata::decode(&bytes).unwrap(), metadata);
        for end in 0..bytes.len() {
            assert!(SchemaMetadata::decode(&bytes[..end]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(SchemaMetadata::decode(&trailing).unwrap_err().is_corrupt());
        let mut newer = bytes;
        newer[8] = 2;
        assert!(SchemaMetadata::decode(&newer).unwrap_err().is_unsupported());
    }
}
