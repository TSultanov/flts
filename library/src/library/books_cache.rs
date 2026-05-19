use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Weak},
};

use uuid::Uuid;

use crate::{library::library_book::LibraryBook, tla_trace::mutex::TracedMutex};

/// Default number of books to pin in the warm LRU. Books accessed beyond this
/// count are still reachable via the weak index while any holder keeps them
/// alive; once the last holder drops, they unload.
pub const DEFAULT_CACHE_CAPACITY: usize = 8;

/// Two-tier book cache that enforces a single-instance-per-Uuid invariant
/// while bounding the strong-reference footprint.
///
/// `weak_by_id` is the identity index — it never keeps a book alive. As long
/// as any `Arc` returned by `get_book` is held anywhere in the process, a
/// concurrent lookup for the same Uuid resolves to that same `Arc` via the
/// weak upgrade, so eviction cannot create a divergent second instance.
///
/// `warm_lru` is the only strong pin. Bounded by `capacity`; oldest pin falls
/// out when capacity is exceeded. When the pin is the only strong ref, the
/// book unloads at that moment; otherwise it lives until external holders drop.
pub(crate) struct BooksCache {
    pub(super) weak_by_id: HashMap<Uuid, Weak<TracedMutex<LibraryBook>>>,
    pub(super) warm_lru: VecDeque<(Uuid, Arc<TracedMutex<LibraryBook>>)>,
    capacity: usize,
}

impl BooksCache {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            weak_by_id: HashMap::new(),
            warm_lru: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub(crate) fn get(&self, uuid: &Uuid) -> Option<Arc<TracedMutex<LibraryBook>>> {
        self.weak_by_id.get(uuid).and_then(Weak::upgrade)
    }

    pub(crate) fn insert(&mut self, uuid: Uuid, book: Arc<TracedMutex<LibraryBook>>) {
        self.weak_by_id.retain(|_, w| w.strong_count() > 0);

        if self.warm_lru.len() >= self.capacity {
            self.warm_lru.pop_front();
        }
        self.warm_lru.push_back((uuid, book.clone()));
        self.weak_by_id.insert(uuid, Arc::downgrade(&book));
    }

    pub(crate) fn remove(&mut self, uuid: &Uuid) {
        self.weak_by_id.remove(uuid);
        self.warm_lru.retain(|(u, _)| u != uuid);
    }

    pub(crate) fn live_books(&self) -> Vec<Arc<TracedMutex<LibraryBook>>> {
        self.weak_by_id
            .values()
            .filter_map(Weak::upgrade)
            .collect()
    }
}
