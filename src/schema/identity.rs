//! The column every entity has: the identity of each row, and the way back from one to its row.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use super::column::{Column, ColumnKind, ColumnSpec, Residency, Row};
use super::entity::Entity;
use super::names::qualified;
use crate::error::{Error, Result};
use crate::id::Id;
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater};

pub struct IdColumn<E: Entity> {
    ids: Arena<Id<E>>,
    rows: OnceLock<HashMap<Id<E>, Row>>,
    building: Mutex<()>,
}

impl<E: Entity> Default for IdColumn<E> {
    fn default() -> Self {
        Self {
            ids: Arena::new(),
            rows: OnceLock::new(),
            building: Mutex::new(()),
        }
    }
}

impl<E: Entity> Clone for IdColumn<E> {
    fn clone(&self) -> Self {
        Self {
            ids: self.ids.clone(),
            rows: self.rows.clone(),
            building: Mutex::new(()),
        }
    }
}

impl<E: Entity> IdColumn<E> {
    pub fn push(&mut self, id: Id<E>) -> Result<usize> {
        self.rows.take();
        self.ids.push(id)
    }

    pub fn at(&self, row: Row) -> Result<Id<E>> {
        self.ids.row(row.index()).copied()
    }

    pub fn find(&self, id: Id<E>) -> Result<Option<Row>> {
        Ok(self.grouping()?.get(&id).copied())
    }

    pub fn row_of(&self, id: Id<E>) -> Result<Row> {
        self.find(id)?
            .ok_or_else(|| Error::invalid(format!("{} has no row with id {id}", E::NAME)))
    }

    fn grouping(&self) -> Result<&HashMap<Id<E>, Row>> {
        if let Some(rows) = self.rows.get() {
            return Ok(rows);
        }

        let _building = self.building.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(rows) = self.rows.get() {
            return Ok(rows);
        }

        let built = self.build()?;
        Ok(self.rows.get_or_init(|| built))
    }

    fn build(&self) -> Result<HashMap<Id<E>, Row>> {
        let column = qualified(E::NAME, "id");
        let mut rows = HashMap::with_capacity(self.ids.len());

        for index in 0..self.ids.len() {
            let id = *self.ids.row(index)?;
            if id.is_nil() {
                return Err(Error::corrupt(
                    "identity",
                    format!("{column} row {index} has no id"),
                ));
            }
            if let Some(first) = rows.insert(id, Row::trusted(index)) {
                return Err(Error::corrupt(
                    "identity",
                    format!("{column} rows {} and {index} are both {id}", first.index()),
                ));
            }
        }
        Ok(rows)
    }
}

impl<E: Entity> Column for IdColumn<E> {
    fn rows(&self) -> usize {
        self.ids.len()
    }

    fn touch_tail(&self) -> Result<()> {
        self.ids.touch_tail()
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: qualified(entity, field),
            elem_size: <Id<E> as Record>::SIZE,
            kind: ColumnKind::Identity,
            counts_rows: true,
        });
    }

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        self.ids.residency(entity, field, out);
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.rows.take();
        self.ids = file.column(&qualified(entity, field))?;
        Ok(())
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        updater.column(&qualified(entity, field), &self.ids)
    }
}

#[cfg(test)]
#[path = "identity_test.rs"]
mod identity_test;
