use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
    sync::{Arc, Weak},
};

use tokio::sync::RwLock;

/// Two-tier in-memory cache that enforces a single-instance-per-key invariant
/// while bounding the strong-reference footprint.
///
/// `weak_by_id` is the identity index — it never keeps a value alive. As long
/// as any `Arc` returned by `get` or `insert` is held anywhere in the process,
/// a concurrent lookup for the same key resolves to that same `Arc` via the
/// weak upgrade, so eviction cannot create a divergent second instance.
///
/// `warm_lru` is the only strong pin. Bounded by `capacity`; oldest pin falls
/// out when capacity is exceeded. When the pin is the only strong ref, the
/// value unloads at that moment; otherwise it lives until external holders drop.
pub struct WeakLruCache<K, V> {
    inner: RwLock<Inner<K, V>>,
}

struct Inner<K, V> {
    weak_by_id: HashMap<K, Weak<V>>,
    warm_lru: VecDeque<(K, Arc<V>)>,
    capacity: usize,
}

impl<K, V> WeakLruCache<K, V>
where
    K: Eq + Hash + Clone,
{
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: RwLock::new(Inner {
                weak_by_id: HashMap::new(),
                warm_lru: VecDeque::with_capacity(capacity),
                capacity,
            }),
        }
    }

    pub async fn get(&self, key: &K) -> Option<Arc<V>> {
        let mut inner = self.inner.write().await;
        let value = inner.weak_by_id.get(key).and_then(Weak::upgrade)?;

        // Refresh recency so an actively-read value isn't treated as the
        // oldest insert and evicted underneath its reader (which would force a
        // disk reload on the next access). Move its warm pin to the back; if
        // the value is live but had already fallen out of the warm set, re-pin
        // it (possibly evicting the current oldest).
        if let Some(pos) = inner.warm_lru.iter().position(|(k, _)| k == key) {
            if let Some(entry) = inner.warm_lru.remove(pos) {
                inner.warm_lru.push_back(entry);
            }
        } else {
            if inner.warm_lru.len() >= inner.capacity {
                inner.warm_lru.pop_front();
            }
            inner.warm_lru.push_back((key.clone(), value.clone()));
        }

        Some(value)
    }

    /// Insert a freshly-loaded value. If a concurrent task already inserted
    /// a live value for `key`, the caller's `value` is dropped and the
    /// existing `Arc` is returned. Otherwise the new value is registered
    /// (pinned in the LRU, possibly evicting the oldest) and returned back.
    pub async fn insert(&self, key: K, value: Arc<V>) -> Arc<V> {
        let mut inner = self.inner.write().await;

        if let Some(existing) = inner.weak_by_id.get(&key).and_then(Weak::upgrade) {
            return existing;
        }

        inner.weak_by_id.retain(|_, w| w.strong_count() > 0);

        if inner.warm_lru.len() >= inner.capacity {
            inner.warm_lru.pop_front();
        }
        inner.warm_lru.push_back((key.clone(), value.clone()));
        inner.weak_by_id.insert(key, Arc::downgrade(&value));
        value
    }

    pub async fn remove(&self, key: &K) {
        let mut inner = self.inner.write().await;
        inner.weak_by_id.remove(key);
        inner.warm_lru.retain(|(k, _)| k != key);
    }

    pub async fn live_values(&self) -> Vec<Arc<V>> {
        self.inner
            .read()
            .await
            .weak_by_id
            .values()
            .filter_map(Weak::upgrade)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn insert_returns_existing_on_race() {
        let cache: WeakLruCache<&'static str, String> = WeakLruCache::new(4);

        let first = Arc::new("first".to_string());
        let returned_first = cache.insert("k", first.clone()).await;
        assert!(Arc::ptr_eq(&first, &returned_first));

        let second = Arc::new("second".to_string());
        let returned_second = cache.insert("k", second.clone()).await;
        assert!(
            Arc::ptr_eq(&first, &returned_second),
            "second insert must return the original Arc, not the racer's",
        );
        assert!(!Arc::ptr_eq(&second, &returned_second));
    }

    #[tokio::test]
    async fn get_after_eviction_returns_none_when_no_holder() {
        let cache: WeakLruCache<usize, String> = WeakLruCache::new(2);

        cache.insert(1, Arc::new("one".to_string())).await;
        cache.insert(2, Arc::new("two".to_string())).await;
        cache.insert(3, Arc::new("three".to_string())).await;

        assert!(
            cache.get(&1).await.is_none(),
            "oldest entry should have been evicted and unreferenced",
        );
        assert!(cache.get(&2).await.is_some());
        assert!(cache.get(&3).await.is_some());
    }

    #[tokio::test]
    async fn get_after_eviction_returns_same_arc_while_held() {
        let cache: WeakLruCache<usize, String> = WeakLruCache::new(2);

        let held = cache.insert(1, Arc::new("pinned".to_string())).await;
        cache.insert(2, Arc::new("two".to_string())).await;
        cache.insert(3, Arc::new("three".to_string())).await;

        let upgraded = cache.get(&1).await.expect("Weak must upgrade");
        assert!(Arc::ptr_eq(&held, &upgraded));
    }

    #[tokio::test]
    async fn remove_clears_both_indices() {
        let cache: WeakLruCache<usize, String> = WeakLruCache::new(4);

        let _pinned = cache.insert(1, Arc::new("one".to_string())).await;
        cache.remove(&1).await;

        assert!(cache.get(&1).await.is_none());
        assert!(cache.live_values().await.is_empty());
    }

    #[tokio::test]
    async fn get_refreshes_recency_so_touched_entry_survives_eviction() {
        // No external holders: the warm pin is the only strong ref, so an
        // evicted key's Weak dies and get() returns None.
        let cache: WeakLruCache<usize, String> = WeakLruCache::new(2);
        cache.insert(1, Arc::new("one".to_string())).await;
        cache.insert(2, Arc::new("two".to_string())).await;

        // Touch key 1 -> most-recently-used. Under the old FIFO behavior get
        // was a no-op and key 1 (oldest insert) would be evicted next.
        assert!(cache.get(&1).await.is_some());

        // Insert a third: capacity 2 evicts the least-recently-used, which is
        // now key 2, not the freshly-touched key 1.
        cache.insert(3, Arc::new("three".to_string())).await;

        assert!(cache.get(&1).await.is_some(), "touched key 1 must survive");
        assert!(cache.get(&2).await.is_none(), "untouched key 2 must be evicted");
        assert!(cache.get(&3).await.is_some());
    }

    #[tokio::test]
    async fn live_values_excludes_dropped_entries() {
        let cache: WeakLruCache<usize, String> = WeakLruCache::new(1);

        cache.insert(1, Arc::new("first".to_string())).await;
        cache.insert(2, Arc::new("second".to_string())).await;

        let live = cache.live_values().await;
        assert_eq!(live.len(), 1);
        assert_eq!(**live.first().unwrap(), "second");
    }
}
