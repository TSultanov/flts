use std::collections::{BTreeMap, BTreeSet};

use crate::book::serialization::{
    ChecksumedWriter, Magic, Serializable, Version, read_len_prefixed_string,
    read_u64, read_var_u64, validate_hash, write_len_prefixed_bytes,
    write_len_prefixed_str, write_u64, write_var_u64,
};
use std::io;

pub struct Dictionary {
    pub source_language: String,
    pub target_language: String,
    translations: BTreeMap<String, BTreeSet<String>>,
}

impl Dictionary {
    pub fn create(source_language: String, target_language: String) -> Self {
        Self {
            source_language,
            target_language,
            translations: BTreeMap::new(),
        }
    }

    pub fn add_translation(&mut self, original_word: &str, translation: &str) {
        let original_lowercase = original_word.to_lowercase();
        if !self.translations.contains_key(&original_lowercase) {
            self.translations.insert(original_lowercase.clone(), BTreeSet::new());
        }

        self.translations.get_mut(&original_lowercase).unwrap().insert(translation.to_lowercase());
    }
}

impl Serializable for Dictionary {
    fn serialize<TWriter: std::io::Write>(&self, output_stream: &mut TWriter) -> std::io::Result<()> {
        // Binary format DC01 v1 (little-endian):
        // magic[4] = DC01
        // u8 version = 1
        // Metadata section
        //   u64 metadata hash
        //   metadata payload (len-prefixed):
        //     source_language (len-prefixed string)
        //     target_language (len-prefixed string)
        //     u64 unique_original_words_count
        // Data section
        //   u64 pairs_count (sum over all originals of number of translations)
        //   For each original word entry:
        //       original (len-prefixed string)
        //       u64 translations_count
        //       repeat translations_count times: translation (len-prefixed string)
        // u64 fnv1 hash of the entire file except the hash itself

        let mut hashing_stream = ChecksumedWriter::create(output_stream);

        // Magic + version
        Magic::Dictionary.write(&mut hashing_stream)?;
        Version::V1.write_version(&mut hashing_stream)?;

        // Build metadata buf with its own hasher
        let mut metadata_buf = Vec::new();
        let mut metadata_hasher = ChecksumedWriter::create(&mut metadata_buf);
        write_len_prefixed_str(&mut metadata_hasher, &self.source_language)?;
        write_len_prefixed_str(&mut metadata_hasher, &self.target_language)?;
        write_var_u64(
            &mut metadata_hasher,
            self.translations.len() as u64,
        )?;

        let metadata_hash = metadata_hasher.current_hash();
        write_u64(&mut hashing_stream, metadata_hash)?;
        write_len_prefixed_bytes(&mut hashing_stream, &metadata_buf)?;

        // Compute total pairs (optional, informational). We'll still write per-original blocks.
        let mut total_pairs = 0u64;
        for (_orig, tr_set) in &self.translations {
            total_pairs += tr_set.len() as u64;
        }
        write_var_u64(&mut hashing_stream, total_pairs)?;

        // Write entries: we want deterministic ordering -> BTreeMap + BTreeSet already provide it
        write_var_u64(&mut hashing_stream, self.translations.len() as u64)?;
        for (original, translations) in &self.translations {
            write_len_prefixed_str(&mut hashing_stream, original)?;
            write_var_u64(&mut hashing_stream, translations.len() as u64)?;
            for t in translations {
                write_len_prefixed_str(&mut hashing_stream, t)?;
            }
        }

        let hash = hashing_stream.current_hash();
        write_u64(output_stream, hash)?;
        output_stream.flush()?;
        Ok(())
    }

    fn deserialize<TReader: std::io::Seek + std::io::Read>(input_stream: &mut TReader) -> std::io::Result<Self>
    where
        Self: Sized {
        // Validate full-file hash
        let hash_valid = validate_hash(input_stream)?;
        if !hash_valid {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid hash"));
        }

        // Magic + version
        let mut magic = [0u8; 4];
        input_stream.read_exact(&mut magic)?;
        if &magic != Magic::Dictionary.as_bytes() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid magic"));
        }
        Version::read_version(input_stream)?;

        // Skip metadata hash and length; then read metadata payload
        _ = read_u64(input_stream)?; // metadata hash (unused on full read)
        _ = read_var_u64(input_stream)?; // metadata len

        let source_language = read_len_prefixed_string(input_stream)?;
        let target_language = read_len_prefixed_string(input_stream)?;
        // unique original count (informational)
        _ = read_var_u64(input_stream)?;

        // Total pairs (informational)
        _ = read_var_u64(input_stream)?;

        // Entries
        let originals_len = read_var_u64(input_stream)? as usize;
        let mut translations: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for _ in 0..originals_len {
            let original = read_len_prefixed_string(input_stream)?;
            let count = read_var_u64(input_stream)? as usize;
            let mut set: BTreeSet<String> = BTreeSet::new();
            for _ in 0..count {
                let tr = read_len_prefixed_string(input_stream)?;
                set.insert(tr);
            }
            translations.insert(original, set);
        }

        Ok(Dictionary {
            source_language,
            target_language,
            translations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn dictionary_add_and_roundtrip() {
        let mut d = Dictionary {
            source_language: "en".into(),
            target_language: "ru".into(),
            translations: BTreeMap::new(),
        };
        d.add_translation("Hello", "Привет");
        d.add_translation("Hello", "Здравствуй");
        d.add_translation("world", "мир");

        let mut buf: Vec<u8> = vec![];
        d.serialize(&mut buf).unwrap();

        let mut cur = Cursor::new(buf);
        let d2 = Dictionary::deserialize(&mut cur).unwrap();

        assert_eq!(d2.source_language, "en");
        assert_eq!(d2.target_language, "ru");
        assert_eq!(d2.translations.len(), 2);
        let hello = d2.translations.get("hello").unwrap();
        assert!(hello.contains("привет"));
        assert!(hello.contains("здравствуй"));
        let world = d2.translations.get("world").unwrap();
        assert!(world.contains("мир"));
    }

    #[test]
    fn dictionary_corruption_detection() {
        let mut d = Dictionary {
            source_language: "en".into(),
            target_language: "ru".into(),
            translations: BTreeMap::new(),
        };
        d.add_translation("Hello", "Привет");
        let mut buf: Vec<u8> = vec![];
        d.serialize(&mut buf).unwrap();
        // Corrupt some byte
        buf[12] ^= 0xFF;
        let mut cur = Cursor::new(buf);
        let r = Dictionary::deserialize(&mut cur);
        assert!(r.is_err());
    }
}