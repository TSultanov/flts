//! Per-chapter source-language summary sidecar for a book.
//!
//! Stored as `chapter_summaries.dat` next to `book.dat`. Generated chapters
//! provide context to per-paragraph translation requests (see
//! `library/src/translator/gemini.rs`). Merging is "fullest wins" — see
//! [`ChapterSummaries::merge`].

use std::{
    io::{self, BufWriter, Cursor, Read, Seek, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use crate::{
    book::{
        serialization::{
            ChecksumedWriter, Magic, Serializable, Version, create_random_string, read_exact_array,
            read_u8, read_u64, read_var_u64, validate_hash, write_u64, write_u8, write_var_u64,
        },
        soa_helpers::VecSlice,
    },
    translator::TranslationModel,
};

/// One row of the sidecar. `generated == false` means the LLM call hasn't
/// successfully completed for this chapter yet; `model`/`timestamp` are
/// meaningless in that case and `text` is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChapterSummary {
    pub generated: bool,
    pub model: TranslationModel,
    pub timestamp: u64,
    pub text: String,
}

impl ChapterSummary {
    fn pending() -> Self {
        Self {
            generated: false,
            model: TranslationModel::Unknown,
            timestamp: 0,
            text: String::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChapterSummaries {
    pub book_id: Uuid,
    pub entries: Vec<ChapterSummary>,
    /// `mtime` of the on-disk file we last read. Used by the save retry
    /// loop to detect concurrent writers (mirrors
    /// `LibraryTranslation::last_modified`).
    pub last_modified: Option<SystemTime>,
}

pub fn chapter_summaries_path(book_path: &Path) -> PathBuf {
    book_path.join("chapter_summaries.dat")
}

impl ChapterSummaries {
    /// All-pending initial state. Used at first enqueue when no sidecar exists.
    pub fn empty_for(book_id: Uuid, chapter_count: usize) -> Self {
        Self {
            book_id,
            entries: (0..chapter_count).map(|_| ChapterSummary::pending()).collect(),
            last_modified: None,
        }
    }

    /// First index whose summary still needs generation, or `None` when
    /// every chapter is already generated.
    pub fn next_pending(&self) -> Option<usize> {
        self.entries.iter().position(|e| !e.generated)
    }

    /// Highest `k` such that every chapter in `0..=k` is generated. `None`
    /// if chapter 0 is still pending. Used to populate the per-book
    /// `watch::Sender` in the summary generation queue.
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

    /// Per-chapter "fullest wins" union. Generated entries replace pending
    /// ones; if both sides have a generated entry, the newer `timestamp`
    /// wins (semantically equivalent on an immutable book, but newer is a
    /// deterministic tiebreaker).
    ///
    /// Errors if the chapter counts disagree — the book on disk has
    /// changed shape and we should refuse to silently corrupt.
    pub fn merge(&self, other: &Self) -> anyhow::Result<Self> {
        if self.book_id != other.book_id {
            anyhow::bail!(
                "chapter summaries book_id mismatch: {} vs {}",
                self.book_id,
                other.book_id
            );
        }
        if self.entries.len() != other.entries.len() {
            anyhow::bail!(
                "chapter summaries length mismatch: {} vs {}",
                self.entries.len(),
                other.entries.len()
            );
        }

        let entries = self
            .entries
            .iter()
            .zip(other.entries.iter())
            .map(|(a, b)| match (a.generated, b.generated) {
                (true, true) => {
                    if a.timestamp >= b.timestamp {
                        a.clone()
                    } else {
                        b.clone()
                    }
                }
                (true, false) => a.clone(),
                (false, true) => b.clone(),
                (false, false) => a.clone(),
            })
            .collect();

        Ok(Self {
            book_id: self.book_id,
            entries,
            last_modified: match (self.last_modified, other.last_modified) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            },
        })
    }

    /// Read one sidecar file. Doesn't deal with conflict files — see
    /// [`load_from_metadata`].
    pub async fn load(main_path: &Path) -> anyhow::Result<Self> {
        let last_modified = tokio::fs::metadata(main_path).await?.modified().ok();
        let mut buffer = Vec::new();
        tokio::fs::File::open(main_path)
            .await?
            .read_to_end(&mut buffer)
            .await?;
        let mut cursor = Cursor::new(buffer);
        let mut s = Self::deserialize(&mut cursor)?;
        s.last_modified = last_modified;
        Ok(s)
    }

    /// Load the main sidecar plus any sibling conflict files
    /// (`chapter_summaries~*.dat`), merge them, write the merged result
    /// back to `main_path`, and delete the conflict files. Mirrors
    /// `LibraryTranslation::load_from_metadata`.
    pub async fn load_from_metadata(
        main_path: &Path,
        conflicts: &[PathBuf],
    ) -> anyhow::Result<Self> {
        let mut merged = Self::load(main_path).await?;
        if conflicts.is_empty() {
            return Ok(merged);
        }
        for conflict in conflicts {
            let other = Self::load(conflict).await?;
            merged = merged.merge(&other)?;
        }
        // Write the merged result back as the new main, then delete the
        // conflict files (only after the main write succeeds).
        let mut buf = Vec::new();
        merged.serialize(&mut buf)?;
        tokio::fs::write(main_path, &buf).await?;
        merged.last_modified = tokio::fs::metadata(main_path).await?.modified().ok();
        for conflict in conflicts {
            if let Err(err) = tokio::fs::remove_file(conflict).await {
                log::warn!(
                    "failed to delete merged chapter-summaries conflict file {:?}: {err}",
                    conflict
                );
            }
        }
        Ok(merged)
    }

    /// Atomic write with a pre/post-modified-time check + merge-on-newer
    /// retry loop, mirroring `LibraryTranslation::save` body. Writes via
    /// `chapter_summaries~<random>.dat` temp file.
    pub async fn save(&mut self, main_path: &Path) -> anyhow::Result<()> {
        let dir = main_path.parent().ok_or_else(|| {
            anyhow::anyhow!("chapter summaries path has no parent: {:?}", main_path)
        })?;
        let file_name = main_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("chapter_summaries.dat");
        loop {
            let pre_modified = if tokio::fs::try_exists(main_path).await? {
                tokio::fs::metadata(main_path).await?.modified().ok()
            } else {
                None
            };

            // If disk is newer than what we last saw, merge it into self
            // before writing.
            if let Some(last_modified) = self.last_modified
                && tokio::fs::try_exists(main_path).await?
            {
                let on_disk_modified = tokio::fs::metadata(main_path).await?.modified().ok();
                if let Some(on_disk_modified) = on_disk_modified
                    && on_disk_modified > last_modified
                {
                    let on_disk = Self::load(main_path).await?;
                    *self = self.merge(&on_disk)?;
                }
            } else if tokio::fs::try_exists(main_path).await? {
                let on_disk = Self::load(main_path).await?;
                *self = self.merge(&on_disk)?;
            }

            // Write to temp.
            let temp_path = dir.join(format!("{file_name}~{}", create_random_string(8)));
            let mut buf = Vec::new();
            self.serialize(&mut buf)?;
            tokio::fs::File::create(&temp_path)
                .await?
                .write_all(&buf)
                .await?;

            // Re-check the main file's mtime; if it hasn't moved since
            // pre_modified, our write is the canonical one.
            let post_modified = if tokio::fs::try_exists(main_path).await? {
                tokio::fs::metadata(main_path).await?.modified().ok()
            } else {
                None
            };
            if post_modified == pre_modified || pre_modified.is_none() {
                if tokio::fs::try_exists(main_path).await? {
                    tokio::fs::remove_file(main_path).await?;
                }
                tokio::fs::rename(&temp_path, main_path).await?;
                self.last_modified = tokio::fs::metadata(main_path).await?.modified().ok();
                return Ok(());
            }
            // Someone else wrote between our pre-check and now. Loop and
            // re-merge. The temp file we created becomes a conflict file
            // that will be picked up on next load if nobody removes it
            // first; remove it preemptively.
            let _ = tokio::fs::remove_file(&temp_path).await;
        }
    }
}

impl Serializable for ChapterSummaries {
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

        // Build the concatenated strings blob + per-entry slices.
        let mut strings: Vec<u8> = Vec::new();
        let mut slices: Vec<VecSlice<u8>> = Vec::with_capacity(self.entries.len());
        for e in &self.entries {
            let start = strings.len();
            strings.extend_from_slice(e.text.as_bytes());
            slices.push(VecSlice::new(start, e.text.len()));
        }

        for (e, slice) in self.entries.iter().zip(slices.iter()) {
            write_u8(&mut w, if e.generated { 1 } else { 0 })?;
            write_var_u64(&mut w, usize::from(e.model) as u64)?;
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
            let model = TranslationModel::from(read_var_u64(input_stream)? as usize);
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
            let end = slice.start.checked_add(slice.len).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "slice end overflow")
            })?;
            if end > strings.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "chapter summary slice out of bounds",
                ));
            }
            entry.text = String::from_utf8(strings[slice.start..end].to_vec())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "summary not utf-8"))?;
        }

        // Skip the trailing 8-byte hash (validate_hash already saw it).
        let _ = read_u64(input_stream)?;

        Ok(Self {
            book_id,
            entries,
            last_modified: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made(book_id: Uuid) -> ChapterSummaries {
        let mut s = ChapterSummaries::empty_for(book_id, 4);
        s.entries[0] = ChapterSummary {
            generated: true,
            model: TranslationModel::Gemini25Flash,
            timestamp: 100,
            text: "Chapter 0 summary: characters Alice and Bob.".into(),
        };
        s.entries[2] = ChapterSummary {
            generated: true,
            model: TranslationModel::Gemini25Flash,
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

        // chapter 2 still pending; chapter 3 generated should not advance ready_through
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

    #[test]
    fn merge_takes_generated_over_pending() {
        let book_id = Uuid::new_v4();
        let mut a = ChapterSummaries::empty_for(book_id, 3);
        a.entries[0] = ChapterSummary {
            generated: true,
            model: TranslationModel::Gemini25Flash,
            timestamp: 100,
            text: "a0".into(),
        };
        let mut b = ChapterSummaries::empty_for(book_id, 3);
        b.entries[1] = ChapterSummary {
            generated: true,
            model: TranslationModel::Gemini25Flash,
            timestamp: 200,
            text: "b1".into(),
        };

        let merged = a.merge(&b).unwrap();
        assert!(merged.entries[0].generated);
        assert_eq!(merged.entries[0].text, "a0");
        assert!(merged.entries[1].generated);
        assert_eq!(merged.entries[1].text, "b1");
        assert!(!merged.entries[2].generated);
    }

    #[test]
    fn merge_newer_timestamp_wins_when_both_generated() {
        let book_id = Uuid::new_v4();
        let mut older = ChapterSummaries::empty_for(book_id, 2);
        older.entries[0] = ChapterSummary {
            generated: true,
            model: TranslationModel::Gemini25Flash,
            timestamp: 100,
            text: "older".into(),
        };
        let mut newer = ChapterSummaries::empty_for(book_id, 2);
        newer.entries[0] = ChapterSummary {
            generated: true,
            model: TranslationModel::Gemini25Flash,
            timestamp: 200,
            text: "newer".into(),
        };

        let merged = older.merge(&newer).unwrap();
        assert_eq!(merged.entries[0].text, "newer");
        // Symmetric: order shouldn't matter for tiebreak.
        let merged2 = newer.merge(&older).unwrap();
        assert_eq!(merged2.entries[0].text, "newer");
    }

    #[test]
    fn merge_rejects_book_id_mismatch() {
        let a = ChapterSummaries::empty_for(Uuid::new_v4(), 2);
        let b = ChapterSummaries::empty_for(Uuid::new_v4(), 2);
        assert!(a.merge(&b).is_err());
    }

    #[test]
    fn merge_rejects_chapter_count_mismatch() {
        let book_id = Uuid::new_v4();
        let a = ChapterSummaries::empty_for(book_id, 2);
        let b = ChapterSummaries::empty_for(book_id, 3);
        assert!(a.merge(&b).is_err());
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = chapter_summaries_path(dir.path());
        let mut original = made(Uuid::new_v4());
        original.save(&path).await.unwrap();

        let loaded = ChapterSummaries::load(&path).await.unwrap();
        assert_eq!(loaded.book_id, original.book_id);
        for (a, b) in loaded.entries.iter().zip(original.entries.iter()) {
            assert_eq!(a.generated, b.generated);
            assert_eq!(a.text, b.text);
        }
    }

    #[tokio::test]
    async fn save_then_save_again_with_progress_persists_both() {
        let dir = tempfile::tempdir().unwrap();
        let path = chapter_summaries_path(dir.path());
        let book_id = Uuid::new_v4();
        let mut s = ChapterSummaries::empty_for(book_id, 3);
        s.save(&path).await.unwrap();

        s.entries[0] = ChapterSummary {
            generated: true,
            model: TranslationModel::Gemini25Flash,
            timestamp: 1,
            text: "ch0".into(),
        };
        s.save(&path).await.unwrap();

        let loaded = ChapterSummaries::load(&path).await.unwrap();
        assert!(loaded.entries[0].generated);
        assert_eq!(loaded.entries[0].text, "ch0");
        assert!(!loaded.entries[1].generated);
    }

    #[tokio::test]
    async fn load_from_metadata_merges_and_cleans_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let main = chapter_summaries_path(dir.path());
        let book_id = Uuid::new_v4();

        let mut a = ChapterSummaries::empty_for(book_id, 3);
        a.entries[0] = ChapterSummary {
            generated: true,
            model: TranslationModel::Gemini25Flash,
            timestamp: 10,
            text: "from_main".into(),
        };
        a.save(&main).await.unwrap();

        let mut b = ChapterSummaries::empty_for(book_id, 3);
        b.entries[1] = ChapterSummary {
            generated: true,
            model: TranslationModel::Gemini25Flash,
            timestamp: 20,
            text: "from_conflict".into(),
        };
        let conflict_path = dir.path().join("chapter_summaries~abcd1234.dat");
        let mut buf = Vec::new();
        b.serialize(&mut buf).unwrap();
        tokio::fs::write(&conflict_path, &buf).await.unwrap();

        let merged = ChapterSummaries::load_from_metadata(&main, &[conflict_path.clone()])
            .await
            .unwrap();
        assert_eq!(merged.entries[0].text, "from_main");
        assert_eq!(merged.entries[1].text, "from_conflict");
        assert!(!merged.entries[2].generated);
        assert!(!tokio::fs::try_exists(&conflict_path).await.unwrap());
    }
}
