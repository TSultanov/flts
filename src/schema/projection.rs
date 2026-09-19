//! Derived row visibility shared by the tables of a store.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use super::{Conflict, ReferenceConflict, RowStatus};
use crate::{Error, Result, Uuid};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Reference {
    pub column: &'static str,
    pub entity: &'static str,
    pub id: Uuid,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Candidate {
    pub entity: &'static str,
    pub id: Uuid,
    pub deleted: bool,
    pub references: Vec<Reference>,
}

#[derive(Clone, Default)]
struct Projection {
    entities: BTreeSet<&'static str>,
    nodes: BTreeMap<(&'static str, Uuid), Candidate>,
    statuses: BTreeMap<(&'static str, Uuid), RowStatus>,
    dirty: bool,
    blocked: bool,
}

#[derive(Clone, Default)]
pub struct View(Arc<RwLock<Projection>>);

impl View {
    pub fn register(&self, entity: &'static str) {
        self.0.write().expect("view lock").entities.insert(entity);
    }

    pub fn fork(&self) -> Self {
        Self(Arc::new(RwLock::new(
            self.0.read().expect("view lock").clone(),
        )))
    }

    pub fn check_mutable(&self) -> Result<()> {
        if self.0.read().expect("view lock").blocked {
            return Err(Error::invalid(
                "retry commit or merge_from to finish the pending publication before changing this store",
            ));
        }
        Ok(())
    }

    pub fn block(&self, blocked: bool) {
        self.0.write().expect("view lock").blocked = blocked;
    }

    pub fn replace(&self, entity: &'static str, candidates: Vec<Candidate>) {
        let mut held = self.0.write().expect("view lock");
        held.nodes.retain(|(name, _), _| *name != entity);
        for node in candidates {
            held.nodes.insert((entity, node.id), node);
        }
        held.dirty = true;
    }

    pub fn upsert(&self, candidate: Candidate) {
        let mut held = self.0.write().expect("view lock");
        held.nodes
            .insert((candidate.entity, candidate.id), candidate);
        held.dirty = true;
    }

    fn resolve(held: &mut Projection) {
        if !held.dirty {
            return;
        }
        held.statuses.clear();
        for node in held.nodes.values() {
            held.statuses.insert(
                (node.entity, node.id),
                if node.deleted {
                    RowStatus::Deleted
                } else {
                    RowStatus::Visible
                },
            );
        }
        loop {
            let hidden: BTreeSet<_> = held
                .statuses
                .iter()
                .filter_map(|(key, status)| (*status != RowStatus::Visible).then_some(*key))
                .collect();
            let mut changed = false;
            for (key, node) in &held.nodes {
                if node.deleted {
                    continue;
                }
                for parent in &node.references {
                    if parent.id.is_nil() {
                        continue;
                    }
                    if hidden.contains(&(parent.entity, parent.id))
                        || (held.entities.contains(parent.entity)
                            && !held.nodes.contains_key(&(parent.entity, parent.id)))
                    {
                        let reason = ReferenceConflict {
                            entity: node.entity,
                            id: node.id,
                            column: parent.column,
                            parent_entity: parent.entity,
                            parent: parent.id,
                        };
                        let status = held.statuses.get_mut(key).expect("candidate status");
                        if *status == RowStatus::Visible {
                            *status = RowStatus::Suppressed(Vec::new());
                            changed = true;
                        }
                        if let RowStatus::Suppressed(reasons) = status
                            && !reasons.contains(&reason)
                        {
                            reasons.push(reason);
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for status in held.statuses.values_mut() {
            if let RowStatus::Suppressed(reasons) = status {
                reasons.sort();
                reasons.dedup();
            }
        }
        held.dirty = false;
    }

    pub fn status(&self, entity: &'static str, id: Uuid) -> Option<RowStatus> {
        let mut held = self.0.write().expect("view lock");
        Self::resolve(&mut held);
        held.statuses.get(&(entity, id)).cloned()
    }

    pub fn statuses(&self) -> BTreeMap<(&'static str, Uuid), RowStatus> {
        let mut held = self.0.write().expect("view lock");
        Self::resolve(&mut held);
        held.statuses.clone()
    }

    pub fn conflicts(&self) -> Vec<Conflict> {
        self.statuses()
            .into_values()
            .flat_map(|status| match status {
                RowStatus::Suppressed(reasons) => {
                    reasons.into_iter().map(Conflict::Reference).collect()
                }
                _ => Vec::new(),
            })
            .collect()
    }
}

pub fn encode_record<T: crate::Record>(value: &T, out: &mut Vec<u8>) {
    let at = out.len();
    out.resize(at + T::SIZE, 0);
    value.encode(&mut out[at..]);
}

pub fn encode_text(value: &str, out: &mut Vec<u8>) {
    encode_record(&(value.len() as u64), out);
    out.extend_from_slice(value.as_bytes());
}
