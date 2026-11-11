//! A parent or an enum that may be absent, held by the column that holds the present ones.

use std::marker::PhantomData;

use super::column::{Column, ColumnKind, ColumnSpec, Residency, Row};
use super::entity::Entity;
use super::enums::UnitEnum;
use super::names::{presence_name, qualified};
use super::nullable::NullableColumn;
use super::reference::RefColumn;
use crate::error::Result;
use crate::id::Id;
use crate::record::Record;
use crate::storage::{ColumnFile, ColumnFileUpdater};

pub struct OptionalRefColumn<P: Entity, C: Entity>(RefColumn<P, C>);

impl<P: Entity, C: Entity> Clone for OptionalRefColumn<P, C> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<P: Entity, C: Entity> OptionalRefColumn<P, C> {
    pub fn named(column: &str) -> Self {
        Self(RefColumn::named(column))
    }

    pub fn references(&self) -> &RefColumn<P, C> {
        &self.0
    }

    pub fn reach(&self, parent: Option<Id<P>>) -> Result<()> {
        self.0.reach(parent.unwrap_or(Id::NIL))
    }

    pub fn push(&mut self, parent: Option<Id<P>>) -> Result<usize> {
        self.0.push(parent.unwrap_or(Id::NIL))
    }

    pub fn get(&self, row: Row) -> Result<Option<Id<P>>> {
        let parent = self.0.get(row)?;
        Ok((!parent.is_nil()).then_some(parent))
    }

    pub fn set(&mut self, row: Row, parent: Option<Id<P>>) -> Result<()> {
        self.0.set(row, parent.unwrap_or(Id::NIL))
    }
}

impl<P: Entity, C: Entity> Column for OptionalRefColumn<P, C> {
    fn append_value_bytes(&mut self, bytes: &[u8]) -> Result<usize> {
        self.0.append_value_bytes(bytes)
    }

    fn value_bytes(&self, row: Row) -> Result<Vec<u8>> {
        self.0.value_bytes(row)
    }

    fn schema_type(&self) -> String {
        format!("optional reference {}", P::NAME)
    }

    fn rows(&self) -> usize {
        self.0.rows()
    }

    fn touch_tail(&self) -> Result<()> {
        self.0.touch_tail()
    }

    fn make_room(&self, rows: usize) -> Result<()> {
        self.0.make_room(rows)
    }

    fn audit(&self) -> Result<()> {
        self.0.audit()
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        let from = out.len();
        self.0.describe(entity, field, out);
        if let Some(spec) = out.get_mut(from) {
            spec.kind = ColumnKind::Nullable(Box::new(spec.kind.clone()));
        }
    }

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        self.0.residency(entity, field, out);
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.0.read_from(entity, field, file)
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        self.0.offer_to(entity, field, updater)
    }

    fn rebase(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.0.rebase(entity, field, file)
    }
}

#[derive(Clone)]
pub struct OptionalEnumColumn<E: UnitEnum> {
    codes: NullableColumn<u8>,
    marker: PhantomData<fn() -> E>,
}

impl<E: UnitEnum> Default for OptionalEnumColumn<E> {
    fn default() -> Self {
        Self {
            codes: NullableColumn::default(),
            marker: PhantomData,
        }
    }
}

impl<E: UnitEnum> OptionalEnumColumn<E> {
    pub fn push(&mut self, value: Option<E>) -> Result<usize> {
        self.codes.push(value.map(UnitEnum::to_u8))
    }

    pub fn get(&self, row: Row) -> Result<Option<E>> {
        self.codes.get(row)?.map(E::from_u8).transpose()
    }

    pub fn set(&mut self, row: Row, value: Option<E>) -> Result<()> {
        self.codes.set(row, value.map(UnitEnum::to_u8))
    }
}

impl<E: UnitEnum> Column for OptionalEnumColumn<E> {
    fn append_value_bytes(&mut self, bytes: &[u8]) -> Result<usize> {
        if let [1, code] = bytes {
            E::from_u8(*code)?;
        }
        self.codes.append_value_bytes(bytes)
    }

    fn value_bytes(&self, row: Row) -> Result<Vec<u8>> {
        self.codes.value_bytes(row)
    }

    fn schema_type(&self) -> String {
        format!("optional enum {} {:?}", E::NAME, E::VARIANTS)
    }

    fn rows(&self) -> usize {
        self.codes.rows()
    }

    fn touch_tail(&self) -> Result<()> {
        self.codes.touch_tail()
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: qualified(entity, field),
            elem_size: <u8 as Record>::SIZE,
            kind: ColumnKind::Nullable(Box::new(ColumnKind::Enum { name: E::NAME })),
            counts_rows: true,
        });
        out.push(ColumnSpec {
            name: presence_name(entity, field),
            elem_size: <bool as Record>::SIZE,
            kind: ColumnKind::Presence,
            counts_rows: true,
        });
    }

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        self.codes.residency(entity, field, out);
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.codes.read_from(entity, field, file)
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        self.codes.offer_to(entity, field, updater)
    }

    fn rebase(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.codes.rebase(entity, field, file)
    }
}

#[cfg(test)]
#[path = "optional_test.rs"]
mod optional_test;
