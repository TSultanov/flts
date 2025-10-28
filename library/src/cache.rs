use std::path::Path;

use foyer::{
    BlockEngineBuilder, DeviceBuilder, FsDeviceBuilder, HybridCache, HybridCacheBuilder,
    HybridCachePolicy,
};
use isolang::Language;

use crate::book::translation_import::ParagraphTranslation;

pub struct TranslationsCache {
    cache: HybridCache<String, ParagraphTranslation>,
}

impl TranslationsCache {
    pub async fn create(cache_dir: &Path) -> anyhow::Result<Self> {
        let device = FsDeviceBuilder::new(cache_dir)
            .with_capacity(1024 * 1024 * 1024)
            .build()?;
        let cache = HybridCacheBuilder::new()
            .with_policy(HybridCachePolicy::WriteOnInsertion)
            .memory(256 * 1024 * 1024)
            .storage()
            .with_engine_config(BlockEngineBuilder::new(device))
            .build()
            .await?;
        Ok(Self { cache })
    }

    pub fn set(
        &self,
        source_language: &Language,
        target_language: &Language,
        paragraph: &str,
        data: &ParagraphTranslation,
    ) {
        self.cache.insert(
            format!(
                "{}\n{}\n{}",
                source_language.to_639_3(),
                target_language.to_639_3(),
                paragraph.to_owned()
            ),
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
            .get(&format!(
                "{}\n{}\n{}",
                source_language.to_639_3(),
                target_language.to_639_3(),
                paragraph.to_owned()
            ))
            .await?
            .map(|r| r.value().clone()))
    }
}
