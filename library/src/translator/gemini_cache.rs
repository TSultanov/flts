use std::{
    collections::HashMap,
    hash::Hasher,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures_util::StreamExt;
use gemini_rust::{CacheExpirationRequest, CachedContentHandle, Gemini};
use isolang::Language;
use log::info;
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
};
use uuid::Uuid;

use crate::cache::DiskCache;

/// Refreshed on every use, so the TTL only sizes the post-last-use billing
/// tail. Storage bills per token-hour on every chapter the reader leaves
/// behind, so 1h trades a rare rebuild after a long pause for a small tail.
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Minimum spacing between TTL-refresh PATCHes per key. Half the TTL keeps the
/// cache warm at ~1 refresh per window instead of one per paragraph.
const REFRESH_MIN_INTERVAL_SECS: u64 = DEFAULT_CACHE_TTL.as_secs() / 2;

/// Display-name prefix on every FLTS-created cache.
/// [`GeminiPromptCache::purge_all`] deletes exactly the matching server-side
/// caches, so this must stay in sync with [`cache_display_name`].
pub const FLTS_CACHE_DISPLAY_PREFIX: &str = "flts-";

#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub struct CacheKey {
    pub model: String,
    pub from: Language,
    pub to: Language,
    pub book_id: Uuid,
    pub chapter_id: usize,
}

/// Cached payload: per-chapter reference material (`None` only when there is
/// nothing chapter-scoped to send) followed by the translation instructions.
pub struct CacheContent {
    pub instructions: String,
    pub user_reference_material: Option<String>,
}

/// Outcome of [`GeminiPromptCache::purge_all`].
pub struct PurgeReport {
    /// Number of server-side caches successfully deleted.
    pub deleted: usize,
    /// One `"name: error"` entry per cache whose DELETE failed.
    pub failures: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct DiskEntry {
    /// Server-side resource name, e.g. `"cachedContents/abc-123"`.
    name: String,
    /// FNV-1a of `(layout || instructions || 0x1F || reference_material_or_empty)`,
    /// scoped per-`disk_key`. Covering the prompt auto-invalidates entries when
    /// its text changes.
    fingerprint: u64,
    /// Unix seconds, informational: the server-side TTL is authoritative and
    /// expiry surfaces through the 403/404 retry path.
    created_at: u64,
}

/// Maps a chapter-scoped [`CacheKey`] to its server-side cache name, so a
/// launch doesn't re-upload the prompt and reference material. A per-key
/// [`OnceCell`] dedupes concurrent first-callers in one session.
type CacheReply = oneshot::Sender<Result<Arc<CachedContentHandle>, String>>;
enum CacheRequest {
    Get {
        key: CacheKey,
        client: Gemini,
        build: Box<dyn FnOnce() -> CacheContent + Send>,
        reply: CacheReply,
    },
    Invalidate {
        key: Option<CacheKey>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Close {
        reply: oneshot::Sender<()>,
    },
    #[cfg(test)]
    Size(oneshot::Sender<usize>),
}
#[derive(Clone)]
pub struct GeminiPromptCache {
    #[cfg(test)]
    disk: DiskCache<DiskEntry>,
    requests: mpsc::Sender<CacheRequest>,
    control: crate::task::TaskControl,
}
impl GeminiPromptCache {
    pub async fn open(
        dir: &Path,
        capacity_bytes: u64,
    ) -> anyhow::Result<(Self, crate::task::TaskOwner)> {
        let (disk, disk_owner) = DiskCache::open(dir, capacity_bytes).await?;
        let (requests, receiver) = mpsc::channel(256);
        #[cfg(test)]
        let test_disk = disk.clone();
        let keepalive = requests.clone();
        let owner = crate::task::TaskOwner::spawn(move |stop| async move {
            let _keepalive = keepalive;
            serve_prompt_cache(disk, receiver, stop).await;
            let _ = disk_owner.shutdown().await;
        });
        Ok((
            Self {
                #[cfg(test)]
                disk: test_disk,
                requests,
                control: owner.control(),
            },
            owner,
        ))
    }
    pub async fn close(&self) {
        let (reply, result) = oneshot::channel();
        if self
            .requests
            .send(CacheRequest::Close { reply })
            .await
            .is_ok()
        {
            let _ = result.await;
        }
        let _ = self.control.wait().await;
    }
    async fn invalidate(&self, key: Option<CacheKey>) -> anyhow::Result<()> {
        let (reply, result) = oneshot::channel();
        self.requests
            .send(CacheRequest::Invalidate { key, reply })
            .await
            .map_err(|_| anyhow::anyhow!("prompt cache closed"))?;
        result.await?.map_err(anyhow::Error::msg)
    }
    pub async fn clear_local(&self) -> anyhow::Result<()> {
        self.invalidate(None).await
    }
    pub async fn evict(&self, key: &CacheKey) -> anyhow::Result<()> {
        self.invalidate(Some(key.clone())).await
    }
    pub async fn get_or_create<F: FnOnce() -> CacheContent + Send + 'static>(
        &self,
        client: &Gemini,
        key: CacheKey,
        build: F,
    ) -> anyhow::Result<Arc<CachedContentHandle>> {
        let (reply, result) = oneshot::channel();
        self.requests
            .send(CacheRequest::Get {
                key,
                client: client.clone(),
                build: Box::new(build),
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("prompt cache closed"))?;
        result.await?.map_err(anyhow::Error::msg)
    }
    #[cfg(test)]
    async fn cached_len(&self) -> usize {
        let (reply, result) = oneshot::channel();
        self.requests.send(CacheRequest::Size(reply)).await.unwrap();
        result.await.unwrap()
    }
    pub async fn purge_all(&self, api_key: &str) -> anyhow::Result<PurgeReport> {
        let client = Gemini::new(api_key)?;

        let mut names = Vec::new();
        {
            let stream = client.list_cached_contents(100);
            futures_util::pin_mut!(stream);
            while let Some(summary) = stream.next().await {
                let summary = summary?;
                if is_flts_cache(summary.display_name.as_deref()) {
                    names.push(summary.name);
                }
            }
        }

        let mut deleted = 0usize;
        let mut failures = Vec::new();
        for name in names {
            match client.get_cached_content(&name).delete().await {
                Ok(()) => {
                    info!("Deleted Gemini cache {name}");
                    deleted += 1;
                }
                Err((_, err)) => failures.push(format!("{name}: {err}")),
            }
        }

        self.clear_local().await?;
        Ok(PurgeReport { deleted, failures })
    }
}
enum CacheFinished {
    Created(
        u64,
        CacheKey,
        anyhow::Result<(Arc<CachedContentHandle>, Option<DiskEntry>)>,
    ),
    Refreshed,
}
async fn create_cached(
    disk: &DiskCache<DiskEntry>,
    key: CacheKey,
    client: Gemini,
    content: CacheContent,
) -> anyhow::Result<(Arc<CachedContentHandle>, Option<DiskEntry>)> {
    let fingerprint = fingerprint_of(&content);
    if let Some(entry) = disk.get(&disk_key(&key)).await? {
        if entry.fingerprint == fingerprint {
            return Ok((Arc::new(client.get_cached_content(&entry.name)), None));
        }
    }
    let mut builder = client
        .create_cache()
        .with_display_name(cache_display_name(&key))?
        .with_ttl(DEFAULT_CACHE_TTL);
    if let Some(reference) = content.user_reference_material {
        builder = builder.with_user_message(reference);
    }
    let handle = Arc::new(
        builder
            .with_user_message(content.instructions)
            .execute()
            .await?,
    );
    let entry = DiskEntry {
        name: handle.name().to_owned(),
        fingerprint,
        created_at: now_secs(),
    };
    Ok((handle, Some(entry)))
}
async fn serve_prompt_cache(
    disk: DiskCache<DiskEntry>,
    mut requests: mpsc::Receiver<CacheRequest>,
    mut stop: watch::Receiver<bool>,
) {
    use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};
    use std::panic::AssertUnwindSafe;
    let mut jobs = FuturesUnordered::new();
    let mut builders = JoinSet::new();
    let mut next_id = 0u64;
    let mut current: HashMap<CacheKey, u64> = HashMap::new();
    let mut waiters: HashMap<u64, Vec<CacheReply>> = HashMap::new();
    let mut ready: HashMap<CacheKey, Arc<CachedContentHandle>> = HashMap::new();
    let mut refreshed: HashMap<CacheKey, u64> = HashMap::new();
    let close_reply = loop {
        tokio::select! {
            biased;
            _ = stop.changed() => break None,
            request = requests.recv() => match request {
                Some(CacheRequest::Get {key, client, build, reply}) => {
                    if let Some(handle) = ready.get(&key) {
                        let now = now_secs();
                        if now.saturating_sub(refreshed.get(&key).copied().unwrap_or(0)) >= REFRESH_MIN_INTERVAL_SECS {
                            refreshed.insert(key.clone(), now); let handle = handle.clone();
                            jobs.push(async move {
                                let _ = AssertUnwindSafe(handle.update(CacheExpirationRequest::from_ttl(DEFAULT_CACHE_TTL))).catch_unwind().await;
                                CacheFinished::Refreshed
                            }.boxed());
                        }
                        let _ = reply.send(Ok(handle.clone())); continue;
                    }
                    let id = match current.get(&key) {Some(id) => *id, None => {
                        let id = next_id; next_id += 1;
                        let issued = key.clone();
                        builders.spawn_blocking(move || {
                            let content = std::panic::catch_unwind(AssertUnwindSafe(build)).map_err(|_| anyhow::anyhow!("cache content builder panicked"));
                            (id, issued, client, content)
                        });
                        current.insert(key, id); id
                    }};
                    waiters.entry(id).or_default().push(reply);
                }
                Some(CacheRequest::Invalidate {key, reply}) => {
                    let keys: Vec<_> = match &key {Some(key) => vec![key.clone()], None => current.keys().chain(ready.keys()).cloned().collect()};
                    for key in keys {
                        ready.remove(&key); refreshed.remove(&key);
                        if let Some(id) = current.remove(&key) {
                            if let Some(replies) = waiters.remove(&id) { for reply in replies { let _ = reply.send(Err("prompt cache invalidated; retry request".into())); } }
                        }
                    }
                    let result = match key {Some(key) => disk.remove_and_wait(&disk_key(&key)).await, None => disk.clear_and_wait().await};
                    let _ = reply.send(result.map_err(|err| err.to_string()));
                }
                Some(CacheRequest::Close {reply}) => break Some(reply),
                #[cfg(test)] Some(CacheRequest::Size(reply)) => { let _ = reply.send(ready.len() + current.len()); }
                None => break None,
            },
            Some(result) = builders.join_next(), if !builders.is_empty() => {
                if let Ok((id, key, client, content)) = result {
                    if current.get(&key) != Some(&id) { continue; }
                    let disk = &disk;
                    jobs.push(async move {
                        let result = match content {
                            Ok(content) => AssertUnwindSafe(create_cached(disk, key.clone(), client, content)).catch_unwind().await.unwrap_or_else(|_| Err(anyhow::anyhow!("prompt cache worker panicked"))),
                            Err(err) => Err(err),
                        };
                        CacheFinished::Created(id, key, result)
                    }.boxed());
                }
            },
            Some(result) = jobs.next(), if !jobs.is_empty() => {
                if let CacheFinished::Created(id, key, result) = result {
                    if current.get(&key) != Some(&id) { continue; }
                    current.remove(&key);
                    let result = result.map(|(handle, entry)| {
                        if let Some(entry) = entry { disk.insert(disk_key(&key), entry); }
                        ready.insert(key, handle.clone()); handle
                    }).map_err(|err| format!("{err:#}"));
                    if let Some(replies) = waiters.remove(&id) { for reply in replies { let _ = reply.send(result.clone()); } }
                }
            }
        }
    };
    requests.close();
    jobs.clear();
    while builders.join_next().await.is_some() {}
    drop(waiters);
    disk.close().await;
    if let Some(reply) = close_reply {
        let _ = reply.send(());
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Bumped when the cached message layout changes, so older caches holding the
/// same text in a different layout aren't reused.
const CACHE_LAYOUT_VERSION: u8 = 2;

fn fingerprint_of(content: &CacheContent) -> u64 {
    let mut h = fnv::FnvHasher::default();
    h.write(&[CACHE_LAYOUT_VERSION]);
    h.write(content.instructions.as_bytes());
    h.write(&[0x1F]);
    if let Some(r) = &content.user_reference_material {
        h.write(r.as_bytes());
    }
    h.finish()
}

fn sanitize_model(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn disk_key(key: &CacheKey) -> String {
    format!(
        "flts-gemini-{}-{}-{}-{}-c{}",
        sanitize_model(&key.model),
        key.from.to_639_3(),
        key.to.to_639_3(),
        key.book_id,
        key.chapter_id,
    )
}

fn cache_display_name(key: &CacheKey) -> String {
    // Gemini caps display names at 128 chars; this stays well under.
    format!(
        "{FLTS_CACHE_DISPLAY_PREFIX}{}-{}-{}-{}-c{}",
        sanitize_model(&key.model),
        key.from.to_639_3(),
        key.to.to_639_3(),
        key.book_id,
        key.chapter_id,
    )
}

/// Whether the display-name prefix marks a cache as FLTS-created.
fn is_flts_cache(display_name: Option<&str>) -> bool {
    display_name.is_some_and(|d| d.starts_with(FLTS_CACHE_DISPLAY_PREFIX))
}

/// The per-chapter reference-material payload, or `None` when both pieces are
/// empty so the cache holds the instructions alone.
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

/// Whether the server rejected the cached-content reference (expired, deleted,
/// wrong account). Callers [`GeminiPromptCache::evict`] and retry once.
pub fn is_cache_missing_error(err: &anyhow::Error) -> bool {
    let Some(ce) = err.downcast_ref::<gemini_rust::ClientError>() else {
        return false;
    };
    let gemini_rust::ClientError::BadResponse { code, description } = ce else {
        return false;
    };
    // Any 404 means the cache is gone; matching on wording would let an error
    // text change silently disable the evict-and-rebuild path.
    if *code == 404 {
        return true;
    }
    // Unrelated 403s (quota, auth) must not trigger a rebuild.
    if *code == 403 {
        return description
            .as_deref()
            .map(|d| {
                let d = d.to_lowercase();
                d.contains("cachedcontent")
                    || d.contains("cached content")
                    || d.contains("cache")
                    || d.contains("expired")
            })
            .unwrap_or(false);
    }
    false
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use isolang::Language;
    use uuid::Uuid;

    use super::*;

    fn key(model: &str, from: Language, to: Language, chapter: usize) -> CacheKey {
        CacheKey {
            model: model.to_string(),
            from,
            to,
            book_id: Uuid::nil(),
            chapter_id: chapter,
        }
    }

    fn fake_client() -> Gemini {
        // `get_cached_content` builds a handle without network; the detached
        // TTL refresh fails harmlessly against this key.
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
            instructions: format!("instruction-{label}"),
            user_reference_material: Some(format!("reference-{label}")),
        }
    }

    async fn seed_then_open(
        dir: &Path,
        entries: &[(CacheKey, &CacheContent, &str)],
    ) -> (GeminiPromptCache, crate::task::TaskOwner) {
        let (cache, _owner) = GeminiPromptCache::open(dir, 1024 * 1024).await.unwrap();
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
        // `insert` is fire-and-forget, so close+reopen to flush the writer.
        cache.close().await;
        GeminiPromptCache::open(dir, 1024 * 1024).await.unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn invalidate_resolves_waiters_and_rejects_obsolete_creation() {
        let dir = tmpdir("invalidate-creation");
        let content = make_content("a");
        let key = key("models/gemini-2.5-flash", Language::Eng, Language::Rus, 0);
        let (cache, _owner) =
            seed_then_open(&dir, &[(key.clone(), &content, "cachedContents/seeded")]).await;
        let entered = StdArc::new(tokio::sync::Notify::new());
        let (release, blocked) = std::sync::mpsc::channel();
        let old = tokio::spawn({
            let cache = cache.clone();
            let key = key.clone();
            let entered = entered.clone();
            async move {
                cache
                    .get_or_create(&fake_client(), key, move || {
                        entered.notify_one();
                        blocked.recv().unwrap();
                        make_content("a")
                    })
                    .await
            }
        });
        entered.notified().await;
        cache.evict(&key).await.unwrap();
        assert!(old.await.unwrap().is_err());
        cache.disk.insert(
            disk_key(&key),
            DiskEntry {
                name: "cachedContents/new".into(),
                fingerprint: fingerprint_of(&content),
                created_at: 0,
            },
        );
        cache.disk.flush().await.unwrap();
        release.send(()).unwrap();
        let fresh = cache
            .get_or_create(&fake_client(), key, || make_content("a"))
            .await
            .unwrap();
        assert_eq!(fresh.name(), "cachedContents/new");
        cache.close().await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn single_concurrent_create_for_same_key() {
        let dir = tmpdir("dedup");
        let content = make_content("a");
        let k = key("models/gemini-2.5-flash", Language::Eng, Language::Rus, 0);
        let (cache, _owner) =
            seed_then_open(&dir, &[(k.clone(), &content, "cachedContents/seeded")]).await;

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
        let k1 = key("models/gemini-2.5-flash", Language::Eng, Language::Rus, 0);
        let k2 = key("models/gemini-2.5-pro", Language::Eng, Language::Rus, 0);
        let k3 = key("models/gemini-2.5-flash", Language::Eng, Language::Spa, 0);
        let (cache, _owner) = seed_then_open(
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
        let k = key("models/gemini-2.5-flash", Language::Eng, Language::Rus, 0);
        let (cache, _owner) =
            seed_then_open(&dir, &[(k.clone(), &content, "cachedContents/seeded")]).await;

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

        cache.evict(&k).await.unwrap();
        cache.close().await;

        let (cache, _owner) = GeminiPromptCache::open(&dir, 1024 * 1024).await.unwrap();
        assert!(cache.disk.get(&disk_key(&k)).await.unwrap().is_none());
        cache.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn persisted_entry_survives_close_reopen() {
        let dir = tmpdir("reopen");
        let content = make_content("a");
        let k = key("models/gemini-2.5-flash", Language::Eng, Language::Rus, 0);
        let (cache, _owner) =
            seed_then_open(&dir, &[(k.clone(), &content, "cachedContents/persisted")]).await;

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
    fn is_flts_cache_matches_only_prefixed_names() {
        assert!(is_flts_cache(Some("flts-1-eng-rus-uuid-c0")));
        assert!(is_flts_cache(Some("flts-1-eng-rus")));
        assert!(!is_flts_cache(Some("flts")));
        assert!(!is_flts_cache(Some("other-cache")));
        assert!(!is_flts_cache(None));
    }

    #[test]
    fn cache_display_name_starts_with_purge_prefix() {
        let k = key("models/gemini-2.5-flash", Language::Eng, Language::Rus, 3);
        assert!(cache_display_name(&k).starts_with(FLTS_CACHE_DISPLAY_PREFIX));
    }

    #[tokio::test]
    async fn clear_local_drops_inflight_and_disk() {
        let dir = tmpdir("clear-local");
        let c_a = make_content("a");
        let c_b = make_content("b");
        let k1 = key("models/gemini-2.5-flash", Language::Eng, Language::Rus, 0);
        let k2 = key("models/gemini-2.5-flash", Language::Eng, Language::Rus, 1);
        let (cache, _owner) = seed_then_open(
            &dir,
            &[
                (k1.clone(), &c_a, "cachedContents/k1"),
                (k2.clone(), &c_b, "cachedContents/k2"),
            ],
        )
        .await;

        let client = fake_client();
        cache
            .get_or_create(&client, k1.clone(), || make_content("a"))
            .await
            .unwrap();
        assert!(cache.cached_len().await > 0);

        cache.clear_local().await.unwrap();
        assert!(cache.cached_len().await == 0);
        cache.close().await;

        let (cache, _owner) = GeminiPromptCache::open(&dir, 1024 * 1024).await.unwrap();
        assert!(cache.disk.get(&disk_key(&k1)).await.unwrap().is_none());
        assert!(cache.disk.get(&disk_key(&k2)).await.unwrap().is_none());
        cache.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fingerprint_changes_with_content() {
        let a = CacheContent {
            instructions: "sys".into(),
            user_reference_material: Some("ref".into()),
        };
        let b = CacheContent {
            instructions: "sys".into(),
            user_reference_material: Some("ref-modified".into()),
        };
        let c = CacheContent {
            instructions: "sys-modified".into(),
            user_reference_material: Some("ref".into()),
        };
        let d = CacheContent {
            instructions: "sys".into(),
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

    #[test]
    fn is_cache_missing_error_treats_any_404_and_broad_403_terms_as_missing() {
        let err = anyhow::Error::from(gemini_rust::ClientError::BadResponse {
            code: 404,
            description: Some("resource not found".into()),
        });
        assert!(is_cache_missing_error(&err));

        let err = anyhow::Error::from(gemini_rust::ClientError::BadResponse {
            code: 403,
            description: Some("the cache has expired".into()),
        });
        assert!(is_cache_missing_error(&err));

        let err = anyhow::Error::from(gemini_rust::ClientError::BadResponse {
            code: 403,
            description: Some("Permission denied on resource".into()),
        });
        assert!(!is_cache_missing_error(&err));
    }
}
