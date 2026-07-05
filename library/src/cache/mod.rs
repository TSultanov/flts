use std::path::Path;

use isolang::Language;

use crate::book::translation_import::ParagraphTranslation;

pub mod disk;
pub mod weak_lru;
pub use disk::DiskCache;
pub use weak_lru::WeakLruCache;

const MIB: u64 = 1024 * 1024;

#[cfg(any(target_os = "ios", target_os = "android"))]
const TRANSLATIONS_CACHE_STORAGE_CAPACITY: u64 = 128 * MIB;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
const TRANSLATIONS_CACHE_STORAGE_CAPACITY: u64 = 1024 * MIB;

/// Disk capacity for the Gemini prompt-cache name index. Each entry is on
/// the order of 150 bytes (name + fingerprint + timestamp + key string,
/// after zstd), so 4 MiB comfortably holds tens of thousands of entries.
pub const GEMINI_PROMPT_CACHE_CAPACITY: u64 = 4 * MIB;

pub struct TranslationsCache {
    cache: DiskCache<ParagraphTranslation>,
}

impl TranslationsCache {
    pub async fn create(cache_dir: &Path) -> anyhow::Result<Self> {
        let dir = cache_dir.join("translations");
        let cache = DiskCache::open(&dir, TRANSLATIONS_CACHE_STORAGE_CAPACITY).await?;
        Ok(Self { cache })
    }

    /// Bump whenever the translation prompt or output schema changes (the
    /// `translator` prompt text, the part-of-speech vocabulary, the grammar
    /// schema, etc.). It is part of the cache key, so bumping it makes every
    /// previously-cached translation a miss and forces a re-translation with
    /// the new prompt instead of serving the stale old result forever.
    const PROMPT_SCHEMA_VERSION: &'static str = "v1";

    fn make_key(source_language: &Language, target_language: &Language, paragraph: &str) -> String {
        format!(
            "{}\n{}\n{}\n{}",
            Self::PROMPT_SCHEMA_VERSION,
            source_language.to_639_3(),
            target_language.to_639_3(),
            paragraph
        )
    }

    pub fn set(
        &self,
        source_language: &Language,
        target_language: &Language,
        paragraph: &str,
        data: &ParagraphTranslation,
    ) {
        self.cache.insert(
            Self::make_key(source_language, target_language, paragraph),
            data.clone(),
        );
    }

    pub async fn close(&self) {
        self.cache.close().await;
    }

    pub async fn get(
        &self,
        source_language: &Language,
        target_language: &Language,
        paragraph: &str,
    ) -> anyhow::Result<Option<ParagraphTranslation>> {
        self.cache
            .get(&Self::make_key(source_language, target_language, paragraph))
            .await
    }
}
