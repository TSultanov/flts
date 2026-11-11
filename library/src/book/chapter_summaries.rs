//! Per-chapter source-language summaries of a book; generated chapters give
//! per-paragraph translation requests their context.

use std::io::{self, Read, Seek};
#[cfg(test)]
use std::io::{BufWriter, Write};

use uuid::Uuid;

#[cfg(test)]
use crate::book::serialization::{ChecksumedWriter, write_u8, write_u64, write_var_u64};
use crate::book::{
    serialization::{
        Magic, Serializable, Version, read_exact_array, read_u8, read_u64, read_var_u64,
        validate_hash,
    },
    soa_helpers::VecSlice,
};

/// One row. When `generated` is false the LLM call hasn't completed, so
/// `model`/`timestamp` are meaningless and `text` is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChapterSummary {
    pub generated: bool,
    pub model: String,
    pub timestamp: u64,
    pub text: String,
}

impl ChapterSummary {
    fn pending() -> Self {
        Self {
            generated: false,
            model: String::new(),
            timestamp: 0,
            text: String::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChapterSummaries {
    pub book_id: Uuid,
    pub entries: Vec<ChapterSummary>,
}

impl ChapterSummaries {
    /// All-pending initial state.
    pub fn empty_for(book_id: Uuid, chapter_count: usize) -> Self {
        Self {
            book_id,
            entries: (0..chapter_count)
                .map(|_| ChapterSummary::pending())
                .collect(),
        }
    }

    /// First index still needing generation.
    pub fn next_pending(&self) -> Option<usize> {
        self.entries.iter().position(|e| !e.generated)
    }

    /// Highest `k` with every chapter in `0..=k` generated.
    pub fn ready_through(&self) -> Option<usize> {
        let mut last = None;
        for (i, e) in self.entries.iter().enumerate() {
            if !e.generated {
                break;
            }
            last = Some(i);
        }
        last
    }
}

impl Serializable for ChapterSummaries {
    #[cfg(test)]
    fn serialize<TWriter: Write>(&self, output_stream: &mut TWriter) -> io::Result<()> {
        // Binary format CS01 v1 (little endian):
        // magic[4] = CS01
        // u8 version = 1
        // u8[16] book_id
        // varuint chapter_count
        // for each chapter:
        //   u8 generated
        //   varuint model_id
        //   varuint timestamp
        //   varuint text_slice.start
        //   varuint text_slice.len
        // varuint strings_compressed_len, [u8]* strings (zstd-compressed,
        //                                                concatenated chapter texts)
        // u64 fnv1 hash of everything above
        let mut hashing_stream_unbuffered = ChecksumedWriter::create(output_stream);
        let mut w = BufWriter::new(hashing_stream_unbuffered);

        Magic::ChapterSummaries.write(&mut w)?;
        Version::V1.write_version(&mut w)?;
        w.write_all(self.book_id.as_bytes())?;
        write_var_u64(&mut w, self.entries.len() as u64)?;

        let mut strings: Vec<u8> = Vec::new();
        let mut slices: Vec<VecSlice<u8>> = Vec::with_capacity(self.entries.len());
        for e in &self.entries {
            let start = strings.len();
            strings.extend_from_slice(e.text.as_bytes());
            slices.push(VecSlice::new(start, e.text.len()));
        }

        for (e, slice) in self.entries.iter().zip(slices.iter()) {
            write_u8(&mut w, if e.generated { 1 } else { 0 })?;
            write_var_u64(
                &mut w,
                crate::translator::catalog::legacy_id_from_api(&e.model),
            )?;
            write_var_u64(&mut w, e.timestamp)?;
            write_var_u64(&mut w, slice.start as u64)?;
            write_var_u64(&mut w, slice.len as u64)?;
        }

        let encoded = zstd::stream::encode_all(strings.as_slice(), -7)?;
        write_var_u64(&mut w, encoded.len() as u64)?;
        w.write_all(&encoded)?;

        hashing_stream_unbuffered = w.into_inner()?;
        let hash = hashing_stream_unbuffered.current_hash();
        write_u64(output_stream, hash)?;
        output_stream.flush()
    }

    fn deserialize<TReader: Seek + Read>(input_stream: &mut TReader) -> io::Result<Self> {
        if !validate_hash(input_stream)? {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid hash"));
        }

        Magic::read(Magic::ChapterSummaries, input_stream)?;
        Version::read_version(input_stream)?;

        let book_id_bytes = read_exact_array::<16>(input_stream)?;
        let book_id = Uuid::from_bytes(book_id_bytes);

        let chapter_count = read_var_u64(input_stream)? as usize;

        let mut entries = Vec::with_capacity(chapter_count);
        let mut slices = Vec::with_capacity(chapter_count);
        for _ in 0..chapter_count {
            let generated = read_u8(input_stream)? == 1;
            let n = read_var_u64(input_stream)?;
            let model = crate::translator::catalog::api_id_from_legacy(n);
            let timestamp = read_var_u64(input_stream)?;
            let start = read_var_u64(input_stream)? as usize;
            let len = read_var_u64(input_stream)? as usize;
            slices.push(VecSlice::<u8>::new(start, len));
            entries.push(ChapterSummary {
                generated,
                model,
                timestamp,
                text: String::new(),
            });
        }

        let strings_len = read_var_u64(input_stream)? as usize;
        let mut encoded = vec![0u8; strings_len];
        input_stream.read_exact(&mut encoded)?;
        let strings = zstd::stream::decode_all(encoded.as_slice())?;

        for (entry, slice) in entries.iter_mut().zip(slices.iter()) {
            let end = slice
                .start
                .checked_add(slice.len)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "slice end overflow"))?;
            if end > strings.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "chapter summary slice out of bounds",
                ));
            }
            entry.text = String::from_utf8(strings[slice.start..end].to_vec())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "summary not utf-8"))?;
        }

        let _ = read_u64(input_stream)?;

        Ok(Self { book_id, entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn made(book_id: Uuid) -> ChapterSummaries {
        let mut s = ChapterSummaries::empty_for(book_id, 4);
        s.entries[0] = ChapterSummary {
            generated: true,
            model: "models/gemini-2.5-flash".to_string(),
            timestamp: 100,
            text: "Chapter 0 summary: characters Alice and Bob.".into(),
        };
        s.entries[2] = ChapterSummary {
            generated: true,
            model: "models/gemini-2.5-flash".to_string(),
            timestamp: 300,
            text: "Chapter 2 summary: Bob travels.".into(),
        };
        s
    }

    #[test]
    fn next_pending_skips_generated() {
        let s = made(Uuid::new_v4());
        assert_eq!(s.next_pending(), Some(1));
    }

    #[test]
    fn ready_through_returns_contiguous_prefix() {
        let book_id = Uuid::new_v4();
        let mut s = ChapterSummaries::empty_for(book_id, 5);
        assert_eq!(s.ready_through(), None);

        s.entries[0].generated = true;
        s.entries[0].text = "a".into();
        assert_eq!(s.ready_through(), Some(0));

        s.entries[1].generated = true;
        s.entries[1].text = "b".into();
        assert_eq!(s.ready_through(), Some(1));

        s.entries[3].generated = true;
        s.entries[3].text = "d".into();
        assert_eq!(s.ready_through(), Some(1));
    }

    #[test]
    fn serde_roundtrip_preserves_state() {
        let original = made(Uuid::new_v4());
        let mut buf = Vec::new();
        original.serialize(&mut buf).unwrap();

        let mut cursor = Cursor::new(buf);
        let decoded = ChapterSummaries::deserialize(&mut cursor).unwrap();

        assert_eq!(decoded.book_id, original.book_id);
        assert_eq!(decoded.entries.len(), original.entries.len());
        for (a, b) in decoded.entries.iter().zip(original.entries.iter()) {
            assert_eq!(a.generated, b.generated);
            assert_eq!(a.text, b.text);
            if a.generated {
                assert_eq!(a.model, b.model);
                assert_eq!(a.timestamp, b.timestamp);
            }
        }
    }
}
