use std::collections::HashMap;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::SystemTime;

use crate::task::{TaskControl, TaskOwner};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{
    mpsc::{Receiver, Sender, channel},
    oneshot, watch,
};

const ZSTD_LEVEL: i32 = 3;

pub struct DiskCache<V> {
    dir: PathBuf,
    index: Arc<StdMutex<Index>>,
    writer_tx: Sender<WriteOp<V>>,
    control: TaskControl,
}

impl<V> Clone for DiskCache<V> {
    fn clone(&self) -> Self {
        Self {
            dir: self.dir.clone(),
            index: self.index.clone(),
            writer_tx: self.writer_tx.clone(),
            control: self.control.clone(),
        }
    }
}

#[derive(Default)]
struct Index {
    entries: HashMap<u64, EntryMeta>,
    total_bytes: u64,
    generation: u64,
}

#[derive(Clone, Copy)]
struct EntryMeta {
    size: u64,
    last_access: SystemTime,
}

enum WriteOp<V> {
    Insert {
        hash: u64,
        key: String,
        value: V,
    },
    Remove {
        hash: u64,
    },
    Clear,
    Acknowledged {
        op: Box<WriteOp<V>>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Flush,
}

fn hash_key(key: &str) -> u64 {
    let mut h = fnv::FnvHasher::default();
    h.write(key.as_bytes());
    h.finish()
}

fn entry_path(dir: &Path, hash: u64) -> PathBuf {
    let hex = format!("{:016x}", hash);
    dir.join(&hex[..2]).join(format!("{}.bin", hex))
}

impl<V> DiskCache<V>
where
    V: Serialize + DeserializeOwned + Send + 'static,
{
    pub async fn open(dir: &Path, capacity_bytes: u64) -> anyhow::Result<(Self, TaskOwner)> {
        tokio::fs::create_dir_all(dir).await?;

        let dir_owned = dir.to_path_buf();
        let index = tokio::task::spawn_blocking(move || scan_dir(&dir_owned)).await??;
        let index = Arc::new(StdMutex::new(index));

        let (tx, rx) = channel::<WriteOp<V>>(256);
        let writer_dir = dir.to_path_buf();
        let writer_index = index.clone();
        let keepalive = tx.clone();
        let owner = TaskOwner::spawn(move |stop| async move {
            let _keepalive = keepalive;
            writer_loop(writer_dir, capacity_bytes, writer_index, rx, stop).await;
        });
        let cache = Self {
            dir: dir.to_path_buf(),
            index,
            writer_tx: tx,
            control: owner.control(),
        };
        Ok((cache, owner))
    }

    pub async fn get(&self, key: &str) -> anyhow::Result<Option<V>> {
        let hash = hash_key(key);
        let generation = {
            let idx = self.index.lock().unwrap();
            if !idx.entries.contains_key(&hash) {
                return Ok(None);
            }
            idx.generation
        };

        let path = entry_path(&self.dir, hash);
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        let key_owned = key.to_string();
        let decoded =
            tokio::task::spawn_blocking(move || decode::<V>(&bytes, &key_owned)).await??;

        if decoded.is_some() {
            let mut idx = self.index.lock().unwrap();
            if generation != idx.generation {
                return Ok(None);
            }
            if let Some(meta) = idx.entries.get_mut(&hash) {
                meta.last_access = SystemTime::now();
            }
        }
        Ok(decoded)
    }

    pub fn insert(&self, key: String, value: V) {
        let hash = hash_key(&key);
        {
            let _ = self
                .writer_tx
                .try_send(WriteOp::Insert { hash, key, value });
        }
    }

    /// Removes `key` from disk and the index, through the writer channel so it
    /// orders after a pending `insert`. No-op once the writer is down.
    pub fn remove(&self, key: &str) {
        let hash = hash_key(key);
        {
            let _ = self.writer_tx.try_send(WriteOp::Remove { hash });
        }
    }

    /// Removes every entry, through the writer channel so it orders after
    /// pending `insert`s; later ones survive. No-op once the writer is down.
    pub fn clear(&self) {
        {
            let _ = self.writer_tx.try_send(WriteOp::Clear);
        }
    }

    async fn acknowledged(&self, op: WriteOp<V>) -> anyhow::Result<()> {
        let tx = &self.writer_tx;
        let (reply, result) = oneshot::channel();
        tx.send(WriteOp::Acknowledged {
            op: Box::new(op),
            reply,
        })
        .await
        .map_err(|_| anyhow::anyhow!("disk cache writer closed"))?;
        result.await?.map_err(anyhow::Error::msg)
    }
    pub async fn remove_and_wait(&self, key: &str) -> anyhow::Result<()> {
        self.acknowledged(WriteOp::Remove {
            hash: hash_key(key),
        })
        .await
    }
    pub async fn clear_and_wait(&self) -> anyhow::Result<()> {
        self.acknowledged(WriteOp::Clear).await
    }
    pub async fn flush(&self) -> anyhow::Result<()> {
        self.acknowledged(WriteOp::Flush).await
    }
    pub async fn close(&self) {
        let _ = self.control.shutdown().await;
    }
}

fn scan_dir(dir: &Path) -> anyhow::Result<Index> {
    let mut index = Index::default();
    let shards = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(index),
        Err(e) => return Err(e.into()),
    };
    for shard in shards {
        let shard = match shard {
            Ok(s) => s,
            Err(_) => continue,
        };
        let shard_path = shard.path();
        if !shard_path.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(&shard_path) {
            Ok(it) => it,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(stem) = name.strip_suffix(".bin") else {
                continue;
            };
            let Ok(hash) = u64::from_str_radix(stem, 16) else {
                continue;
            };
            let Ok(meta) = entry.metadata() else { continue };
            let size = meta.len();
            let last_access = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            index.total_bytes = index.total_bytes.saturating_add(size);
            index.entries.insert(hash, EntryMeta { size, last_access });
        }
    }
    Ok(index)
}

fn encode<V: Serialize>(key: &str, value: &V) -> anyhow::Result<Vec<u8>> {
    let json = serde_json::to_vec(&(key, value))?;
    let compressed = zstd::encode_all(json.as_slice(), ZSTD_LEVEL)?;
    Ok(compressed)
}

fn decode<V: DeserializeOwned>(bytes: &[u8], expected_key: &str) -> anyhow::Result<Option<V>> {
    let json = zstd::decode_all(bytes)?;
    let (stored_key, value): (String, V) = serde_json::from_slice(&json)?;
    if stored_key != expected_key {
        return Ok(None);
    }
    Ok(Some(value))
}

async fn writer_loop<V: Serialize + Send + 'static>(
    dir: PathBuf,
    capacity_bytes: u64,
    index: Arc<StdMutex<Index>>,
    mut rx: Receiver<WriteOp<V>>,
    mut stop: watch::Receiver<bool>,
) {
    let mut last_error: Option<String> = None;
    let mut closing = false;
    loop {
        let op = tokio::select! {
            biased;
            _ = stop.changed(), if !closing => { closing = true; rx.close(); continue; }
            op = rx.recv() => op,
        };
        let Some(op) = op else {
            break;
        };
        let (op, reply) = match op {
            WriteOp::Acknowledged { op, reply } => (*op, Some(reply)),
            op => (op, None),
        };
        let dir = dir.clone();
        let index = index.clone();
        let result = tokio::task::spawn_blocking(move || match op {
            WriteOp::Insert { hash, key, value } => {
                write_entry(&dir, capacity_bytes, &index, hash, key, value)
            }
            WriteOp::Remove { hash } => remove_entry(&dir, &index, hash),
            WriteOp::Clear => clear_all(&dir, &index),
            WriteOp::Flush => Ok(()),
            WriteOp::Acknowledged { .. } => unreachable!(),
        })
        .await
        .map_err(|err| err.to_string())
        .and_then(|result| result.map_err(|err| format!("{err:#}")));
        if let Err(err) = &result {
            log::warn!("disk cache write failed: {err}");
            last_error = Some(err.clone());
        }
        if let Some(reply) = reply {
            let _ = reply.send(match last_error.take() {
                Some(err) => Err(err),
                None => result,
            });
        }
    }
}

fn write_entry<V: Serialize>(
    dir: &Path,
    capacity_bytes: u64,
    index: &StdMutex<Index>,
    hash: u64,
    key: String,
    value: V,
) -> anyhow::Result<()> {
    let bytes = encode(&key, &value)?;
    let size = bytes.len() as u64;
    let path = entry_path(dir, hash);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let tmp = path.with_extension("bin.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;

    let now = SystemTime::now();
    let victims: Vec<u64> = {
        let mut idx = index.lock().unwrap();
        idx.generation = idx.generation.wrapping_add(1);
        if let Some(prev) = idx.entries.get(&hash).copied() {
            idx.total_bytes = idx.total_bytes.saturating_sub(prev.size);
        }
        idx.total_bytes = idx.total_bytes.saturating_add(size);
        idx.entries.insert(
            hash,
            EntryMeta {
                size,
                last_access: now,
            },
        );

        let mut victims = Vec::new();
        if idx.total_bytes > capacity_bytes {
            let mut by_access: Vec<(u64, EntryMeta)> = idx
                .entries
                .iter()
                .filter(|(h, _)| **h != hash)
                .map(|(h, m)| (*h, *m))
                .collect();
            by_access.sort_by_key(|(_, m)| m.last_access);
            for (h, m) in by_access {
                if idx.total_bytes <= capacity_bytes {
                    break;
                }
                idx.entries.remove(&h);
                idx.total_bytes = idx.total_bytes.saturating_sub(m.size);
                victims.push(h);
            }
        }
        victims
    };

    for h in victims {
        let _ = std::fs::remove_file(entry_path(dir, h));
    }
    Ok(())
}

/// Wipes the index first so concurrent `get`s miss, then deletes whole shard
/// directories, sweeping orphaned `.bin.tmp` files with them.
fn clear_all(dir: &Path, index: &StdMutex<Index>) -> anyhow::Result<()> {
    {
        let mut idx = index.lock().unwrap();
        idx.generation = idx.generation.wrapping_add(1);
        idx.entries.clear();
        idx.total_bytes = 0;
    }
    let shards = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for shard in shards.flatten() {
        let path = shard.path();
        if !path.is_dir() {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn remove_entry(dir: &Path, index: &StdMutex<Index>, hash: u64) -> anyhow::Result<()> {
    {
        let mut idx = index.lock().unwrap();
        idx.generation = idx.generation.wrapping_add(1);
        if let Some(prev) = idx.entries.remove(&hash) {
            idx.total_bytes = idx.total_bytes.saturating_sub(prev.size);
        }
    }
    let path = entry_path(dir, hash);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn acknowledged_remove_and_concurrent_close_are_ordered() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, _owner) = super::DiskCache::<String>::open(dir.path(), 4096)
            .await
            .unwrap();
        cache.insert("removed".into(), "old".into());
        cache.remove_and_wait("removed").await.unwrap();
        assert!(cache.get("removed").await.unwrap().is_none());
        cache.insert("kept".into(), "new".into());
        tokio::join!(cache.close(), cache.close());
        let (reopened, _owner) = super::DiskCache::<String>::open(dir.path(), 4096)
            .await
            .unwrap();
        assert_eq!(reopened.get("kept").await.unwrap().as_deref(), Some("new"));
        reopened.close().await;
    }

    #[tokio::test]
    async fn close_drains_with_live_client_clones_and_rejects_new_writes() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, owner) = super::DiskCache::<String>::open(dir.path(), 4096)
            .await
            .unwrap();
        let retained = cache.clone();
        cache.insert("key".into(), "value".into());
        tokio::time::timeout(std::time::Duration::from_secs(2), owner.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.get("key").await.unwrap().as_deref(), Some("value"));
        assert!(retained.flush().await.is_err());
    }

    #[tokio::test]
    async fn dropping_the_last_client_keeps_the_writer_owned_until_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, owner) = super::DiskCache::<String>::open(dir.path(), 4096)
            .await
            .unwrap();
        let completion = owner.control();
        drop(cache);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), completion.wait())
                .await
                .is_err()
        );
        owner.shutdown().await.unwrap();
    }

    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
    struct V {
        s: String,
        n: u32,
    }

    fn tmpdir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "flts-disk-cache-test-{}-{}",
            name,
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    async fn drain_close<V: Serialize + DeserializeOwned + Send + 'static>(c: DiskCache<V>) {
        c.close().await;
    }

    #[tokio::test]
    async fn roundtrip() {
        let dir = tmpdir("roundtrip");
        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        let v = V {
            s: "hello".into(),
            n: 7,
        };
        cache.insert("k1".into(), v.clone());
        drain_close(cache).await;

        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        let got = cache.get("k1").await.unwrap();
        assert_eq!(got, Some(v));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn miss_returns_none() {
        let dir = tmpdir("miss");
        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        assert!(cache.get("absent").await.unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn survives_reopen() {
        let dir = tmpdir("reopen");
        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        for i in 0..5 {
            cache.insert(
                format!("key-{i}"),
                V {
                    s: format!("val-{i}"),
                    n: i,
                },
            );
        }
        cache.close().await;

        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        for i in 0..5 {
            let got = cache.get(&format!("key-{i}")).await.unwrap();
            assert_eq!(
                got,
                Some(V {
                    s: format!("val-{i}"),
                    n: i,
                })
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_deletes_file_and_decrements_index() {
        let dir = tmpdir("remove");
        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        cache.insert(
            "doomed".into(),
            V {
                s: "bye".into(),
                n: 1,
            },
        );
        cache.insert(
            "kept".into(),
            V {
                s: "stay".into(),
                n: 2,
            },
        );
        cache.close().await;

        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        assert!(cache.get("doomed").await.unwrap().is_some());
        let bytes_before = cache.index.lock().unwrap().total_bytes;
        cache.remove("doomed");
        cache.close().await;

        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        assert!(cache.get("doomed").await.unwrap().is_none());
        assert!(cache.get("kept").await.unwrap().is_some());
        let bytes_after = cache.index.lock().unwrap().total_bytes;
        assert!(
            bytes_after < bytes_before,
            "total_bytes should decrease after remove ({bytes_before} -> {bytes_after})"
        );
        cache.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_missing_key_is_noop() {
        let dir = tmpdir("remove-missing");
        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        cache.remove("never-inserted");
        cache.close().await;
        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        assert!(cache.get("never-inserted").await.unwrap().is_none());
        cache.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn clear_removes_all_entries_and_resets_index() {
        let dir = tmpdir("clear-all");
        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        for i in 0..5 {
            cache.insert(
                format!("key-{i}"),
                V {
                    s: format!("val-{i}"),
                    n: i,
                },
            );
        }
        cache.clear();
        cache.close().await;

        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        for i in 0..5 {
            assert!(cache.get(&format!("key-{i}")).await.unwrap().is_none());
        }
        assert_eq!(cache.index.lock().unwrap().total_bytes, 0);
        cache.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn clear_orders_after_pending_inserts() {
        let dir = tmpdir("clear-ordering");
        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        cache.insert(
            "a".into(),
            V {
                s: "wiped".into(),
                n: 1,
            },
        );
        cache.clear();
        cache.insert(
            "b".into(),
            V {
                s: "survives".into(),
                n: 2,
            },
        );
        cache.close().await;

        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        assert!(cache.get("a").await.unwrap().is_none());
        assert!(cache.get("b").await.unwrap().is_some());
        cache.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn clear_on_empty_is_noop() {
        let dir = tmpdir("clear-empty");
        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        cache.clear();
        cache.close().await;

        let (cache, _owner) = DiskCache::<V>::open(&dir, 10 * 1024 * 1024).await.unwrap();
        assert_eq!(cache.index.lock().unwrap().total_bytes, 0);
        cache.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn eviction_under_capacity() {
        let dir = tmpdir("evict");
        let cap: u64 = 4 * 1024;
        let (cache, _owner) = DiskCache::<V>::open(&dir, cap).await.unwrap();
        for i in 0..40 {
            cache.insert(
                format!("key-{i:03}"),
                V {
                    s: "x".repeat(256),
                    n: i,
                },
            );
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        cache.close().await;

        let (cache, _owner) = DiskCache::<V>::open(&dir, cap).await.unwrap();
        let total = cache.index.lock().unwrap().total_bytes;
        assert!(
            total <= cap,
            "total_bytes={total} exceeded cap={cap} after eviction"
        );
        let newest = cache.get("key-039").await.unwrap();
        assert!(
            newest.is_some(),
            "most-recent entry should survive eviction"
        );
        cache.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
