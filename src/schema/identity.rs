//! The column every entity has: the identity of each row, and the index that finds a row from one.

use super::column::{Column, ColumnKind, ColumnSpec, Residency, Row};
use super::entity::Entity;
use super::index::HashIndex;
use super::names::qualified;
use crate::error::{Error, Result};
use crate::id::Id;
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater};

pub struct IdColumn<E: Entity> {
    ids: Arena<Id<E>>,
    index: HashIndex<Id<E>>,
}

impl<E: Entity> Default for IdColumn<E> {
    fn default() -> Self {
        Self {
            ids: Arena::new(),
            index: HashIndex::new(qualified(E::NAME, "id")),
        }
    }
}

impl<E: Entity> Clone for IdColumn<E> {
    fn clone(&self) -> Self {
        Self {
            ids: self.ids.clone(),
            index: self.index.clone(),
        }
    }
}

impl<E: Entity> IdColumn<E> {
    pub fn push(&mut self, id: Id<E>) -> Result<usize> {
        if id.is_nil() {
            return Err(Error::invalid(format!(
                "a {} row cannot be given no id at all",
                E::NAME
            )));
        }

        self.index.append(id)?;
        self.ids.push(id)
    }

    pub fn at(&self, row: Row) -> Result<Id<E>> {
        self.ids.row(row.index()).copied()
    }

    pub fn find(&self, id: Id<E>) -> Result<Option<Row>> {
        match self.index.row_of(id)? {
            Some(row) => Ok(Some(Row::of(row, self.ids.len(), E::NAME)?)),
            None => Ok(None),
        }
    }

    pub fn row_of(&self, id: Id<E>) -> Result<Row> {
        self.find(id)?
            .ok_or_else(|| Error::invalid(format!("{} has no row with id {id}", E::NAME)))
    }

    pub fn rows_of(&self, id: Id<E>) -> Result<Vec<Row>> {
        self.index
            .rows_of(id)?
            .into_iter()
            .map(|row| Row::of(row, self.ids.len(), E::NAME))
            .collect()
    }
}

impl<E: Entity> Column for IdColumn<E> {
    fn rows(&self) -> usize {
        self.ids.len()
    }

    fn touch_tail(&self) -> Result<()> {
        self.ids.touch_tail()?;
        self.index.touch_tail()
    }

    fn make_room(&self, rows: usize) -> Result<()> {
        self.ids.touch_tail()?;
        self.index.make_room(rows)
    }

    fn audit(&self) -> Result<()> {
        let column = qualified(E::NAME, "id");
        self.index.audit(&self.ids)?;

        for index in 0..self.ids.len() {
            let id = *self.ids.row(index)?;
            if id.is_nil() {
                return Err(Error::corrupt(
                    "identity",
                    format!("{column} row {index} has no id"),
                ));
            }
        }
        Ok(())
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: qualified(entity, field),
            elem_size: <Id<E> as Record>::SIZE,
            kind: ColumnKind::Identity,
            counts_rows: true,
        });
        self.index.describe(entity, field, out);
    }

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        self.ids.residency(entity, field, out);
        self.index.residency(entity, field, out);
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.ids = file.column(&qualified(entity, field))?;
        self.index.read_from(entity, field, file)?;
        if !self.index.agrees_with(self.ids.len()) {
            self.index.rebuild(&self.ids)?;
        }
        Ok(())
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        updater.column(&qualified(entity, field), &self.ids)?;
        self.index.offer_to(entity, field, updater)
    }
}

#[cfg(test)]
#[path = "identity_test.rs"]
mod identity_test;
