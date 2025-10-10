use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use crate::book::serialization::{
    ChecksumedWriter, Magic, Serializable, Version, read_len_prefixed_string, read_u64,
    read_var_u64, validate_hash, write_len_prefixed_bytes, write_len_prefixed_str, write_u64,
    write_var_u64,
};
use std::io;
use std::time::Instant;

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
            self.translations
                .insert(original_lowercase.clone(), BTreeSet::new());
        }

        self.translations
            .get_mut(&original_lowercase)
            .unwrap()
            .insert(translation.to_lowercase());
    }

    pub fn merge(self, other: Self) -> Self {
        Self::try_merge(self, other)
            .expect("merge should not fail; use try_merge for error handling")
    }

    pub fn try_merge(mut self, other: Self) -> Result<Self, DictionaryMergeError> {
        if self.source_language != other.source_language
            || self.target_language != other.target_language
        {
            return Err(DictionaryMergeError::LanguageMismatch);
        }

        // Efficiently merge without cloning sets: move entries from `other` into `self`.
        for (orig, set) in other.translations.into_iter() {
            match self.translations.entry(orig) {
                Entry::Vacant(v) => {
                    v.insert(set);
                }
                Entry::Occupied(mut o) => {
                    o.get_mut().extend(set);
                }
            }
        }

        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictionaryMergeError {
    LanguageMismatch,
}

impl std::fmt::Display for DictionaryMergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DictionaryMergeError::LanguageMismatch => {
                write!(f, "Cannot merge dictionaries with different languages")
            }
        }
    }
}

impl std::error::Error for DictionaryMergeError {}

impl Serializable for Dictionary {
    fn serialize<TWriter: std::io::Write>(
        &self,
        output_stream: &mut TWriter,
    ) -> std::io::Result<()> {
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

        let total_start = Instant::now();
        let mut hashing_stream = ChecksumedWriter::create(output_stream);

        // Magic + version
        let t_magic = Instant::now();
        Magic::Dictionary.write(&mut hashing_stream)?;
        Version::V1.write_version(&mut hashing_stream)?;
        let d_magic = t_magic.elapsed();

        // Build metadata buf with its own hasher
        let t_meta_build = Instant::now();
        let mut metadata_buf = Vec::new();
        let mut metadata_hasher = ChecksumedWriter::create(&mut metadata_buf);
        write_len_prefixed_str(&mut metadata_hasher, &self.source_language)?;
        write_len_prefixed_str(&mut metadata_hasher, &self.target_language)?;
        write_var_u64(&mut metadata_hasher, self.translations.len() as u64)?;
        let metadata_hash = metadata_hasher.current_hash();
        let d_meta_build = t_meta_build.elapsed();

        // Write metadata
        let t_meta_write = Instant::now();
        write_u64(&mut hashing_stream, metadata_hash)?;
        write_len_prefixed_bytes(&mut hashing_stream, &metadata_buf)?;
        let d_meta_write = t_meta_write.elapsed();

        // Compute total pairs (optional, informational). We'll still write per-original blocks.
        let t_pairs = Instant::now();
        let mut total_pairs = 0u64;
        for (_orig, tr_set) in &self.translations {
            total_pairs += tr_set.len() as u64;
        }
        write_var_u64(&mut hashing_stream, total_pairs)?;
        let d_pairs = t_pairs.elapsed();

        // Write entries: we want deterministic ordering -> BTreeMap + BTreeSet already provide it
        let t_entries = Instant::now();
        write_var_u64(&mut hashing_stream, self.translations.len() as u64)?;
        for (original, translations) in &self.translations {
            write_len_prefixed_str(&mut hashing_stream, original)?;
            write_var_u64(&mut hashing_stream, translations.len() as u64)?;
            for t in translations {
                write_len_prefixed_str(&mut hashing_stream, t)?;
            }
        }
        let d_entries = t_entries.elapsed();

        // Finalize
        let t_finalize = Instant::now();
        let hash = hashing_stream.current_hash();
        write_u64(output_stream, hash)?;
        output_stream.flush()?;
        let d_finalize = t_finalize.elapsed();

        let total = total_start.elapsed();
        println!(
            "Serialization timings (Dictionary):\n  - magic+version: {:?}\n  - metadata build: {:?}\n  - metadata write: {:?}\n  - count pairs ({}): {:?}\n  - entries ({} originals): {:?}\n  - finalize hash+flush: {:?}\n  - TOTAL: {:?}",
            d_magic,
            d_meta_build,
            d_meta_write,
            total_pairs,
            d_pairs,
            self.translations.len(),
            d_entries,
            d_finalize,
            total
        );
        Ok(())
    }

    fn deserialize<TReader: std::io::Seek + std::io::Read>(
        input_stream: &mut TReader,
    ) -> std::io::Result<Self>
    where
        Self: Sized,
    {
        let total_start = Instant::now();

        // Validate full-file hash
        let t_hash = Instant::now();
        let hash_valid = validate_hash(input_stream)?;
        if !hash_valid {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid hash"));
        }
        let d_hash = t_hash.elapsed();

        // Magic + version
        let t_magic = Instant::now();
        let mut magic = [0u8; 4];
        input_stream.read_exact(&mut magic)?;
        if &magic != Magic::Dictionary.as_bytes() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid magic"));
        }
        Version::read_version(input_stream)?;
        let d_magic = t_magic.elapsed();

        // Skip metadata hash and length; then read metadata payload
        let t_meta = Instant::now();
        _ = read_u64(input_stream)?; // metadata hash (unused on full read)
        _ = read_var_u64(input_stream)?; // metadata len

        let source_language = read_len_prefixed_string(input_stream)?;
        let target_language = read_len_prefixed_string(input_stream)?;
        // unique original count (informational)
        _ = read_var_u64(input_stream)?;
        let d_meta = t_meta.elapsed();

        // Total pairs (informational)
        let t_pairs = Instant::now();
        _ = read_var_u64(input_stream)?;
        let d_pairs = t_pairs.elapsed();

        // Entries
        let t_entries = Instant::now();
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
        let d_entries = t_entries.elapsed();

        let total = total_start.elapsed();
        println!(
            "Deserialization timings (Dictionary):\n  - hash validate: {:?}\n  - magic+version: {:?}\n  - metadata (incl. read): {:?}\n  - pairs read: {:?}\n  - entries ({} originals): {:?}\n  - TOTAL: {:?}",
            d_hash, d_magic, d_meta, d_pairs, originals_len, d_entries, total
        );

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

    #[test]
    fn dictionary_merge_success() {
        let mut d1 = Dictionary::create("en".into(), "ru".into());
        d1.add_translation("Hello", "Привет");
        d1.add_translation("world", "мир");

        let mut d2 = Dictionary::create("en".into(), "ru".into());
        d2.add_translation("hello", "Здравствуй");
        d2.add_translation("new", "новый");

        let merged = d1.try_merge(d2).unwrap();

        assert_eq!(merged.source_language, "en");
        assert_eq!(merged.target_language, "ru");
        assert_eq!(merged.translations.len(), 3);

        let hello = merged.translations.get("hello").unwrap();
        assert!(hello.contains("привет"));
        assert!(hello.contains("здравствуй"));

        let world = merged.translations.get("world").unwrap();
        assert!(world.contains("мир"));

        let neww = merged.translations.get("new").unwrap();
        assert!(neww.contains("новый"));
    }

    #[test]
    fn dictionary_merge_language_mismatch_returns_err() {
        let mut d1 = Dictionary::create("en".into(), "ru".into());
        d1.add_translation("Hello", "Привет");

        let mut d2 = Dictionary::create("en".into(), "de".into());
        d2.add_translation("Hello", "Hallo");

        let err = d1.try_merge(d2);
        assert!(matches!(err, Err(DictionaryMergeError::LanguageMismatch)));
    }
}
