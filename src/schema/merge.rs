//! What a merge of two copies did, and what it had to decide for itself.

use super::version::Dot;
use crate::id::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Concurrent {
    pub entity: &'static str,
    pub id: Uuid,
    pub kept: Dot,
    pub dropped: Dot,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Collision {
    pub column: &'static str,
    pub kept: Uuid,
    pub dropped: Uuid,
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct MergeReport {
    pub taken: usize,
    pub known: usize,
    pub concurrent: Vec<Concurrent>,
    pub collided: Vec<Collision>,
}

impl MergeReport {
    pub fn is_quiet(&self) -> bool {
        self.concurrent.is_empty() && self.collided.is_empty()
    }

    pub fn changed_anything(&self) -> bool {
        self.taken > 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Taking {
    Known,
    Fresh,
    Later,
    Concurrent { kept: Dot, dropped: Dot },
}

#[cfg(test)]
#[path = "merge_test.rs"]
mod merge_test;
