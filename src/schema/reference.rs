//! A child's parent, and the cached grouping that answers the parent's question.

use std::sync::{Mutex, OnceLock, PoisonError};

use super::child_index::ChildIndex;
use super::column::{Column, ColumnKind, ColumnSpec, Row};
use super::entity::Entity;
use super::names::qualified;
use crate::error::Result;
use crate::id::Id;
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater};

pub struct RefColumn<P: Entity, C: Entity> {
    ids: Arena<Id<P>>,
    index: OnceLock<ChildIndex<C>>,
    building: Mutex<()>,
}

impl<P: Entity, C: Entity> Default for RefColumn<P, C> {
    fn default() -> Self {
        Self {
            ids: Arena::new(),
            index: OnceLock::new(),
            building: Mutex::new(()),
        }
    }
}

impl<P: Entity, C: Entity> Clone for RefColumn<P, C> {
    fn clone(&self) -> Self {
        Self {
            ids: self.ids.clone(),
            index: self.index.clone(),
            building: Mutex::new(()),
        }
    }
}

impl<P: Entity, C: Entity> RefColumn<P, C> {
    pub fn push(&mut self, parent: Id<P>) -> Result<usize> {
        self.index.take();
        self.ids.push(parent)
    }

    pub fn get(&self, row: Row) -> Result<Id<P>> {
        self.ids.row(row.index()).copied()
    }

    pub fn set(&mut self, row: Row, parent: Id<P>) -> Result<()> {
        self.index.take();
        self.ids.set(row.index(), parent)
    }

    pub fn is_indexed(&self) -> bool {
        self.index.get().is_some()
    }

    pub fn children_of(&self, parent_rows: usize, parent: Id<P>) -> Result<&[Id<C>]> {
        Ok(self.grouping(parent_rows)?.children_of(parent))
    }

    pub fn orphans(&self, parent_rows: usize) -> Result<&[Id<C>]> {
        Ok(self.grouping(parent_rows)?.orphans())
    }

    fn grouping(&self, parent_rows: usize) -> Result<&ChildIndex<C>> {
        if let Some(index) = self.index.get() {
            return Ok(index);
        }

        let _building = self.building.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(index) = self.index.get() {
            return Ok(index);
        }

        let built = ChildIndex::build(&self.ids, parent_rows, &qualified(C::NAME, P::NAME))?;
        Ok(self.index.get_or_init(|| built))
    }
}

impl<P: Entity, C: Entity> Column for RefColumn<P, C> {
    fn rows(&self) -> usize {
        self.ids.len()
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: qualified(entity, field),
            elem_size: <Id<P> as Record>::SIZE,
            kind: ColumnKind::Ref { parent: P::NAME },
            counts_rows: true,
        });
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.index.take();
        self.ids = file.column(&qualified(entity, field))?;
        Ok(())
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        updater.column(&qualified(entity, field), &self.ids)
    }
}

#[cfg(test)]
#[path = "reference_test.rs"]
mod reference_test;
