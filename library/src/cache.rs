use std::path::Path;

use foyer::{
    BlockEngineBuilder, DeviceBuilder, FsDeviceBuilder, HybridCache, HybridCacheBuilder,
    HybridCachePolicy,
};
use isolang::Language;

use crate::book::translation_import::ParagraphTranslation;

const MIB: usize = 1024 * 1024;

#[cfg(any(target_os = "ios", target_os = "android"))]
const TRANSLATIONS_CACHE_MEMORY_CAPACITY: usize = 32 * MIB;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
const TRANSLATIONS_CACHE_MEMORY_CAPACITY: usize = 256 * MIB;

#[cfg(any(target_os = "ios", target_os = "android"))]
const TRANSLATIONS_CACHE_STORAGE_CAPACITY: usize = 128 * MIB;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
const TRANSLATIONS_CACHE_STORAGE_CAPACITY: usize = 1024 * MIB;

pub struct TranslationsCache {
    cache: HybridCache<String, ParagraphTranslation>,
}

impl TranslationsCache {
    pub async fn create(cache_dir: &Path) -> anyhow::Result<Self> {
        let device = FsDeviceBuilder::new(cache_dir)
            .with_capacity(TRANSLATIONS_CACHE_STORAGE_CAPACITY)
            .build()?;
        let cache = HybridCacheBuilder::new()
            .with_policy(HybridCachePolicy::WriteOnInsertion)
            .memory(TRANSLATIONS_CACHE_MEMORY_CAPACITY)
            .storage()
            .with_engine_config(BlockEngineBuilder::new(device))
            .build()
            .await?;
        Ok(Self { cache })
    }

    fn make_key(source_language: &Language, target_language: &Language, paragraph: &str) -> String {
        format!(
            "{}\n{}\n{}",
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

    pub async fn get(
        &self,
        source_language: &Language,
        target_language: &Language,
        paragraph: &str,
    ) -> anyhow::Result<Option<ParagraphTranslation>> {
        Ok(self
            .cache
            .get(&Self::make_key(source_language, target_language, paragraph))
            .await?
            .map(|r| r.value().clone()))
    }
}
