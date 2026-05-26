use std::{
    collections::HashMap,
    hash::Hasher,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gemini_rust::{CacheExpirationRequest, CachedContentHandle, Gemini};
use isolang::Language;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OnceCell};
use uuid::Uuid;

use crate::{cache::DiskCache, translator::TranslationModel};

/// Long-ish TTL so an active reading session keeps the cache warm without
/// rebuilding. Refreshed on every cache use (in-memory hit and disk-hit
/// reconstitution), so an idle book caches storage cost is bounded by
/// inactivity, not session length.
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub struct CacheKey {
    pub model: TranslationModel,
    pub from: Language,
    pub to: Language,
    pub book_id: Uuid,
    pub chapter_id: usize,
}

/// Cached payload split into the immutable system instruction (always
/// present) and the per-chapter reference material (summaries + chapter
/// text). The reference material is `None` only for the `NoChapterContext`
/// stub (CLI path), which has nothing chapter-scoped to send.
pub struct CacheContent {
    pub system_instruction: String,
    pub user_reference_material: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct DiskEntry {
    /// Server-side resource name, e.g. `"cachedContents/abc-123"`.
    name: String,
    /// FNV-1a of `(system_instruction || 0x1F || reference_material_or_empty)`.
    /// Per-`disk_key`, so 64-bit collision risk is negligible. Covers the
    /// system prompt so a FLTS prompt-text bump auto-invalidates entries.
    fingerprint: u64,
    /// Unix seconds. Informational only — server-side TTL is authoritative;
    /// we discover expiry via the existing 403/404 retry path.
    created_at: u64,
}

/// Disk-persisted Gemini content-cache directory. Maps a chapter-scoped
/// [`CacheKey`] to the server-side cache name so we don't re-upload the
/// system prompt + reference material on every app launch. An in-memory
/// single-flight layer ([`OnceCell`]-per-key) dedupes concurrent first-
/// callers within a single session.
pub struct GeminiPromptCache {
    disk: Arc<DiskCache<DiskEntry>>,
    inflight: Mutex<HashMap<CacheKey, Arc<OnceCell<Arc<CachedContentHandle>>>>>,
}

impl GeminiPromptCache {
    pub async fn open(dir: &Path, capacity_bytes: u64) -> anyhow::Result<Arc<Self>> {
        let disk = Arc::new(DiskCache::<DiskEntry>::open(dir, capacity_bytes).await?);
        Ok(Arc::new(Self {
            disk,
            inflight: Mutex::new(HashMap::new()),
        }))
    }

    pub async fn close(&self) {
        self.disk.close().await;
    }

    /// Drops the in-memory slot for `key` AND removes the persisted disk
    /// entry. Used to recover from a server-side cache that has expired or
    /// been deleted; the subsequent [`get_or_create`] runs `build_content`
    /// again and persists a fresh entry.
    pub async fn evict(&self, key: &CacheKey) {
        {
            let mut guard = self.inflight.lock().await;
            guard.remove(key);
        }
        self.disk.remove(&disk_key(key));
    }

    /// Returns a Gemini cache handle for `key`. Disk-hit (with matching
    /// fingerprint): reconstitute via [`Gemini::get_cached_content`] and
    /// fire a TTL refresh. Otherwise: invoke `build_content`, create the
    /// server-side cache, persist `(name, fingerprint, created_at)` to disk.
    ///
    /// Concurrent first-callers for the same `key` within one process
    /// share a single init future via the in-memory `OnceCell`.
    pub async fn get_or_create<F>(
        &self,
        client: &Gemini,
        key: CacheKey,
        build_content: F,
    ) -> anyhow::Result<Arc<CachedContentHandle>>
    where
        F: FnOnce() -> CacheContent,
    {
        let cell = {
            let mut guard = self.inflight.lock().await;
            guard
                .entry(key.clone())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        let existed_before_in_memory = cell.get().is_some();

        let disk = self.disk.clone();
        let key_for_init = key.clone();
        let handle = cell
            .get_or_try_init(|| async move {
                let content = build_content();
                let fingerprint = fingerprint_of(&content);
                let dk = disk_key(&key_for_init);

                if let Ok(Some(entry)) = disk.get(&dk).await
                    && entry.fingerprint == fingerprint
                {
                    info!(
                        "Reusing persisted Gemini cache {dk} ({})",
                        entry.name
                    );
                    let handle = Arc::new(client.get_cached_content(&entry.name));
                    spawn_ttl_refresh(handle.clone());
                    return anyhow::Ok(handle);
                }

                let display = cache_display_name(&key_for_init);
                info!(
                    "Creating Gemini cache {display} (system {} chars, reference {} chars, ttl {}s)",
                    content.system_instruction.len(),
                    content
                        .user_reference_material
                        .as_deref()
                        .map(str::len)
                        .unwrap_or(0),
                    DEFAULT_CACHE_TTL.as_secs()
                );
                let mut builder = client
                    .create_cache()
                    .with_display_name(display)?
                    .with_system_instruction(content.system_instruction)
                    .with_ttl(DEFAULT_CACHE_TTL);
                if let Some(reference) = content.user_reference_material {
                    builder = builder.with_user_message(reference);
                }
                let handle = builder.execute().await?;
                info!("Created Gemini cache: {}", handle.name());
                disk.insert(
                    dk,
                    DiskEntry {
                        name: handle.name().to_string(),
                        fingerprint,
                        created_at: now_secs(),
                    },
                );
                Ok(Arc::new(handle))
            })
            .await?;

        let handle = handle.clone();
        if existed_before_in_memory {
            spawn_ttl_refresh(handle.clone());
        }

        Ok(handle)
    }
}

fn spawn_ttl_refresh(handle: Arc<CachedContentHandle>) {
    tokio::spawn(async move {
        let req = CacheExpirationRequest::from_ttl(DEFAULT_CACHE_TTL);
        if let Err(err) = handle.update(req).await {
            warn!(
                "Failed to refresh Gemini cache TTL ({}): {err}",
                handle.name()
            );
        }
    });
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn fingerprint_of(content: &CacheContent) -> u64 {
    let mut h = fnv::FnvHasher::default();
    h.write(content.system_instruction.as_bytes());
    h.write(&[0x1F]);
    if let Some(r) = &content.user_reference_material {
        h.write(r.as_bytes());
    }
    h.finish()
}

fn disk_key(key: &CacheKey) -> String {
    format!(
        "flts-gemini-{}-{}-{}-{}-c{}",
        usize::from(key.model),
        key.from.to_639_3(),
        key.to.to_639_3(),
        key.book_id,
        key.chapter_id,
    )
}

fn cache_display_name(key: &CacheKey) -> String {
    // Gemini's display name cap is 128 chars — book uuid is 36, the rest
    // adds ~20, well within the limit.
    format!(
        "flts-{}-{}-{}-{}-c{}",
        usize::from(key.model),
        key.from.to_639_3(),
        key.to.to_639_3(),
        key.book_id,
        key.chapter_id,
    )
}

/// Compose the per-chapter reference-material payload that the cache
/// holds. Returns `None` when both pieces are empty (e.g.,
/// `NoChapterContext`), so the cache effectively reverts to system-prompt
/// only.
pub fn build_reference_material(prior_summaries: &str, chapter_text: &str) -> Option<String> {
    if prior_summaries.is_empty() && chapter_text.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(prior_summaries.len() + chapter_text.len() + 256);
    if !prior_summaries.is_empty() {
        out.push_str("Summaries of prior chapters in this book (for cross-chapter context only — do not translate them):\n\n");
        out.push_str(prior_summaries);
        out.push_str("\n\n");
    }
    if !chapter_text.is_empty() {
        out.push_str("Full text of the current chapter (use as surrounding context; the specific paragraph to translate will follow in a separate message):\n\n");
        out.push_str(chapter_text);
    }
    Some(out)
}

/// True if `err` indicates the cached content reference was rejected by the
/// server (expired, deleted, or wrong account). The caller should
/// [`GeminiPromptCache::evict`] the key and retry once with a fresh cache.
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
    use std::path::{Path, PathBuf};
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use isolang::Language;
    use uuid::Uuid;

    use super::*;
    use crate::translator::TranslationModel;

    fn key(model: TranslationModel, from: Language, to: Language, chapter: usize) -> CacheKey {
        CacheKey {
            model,
            from,
            to,
            book_id: Uuid::nil(),
            chapter_id: chapter,
        }
    }

    fn fake_client() -> Gemini {
        // `get_cached_content` constructs a handle locally without hitting
        // the network. The spawned TTL refresh inside `get_or_create` will
        // fail against this fake key, but it's detached and only logs.
        Gemini::with_model("fake-key-for-tests", gemini_rust::Model::Gemini25Flash).unwrap()
    }

    fn tmpdir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "flts-gemini-cache-test-{}-{}",
            name,
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_content(label: &str) -> CacheContent {
        CacheContent {
            system_instruction: format!("instruction-{label}"),
            user_reference_material: Some(format!("reference-{label}")),
        }
    }

    async fn seed_then_open(
        dir: &Path,
        entries: &[(CacheKey, &CacheContent, &str)],
    ) -> StdArc<GeminiPromptCache> {
        let cache = GeminiPromptCache::open(dir, 1024 * 1024).await.unwrap();
        for (k, content, name) in entries {
            cache.disk.insert(
                disk_key(k),
                DiskEntry {
                    name: (*name).to_string(),
                    fingerprint: fingerprint_of(content),
                    created_at: 0,
                },
            );
        }
        // Close + reopen guarantees the writer flushed the seeded entries
        // (insert is fire-and-forget through a channel).
        cache.close().await;
        GeminiPromptCache::open(dir, 1024 * 1024).await.unwrap()
    }

    #[tokio::test]
    async fn single_concurrent_create_for_same_key() {
        let dir = tmpdir("dedup");
        let content = make_content("a");
        let k = key(TranslationModel::Gemini25Flash, Language::Eng, Language::Rus, 0);
        let cache = seed_then_open(&dir, &[(k.clone(), &content, "cachedContents/seeded")]).await;

        let counter = StdArc::new(AtomicUsize::new(0));
        let client = fake_client();
        let mut futs = Vec::new();
        for _ in 0..16 {
            let cache = cache.clone();
            let counter = counter.clone();
            let k = k.clone();
            let client = client.clone();
            futs.push(async move {
                cache
                    .get_or_create(&client, k, move || {
                        counter.fetch_add(1, Ordering::SeqCst);
                        make_content("a")
                    })
                    .await
                    .unwrap()
            });
        }
        let results = futures_util::future::join_all(futs).await;

        assert_eq!(counter.load(Ordering::SeqCst), 1);
        for h in &results {
            assert_eq!(h.name(), "cachedContents/seeded");
        }
        cache.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn different_keys_create_independently() {
        let dir = tmpdir("different-keys");
        let c_a = make_content("a");
        let c_b = make_content("b");
        let c_c = make_content("c");
        let k1 = key(TranslationModel::Gemini25Flash, Language::Eng, Language::Rus, 0);
        let k2 = key(TranslationModel::Gemini25Pro, Language::Eng, Language::Rus, 0);
        let k3 = key(TranslationModel::Gemini25Flash, Language::Eng, Language::Spa, 0);
        let cache = seed_then_open(
            &dir,
            &[
                (k1.clone(), &c_a, "cachedContents/k1"),
                (k2.clone(), &c_b, "cachedContents/k2"),
                (k3.clone(), &c_c, "cachedContents/k3"),
            ],
        )
        .await;

        let counter = StdArc::new(AtomicUsize::new(0));
        let client = fake_client();
        for (k, label) in [(k1, "a"), (k2, "b"), (k3, "c")] {
            let counter = counter.clone();
            cache
                .get_or_create(&client, k, move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                    make_content(label)
                })
                .await
                .unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 3);
        cache.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn evict_clears_inflight_and_disk() {
        let dir = tmpdir("evict");
        let content = make_content("a");
        let k = key(TranslationModel::Gemini25Flash, Language::Eng, Language::Rus, 0);
        let cache = seed_then_open(&dir, &[(k.clone(), &content, "cachedContents/seeded")]).await;

        // First call: disk-hit, reconstitutes.
        let counter = StdArc::new(AtomicUsize::new(0));
        let client = fake_client();
        let h = {
            let counter = counter.clone();
            cache
                .get_or_create(&client, k.clone(), move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                    make_content("a")
                })
                .await
                .unwrap()
        };
        assert_eq!(h.name(), "cachedContents/seeded");
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        cache.evict(&k).await;
        cache.close().await;

        // Reopen and confirm the disk entry is gone (no reuse possible).
        let cache = GeminiPromptCache::open(&dir, 1024 * 1024).await.unwrap();
        assert!(cache.disk.get(&disk_key(&k)).await.unwrap().is_none());
        cache.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn persisted_entry_survives_close_reopen() {
        let dir = tmpdir("reopen");
        let content = make_content("a");
        let k = key(TranslationModel::Gemini25Flash, Language::Eng, Language::Rus, 0);
        let cache = seed_then_open(&dir, &[(k.clone(), &content, "cachedContents/persisted")]).await;

        let counter = StdArc::new(AtomicUsize::new(0));
        let client = fake_client();
        let h = {
            let counter = counter.clone();
            cache
                .get_or_create(&client, k.clone(), move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                    make_content("a")
                })
                .await
                .unwrap()
        };
        assert_eq!(h.name(), "cachedContents/persisted");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        cache.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fingerprint_changes_with_content() {
        let a = CacheContent {
            system_instruction: "sys".into(),
            user_reference_material: Some("ref".into()),
        };
        let b = CacheContent {
            system_instruction: "sys".into(),
            user_reference_material: Some("ref-modified".into()),
        };
        let c = CacheContent {
            system_instruction: "sys-modified".into(),
            user_reference_material: Some("ref".into()),
        };
        let d = CacheContent {
            system_instruction: "sys".into(),
            user_reference_material: None,
        };
        assert_ne!(fingerprint_of(&a), fingerprint_of(&b));
        assert_ne!(fingerprint_of(&a), fingerprint_of(&c));
        assert_ne!(fingerprint_of(&a), fingerprint_of(&d));
        assert_eq!(fingerprint_of(&a), fingerprint_of(&a));
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
