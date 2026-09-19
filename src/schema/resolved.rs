//! Whole-row merge inputs and cached values derived from immutable revisions.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use super::{Dot, Version};
use crate::Uuid;

#[derive(Clone, Debug)]
pub struct MergeCandidate<V> {
    pub version: Version,
    pub value: V,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RevisionAncestry {
    pub dot: Dot,
    pub parents: Vec<Dot>,
}

pub struct MergeInput<'a, V> {
    pub id: Uuid,
    pub candidates: &'a [MergeCandidate<V>],
    pub ancestry: &'a [RevisionAncestry],
}

#[derive(Clone)]
struct Cached<V> {
    sources: Vec<Dot>,
    value: Arc<V>,
}

pub struct Resolved<V> {
    values: RwLock<BTreeMap<Uuid, Cached<V>>>,
}

impl<V> Default for Resolved<V> {
    fn default() -> Self {
        Self {
            values: RwLock::new(BTreeMap::new()),
        }
    }
}

impl<V: Clone> Clone for Resolved<V> {
    fn clone(&self) -> Self {
        Self {
            values: RwLock::new(self.values.read().expect("resolved values lock").clone()),
        }
    }
}

impl<V> Resolved<V> {
    pub fn get(&self, id: Uuid) -> Option<Arc<V>> {
        self.values
            .read()
            .expect("resolved values lock")
            .get(&id)
            .map(|cached| cached.value.clone())
    }

    pub fn sources(&self, id: Uuid) -> Option<Vec<Dot>> {
        self.values
            .read()
            .expect("resolved values lock")
            .get(&id)
            .map(|cached| cached.sources.clone())
    }

    pub fn insert(&self, id: Uuid, sources: Vec<Dot>, value: V) {
        self.values.write().expect("resolved values lock").insert(
            id,
            Cached {
                sources,
                value: Arc::new(value),
            },
        );
    }

    pub fn remove(&self, id: Uuid) {
        self.values
            .write()
            .expect("resolved values lock")
            .remove(&id);
    }

    pub fn clear(&self) {
        self.values.write().expect("resolved values lock").clear();
    }

    pub fn entries(&self) -> Vec<(Uuid, Arc<V>)> {
        self.values
            .read()
            .expect("resolved values lock")
            .iter()
            .map(|(id, cached)| (*id, cached.value.clone()))
            .collect()
    }
}
