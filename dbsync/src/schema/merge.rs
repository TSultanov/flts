//! What a merge of two copies did, and what it had to decide for itself.

use super::version::Dot;
use crate::id::Uuid;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RevisionRecord {
    pub entity: &'static str,
    pub id: Uuid,
    pub version: super::Version,
    pub parents: Vec<Dot>,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Concurrent {
    pub entity: &'static str,
    pub id: Uuid,
    pub kept: Dot,
    pub dropped: Dot,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Combined {
    pub entity: &'static str,
    pub id: Uuid,
    pub sources: Vec<Dot>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ReferenceConflict {
    pub entity: &'static str,
    pub id: Uuid,
    pub column: &'static str,
    pub parent_entity: &'static str,
    pub parent: Uuid,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum RowStatus {
    Visible,
    Deleted,
    Suppressed(Vec<ReferenceConflict>),
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Conflict {
    Concurrent(Concurrent),
    Combined(Combined),
    Reference(ReferenceConflict),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VisibilityChange {
    pub entity: &'static str,
    pub id: Uuid,
    pub before: Option<RowStatus>,
    pub after: RowStatus,
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct MergeReport {
    pub taken: usize,
    pub known: usize,
    pub waiting: usize,
    pub concurrent: Vec<Concurrent>,
    pub combined: Vec<Combined>,
    pub references: Vec<ReferenceConflict>,
    pub visibility: Vec<VisibilityChange>,
    /// Rows that gained a revision, as (entity, id), sorted and distinct.
    pub adopted: Vec<(&'static str, Uuid)>,
    pub clock_changed: bool,
    pub unreadable: Vec<std::path::PathBuf>,
    pub incompatible: Vec<std::path::PathBuf>,
    pub strangers: Vec<std::path::PathBuf>,
}

impl MergeReport {
    pub fn is_quiet(&self) -> bool {
        self.concurrent.is_empty() && self.combined.is_empty() && self.references.is_empty()
    }

    pub fn changed_anything(&self) -> bool {
        self.taken > 0 || self.clock_changed || !self.visibility.is_empty()
    }
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct PruneReport {
    pub dropped: usize,
    pub waiting_for: Vec<crate::WriterId>,
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct PurgeReport {
    pub purged: usize,
    pub dropped: usize,
}

#[cfg(test)]
#[path = "merge_test.rs"]
mod merge_test;
