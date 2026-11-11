use std::path::PathBuf;
use uuid::Uuid;

use crate::book::translation_import;

pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    pub fn new(prefix: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!("{}_{}", prefix, Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub fn full_word(
    original: &str,
    lemma_src: &str,
    lemma_tgt: &str,
    part_of_speech: &str,
    translations: &[&str],
    is_punctuation: bool,
) -> translation_import::Word {
    translation_import::Word {
        original: original.into(),
        contextual_translations: translations.iter().map(|s| (*s).into()).collect(),
        note: None,
        is_punctuation,
        grammar: translation_import::Grammar {
            original_initial_form: lemma_src.into(),
            target_initial_form: lemma_tgt.into(),
            part_of_speech: part_of_speech.into(),
            plurality: None,
            person: None,
            tense: None,
            case: None,
            other: None,
        },
    }
}

pub fn one_sentence_paragraph(
    full_translation: &str,
    words: Vec<translation_import::Word>,
) -> translation_import::ParagraphTranslation {
    translation_import::ParagraphTranslation {
        timestamp: 0,
        total_tokens: None,
        sentences: vec![translation_import::Sentence {
            full_translation: full_translation.into(),
            words,
        }],
    }
}

/// Copies one device's shard directory into another's, as a file sync would.
pub fn copy_shards(from: &std::path::Path, to: &std::path::Path) {
    copy_shards_where(from, to, &|_| true);
}

/// As [`copy_shards`], for the entities `wanted` accepts by directory name.
pub fn copy_shards_where(
    from: &std::path::Path,
    to: &std::path::Path,
    wanted: &dyn Fn(&str) -> bool,
) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            if wanted(&entry.file_name().to_string_lossy()) {
                copy_shards_where(&entry.path(), &target, &|_| true);
            }
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// What another device would hold after syncing `store`'s shards: only what
/// was really committed.
pub fn peer_view(store: &crate::store::LibraryStore) -> (TempDir, crate::store::LibraryStore) {
    let dir = TempDir::new("flts_peer_view");
    let root = dir.path.join("library");
    copy_shards(store.shared_dir(), &root.join(crate::store::SHARED_DIR));
    let peer = crate::store::LibraryStore::open(&root).unwrap();
    peer.run_blocking(|db| db.refresh().map(drop)).unwrap();
    (dir, peer)
}
