//! A child's parent, and the index that answers the parent's question about its children.

use std::marker::PhantomData;

use super::column::{Column, ColumnKind, ColumnSpec, Residency, Row};
use super::entity::Entity;
use super::identity::IdColumn;
use super::index::HashIndex;
use super::names::qualified;
use crate::error::{Error, Result};
use crate::id::Id;
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater};

pub struct RefColumn<P: Entity, C: Entity> {
    ids: Arena<Id<P>>,
    index: HashIndex<Id<P>>,
    name: String,
    marker: PhantomData<fn() -> C>,
}

impl<P: Entity, C: Entity> Clone for RefColumn<P, C> {
    fn clone(&self) -> Self {
        Self {
            ids: self.ids.clone(),
            index: self.index.clone(),
            name: self.name.clone(),
            marker: PhantomData,
        }
    }
}

impl<P: Entity, C: Entity> RefColumn<P, C> {
    pub fn named(column: &str) -> Self {
        Self {
            ids: Arena::new(),
            index: HashIndex::new(column.to_owned()),
            name: column.to_owned(),
            marker: PhantomData,
        }
    }

    pub fn reach(&self, parent: Id<P>) -> Result<()> {
        self.index.reach_for(parent)
    }

    pub fn push(&mut self, parent: Id<P>) -> Result<usize> {
        self.index.append(parent)?;
        self.ids.push(parent)
    }

    pub fn get(&self, row: Row) -> Result<Id<P>> {
        self.ids.row(row.index()).copied()
    }

    pub fn set(&mut self, row: Row, parent: Id<P>) -> Result<()> {
        self.index.repoint_row(row.index(), parent)?;
        self.ids.set(row.index(), parent)
    }

    pub fn child_rows_of(&self, parent: Id<P>) -> Result<Vec<Row>> {
        self.index
            .rows_of(parent)?
            .into_iter()
            .map(|row| Row::of(row, self.ids.len(), C::NAME))
            .collect()
    }

    pub fn children_of(&self, mine: &IdColumn<C>, parent: Id<P>) -> Result<Vec<Id<C>>> {
        self.child_rows_of(parent)?
            .into_iter()
            .map(|row| mine.at(row))
            .collect()
    }

    pub fn orphans(&self, mine: &IdColumn<C>) -> Result<Vec<Id<C>>> {
        self.children_of(mine, Id::NIL)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn check_parents(&self, parents: &IdColumn<P>) -> Result<()> {
        for child in 0..self.ids.len() {
            let parent = *self.ids.row(child)?;
            if !parent.is_nil() && parents.find(parent)?.is_none() {
                return Err(Error::corrupt(
                    "reference",
                    format!(
                        "{} row {child} names parent {parent}, which is not a row of {}",
                        self.name,
                        P::NAME
                    ),
                ));
            }
        }
        Ok(())
    }
}

impl<P: Entity, C: Entity> Column for RefColumn<P, C> {
    fn value_bytes(&self, row: Row) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        super::encode_record(&self.get(row)?, &mut out);
        Ok(out)
    }

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
        self.index.audit(&self.ids)
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: qualified(entity, field),
            elem_size: <Id<P> as Record>::SIZE,
            kind: ColumnKind::Ref { parent: P::NAME },
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
#[path = "reference_test.rs"]
mod reference_test;
