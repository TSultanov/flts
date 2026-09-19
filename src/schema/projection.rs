//! Derived row visibility shared by the tables of a store.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use super::{Collision, Conflict, Dot, ReferenceConflict, RowStatus, Suppression};
use crate::{Error, Result, Uuid};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Claim {
    pub column: &'static str,
    pub value: Vec<u8>,
}

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
    pub dot: Dot,
    pub pending: Option<usize>,
    pub deleted: bool,
    pub claims: Vec<Claim>,
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
                "recover the uncertain commit before changing this store",
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
        let mut candidates: Vec<_> = held.nodes.values().collect();
        candidates.sort_by_key(|node| {
            std::cmp::Reverse((node.pending.is_some(), node.pending, node.dot, node.id))
        });
        let mut claimed = BTreeMap::<Claim, Uuid>::new();
        for node in candidates {
            let key = (node.entity, node.id);
            if node.deleted {
                held.statuses.insert(key, RowStatus::Deleted);
                continue;
            }
            let reasons: Vec<_> = node
                .claims
                .iter()
                .filter_map(|claim| {
                    claimed.get(claim).map(|kept| {
                        Suppression::Collision(Collision {
                            column: claim.column,
                            key: claim.value.clone(),
                            kept: *kept,
                            dropped: node.id,
                        })
                    })
                })
                .collect();
            if reasons.is_empty() {
                for claim in &node.claims {
                    claimed.insert(claim.clone(), node.id);
                }
                held.statuses.insert(key, RowStatus::Visible);
            } else {
                held.statuses.insert(key, RowStatus::Suppressed(reasons));
            }
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
                        let reason = Suppression::Reference(ReferenceConflict {
                            entity: node.entity,
                            id: node.id,
                            column: parent.column,
                            parent_entity: parent.entity,
                            parent: parent.id,
                        });
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
                RowStatus::Suppressed(reasons) => reasons
                    .into_iter()
                    .map(|reason| match reason {
                        Suppression::Collision(c) => Conflict::Collision(c),
                        Suppression::Reference(r) => Conflict::Reference(r),
                    })
                    .collect(),
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
