use std::{collections::HashMap, sync::Arc, time::Duration};

use gemini_rust::{CachedContentHandle, Gemini};
use isolang::Language;
use log::info;
use tokio::sync::{Mutex, OnceCell};

use crate::translator::TranslationModel;

/// Gemini's docs default cached content to a 1h TTL when none is set; the
/// crate's `CacheBuilder::execute` requires an explicit value, so we pick the
/// same. Long enough for an active reading session; short enough that an idle
/// instance doesn't accumulate storage cost.
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(3600);

#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub struct CacheKey {
    pub model: TranslationModel,
    pub from: Language,
    pub to: Language,
}

/// Generic per-process registry that lazily creates one value per key and
/// dedupes concurrent first-callers via `OnceCell`. Used as
/// [`GeminiCacheRegistry`] for [`CachedContentHandle`]; left generic so the
/// concurrency contract can be unit-tested without hitting the network.
pub struct Registry<T> {
    inner: Mutex<HashMap<CacheKey, Arc<OnceCell<Arc<T>>>>>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl<T> Registry<T> {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Lazily creates the value for `key` via `init`. Concurrent callers for
    /// the same key race on the outer `Mutex` only briefly (to insert the
    /// per-key `OnceCell`); the `init` future itself runs at most once per
    /// key per registry lifetime.
    pub async fn get_or_create_with<F, Fut, E>(
        &self,
        key: CacheKey,
        init: F,
    ) -> Result<Arc<T>, E>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Arc<T>, E>>,
    {
        let cell = {
            let mut guard = self.inner.lock().await;
            guard
                .entry(key)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };

        let value = cell.get_or_try_init(init).await?;
        Ok(value.clone())
    }

    /// Drops the entry for `key` so a subsequent [`get_or_create_with`] runs
    /// `init` again. Used to recover from a cache that the server has
    /// expired or deleted.
    pub async fn evict(&self, key: &CacheKey) {
        let mut guard = self.inner.lock().await;
        guard.remove(key);
    }
}

pub type GeminiCacheRegistry = Registry<CachedContentHandle>;

impl Registry<CachedContentHandle> {
    /// Returns a Gemini cache handle for `key`, creating it on first use. The
    /// `system_prompt` closure is only called when the cache actually needs
    /// to be built — repeat callers pay just one map lookup.
    pub async fn get_or_create<F>(
        &self,
        client: &Gemini,
        key: CacheKey,
        system_prompt: F,
    ) -> anyhow::Result<Arc<CachedContentHandle>>
    where
        F: FnOnce() -> String,
    {
        let key_for_init = key.clone();
        self.get_or_create_with::<_, _, anyhow::Error>(key, || async move {
            let prompt = system_prompt();
            let display = format!(
                "flts-{}-{}-{}",
                usize::from(key_for_init.model),
                key_for_init.from.to_639_3(),
                key_for_init.to.to_639_3()
            );
            info!(
                "Creating Gemini cache {display} ({} prompt chars, ttl {}s)",
                prompt.len(),
                DEFAULT_CACHE_TTL.as_secs()
            );
            let handle = client
                .create_cache()
                .with_display_name(display)?
                .with_system_instruction(prompt)
                .with_ttl(DEFAULT_CACHE_TTL)
                .execute()
                .await?;
            info!("Created Gemini cache: {}", handle.name());
            Ok(Arc::new(handle))
        })
        .await
    }
}

/// True if `err` indicates the cached content reference was rejected by the
/// server (expired, deleted, or wrong account). The caller should
/// [`Registry::evict`] the key and retry once with a fresh cache.
pub fn is_cache_missing_error(err: &anyhow::Error) -> bool {
    let Some(ce) = err.downcast_ref::<gemini_rust::ClientError>() else {
        return false;
    };
    let gemini_rust::ClientError::BadResponse { code, description } = ce else {
        return false;
    };
    if *code != 403 && *code != 404 {
        return false;
    }
    description
        .as_deref()
        .map(|d| d.to_lowercase().contains("cachedcontent"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use isolang::Language;

    use super::{CacheKey, Registry, is_cache_missing_error};
    use crate::translator::TranslationModel;

    fn key(model: TranslationModel, from: Language, to: Language) -> CacheKey {
        CacheKey { model, from, to }
    }

    #[tokio::test]
    async fn single_concurrent_create_for_same_key() {
        let registry: std::sync::Arc<Registry<u32>> = std::sync::Arc::new(Registry::default());
        let counter = std::sync::Arc::new(AtomicUsize::new(0));
        let k = key(TranslationModel::Gemini25Flash, Language::Eng, Language::Rus);

        let mut handles = Vec::new();
        for _ in 0..16 {
            let registry = registry.clone();
            let counter = counter.clone();
            let k = k.clone();
            handles.push(async move {
                registry
                    .get_or_create_with::<_, _, anyhow::Error>(k, move || async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Ok(std::sync::Arc::new(42u32))
                    })
                    .await
                    .unwrap()
            });
        }
        let results = futures_util::future::join_all(handles).await;

        assert_eq!(counter.load(Ordering::SeqCst), 1);
        for v in &results {
            assert_eq!(**v, 42);
        }
    }

    #[tokio::test]
    async fn different_keys_create_independently() {
        let registry: Registry<u32> = Registry::default();
        let counter = std::sync::Arc::new(AtomicUsize::new(0));

        let k1 = key(TranslationModel::Gemini25Flash, Language::Eng, Language::Rus);
        let k2 = key(TranslationModel::Gemini25Pro, Language::Eng, Language::Rus);
        let k3 = key(TranslationModel::Gemini25Flash, Language::Eng, Language::Spa);

        for k in [k1, k2, k3] {
            let counter = counter.clone();
            registry
                .get_or_create_with::<_, _, anyhow::Error>(k, move || async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok(std::sync::Arc::new(1u32))
                })
                .await
                .unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn evict_clears_entry_so_init_runs_again() {
        let registry: Registry<u32> = Registry::default();
        let counter = std::sync::Arc::new(AtomicUsize::new(0));
        let k = key(TranslationModel::Gemini25Flash, Language::Eng, Language::Rus);

        let c1 = counter.clone();
        registry
            .get_or_create_with::<_, _, anyhow::Error>(k.clone(), || async move {
                c1.fetch_add(1, Ordering::SeqCst);
                Ok(std::sync::Arc::new(7u32))
            })
            .await
            .unwrap();
        let c2 = counter.clone();
        registry
            .get_or_create_with::<_, _, anyhow::Error>(k.clone(), || async move {
                c2.fetch_add(1, Ordering::SeqCst);
                Ok(std::sync::Arc::new(7u32))
            })
            .await
            .unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        registry.evict(&k).await;

        let c3 = counter.clone();
        registry
            .get_or_create_with::<_, _, anyhow::Error>(k.clone(), || async move {
                c3.fetch_add(1, Ordering::SeqCst);
                Ok(std::sync::Arc::new(7u32))
            })
            .await
            .unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn is_cache_missing_error_matches_403_with_cachedcontents_description() {
        let err = anyhow::Error::from(gemini_rust::ClientError::BadResponse {
            code: 403,
            description: Some("CachedContent not found".into()),
        });
        assert!(is_cache_missing_error(&err));

        let err = anyhow::Error::from(gemini_rust::ClientError::BadResponse {
            code: 404,
            description: Some("cachedContents/abc-123 has expired".into()),
        });
        assert!(is_cache_missing_error(&err));
    }

    #[test]
    fn is_cache_missing_error_rejects_unrelated_errors() {
        let err = anyhow::Error::from(gemini_rust::ClientError::BadResponse {
            code: 500,
            description: Some("CachedContent referenced".into()),
        });
        assert!(!is_cache_missing_error(&err));

        let err = anyhow::Error::from(gemini_rust::ClientError::BadResponse {
            code: 403,
            description: Some("Quota exceeded".into()),
        });
        assert!(!is_cache_missing_error(&err));

        let err = anyhow::anyhow!("some random error");
        assert!(!is_cache_missing_error(&err));
    }
}
