use std::{hash::Hasher, io::{self, Cursor}};

use uuid::Uuid;

use crate::book::serialization::{read_exact_array, read_len_prefixed_string, read_len_prefixed_vec, read_u64, Magic, Version};

pub struct DictionaryMetadata {
    pub id: Uuid,
    pub source_language: String,
    pub target_language: String,
}

impl DictionaryMetadata {
    pub fn read_metadata<TReader: io::Read>(input_stream: &mut TReader) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        // Magic
        let magic = read_exact_array::<4>(input_stream)?;
        if &magic != Magic::Dictionary.as_bytes() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid magic").into());
        }
        Version::read_version(input_stream)?; // ensure supported

        // hash
        let metadata_hash = read_u64(input_stream)?;

        // Read metadata
        let metadata_buf = read_len_prefixed_vec(input_stream)?;

        let mut hasher = fnv::FnvHasher::default();
        hasher.write(&metadata_buf);
        if hasher.finish() != metadata_hash {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid metadata hash",
            ).into());
        }

        let mut cursor = Cursor::new(metadata_buf);

        let id = Uuid::from_bytes(read_exact_array::<16>(&mut cursor)?);
        let source_language = read_len_prefixed_string(&mut cursor)?;
        let target_language = read_len_prefixed_string(&mut cursor)?;

        Ok(Self {
            id,
            source_language,
            target_language
        })
    }
}

#[cfg(test)]
mod dictionary_metadata_tests {
    use std::io::Cursor;

    use crate::dictionary::{dictionary_metadata::DictionaryMetadata, Dictionary};
    use crate::book::serialization::Serializable;

    #[test]
    fn dictionary_metadata_roundtrip() {
        // Prepare a dictionary and serialize it
        let mut d = Dictionary::create("en".into(), "ru".into());
        d.add_translation("Hello", "Привет");
        d.add_translation("world", "мир");

        let mut buf: Vec<u8> = vec![];
        d.serialize(&mut buf).unwrap();

        // Read only the metadata from the serialized bytes
        let mut cur = Cursor::new(buf);
        let md = DictionaryMetadata::read_metadata(&mut cur).unwrap();

        assert_eq!(md.source_language, "en");
        assert_eq!(md.target_language, "ru");
    }

    #[test]
    fn dictionary_metadata_corruption_detection() {
        let d = Dictionary::create("en".into(), "ru".into());

        let mut buf: Vec<u8> = vec![];
        // Serialize minimal dictionary; metadata will be at the beginning
        d.serialize(&mut buf).unwrap();

        // Corrupt a byte within the header/metadata region to break the metadata hash
        // Index 10 is within the u64 metadata hash (after 4-byte magic and 1-byte version),
        // which guarantees a mismatch when the hash is verified.
        if buf.len() > 10 { buf[10] ^= 0xFF; }

        let mut cur = Cursor::new(buf);
        let r = DictionaryMetadata::read_metadata(&mut cur);
        assert!(r.is_err());
    }
}
