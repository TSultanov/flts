//! A column with an index over the values it holds, so a row can be found by one.

use super::column::{Column, ColumnSpec, Residency, Row};
use super::index::IndexKey;
use super::index::{HashIndex, Keys};
use crate::error::Result;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater};

pub trait Keyed: Column + Keys + Default + Clone {
    type Seek<'a>: Copy;

    fn key_for(value: Self::Seek<'_>) -> Self::Key;

    fn shares_bytes(_key: Self::Key) -> bool {
        false
    }

    fn append(&mut self, value: Self::Seek<'_>, shared: Option<Self::Key>) -> Result<Self::Key>;

    fn write(
        &mut self,
        row: Row,
        value: Self::Seek<'_>,
        shared: Option<Self::Key>,
    ) -> Result<Self::Key>;

    fn holds(&self, _key: Self::Key, _value: Self::Seek<'_>) -> Result<bool> {
        Ok(true)
    }
}

impl<T: IndexKey> Keyed for Arena<T> {
    type Seek<'a> = T;

    fn key_for(value: T) -> T {
        value
    }

    fn append(&mut self, value: T, _shared: Option<T>) -> Result<T> {
        self.push(value)?;
        Ok(value)
    }

    fn write(&mut self, row: Row, value: T, _shared: Option<T>) -> Result<T> {
        self.set(row.index(), value)?;
        Ok(value)
    }
}

pub struct Indexed<C: Keyed> {
    column: C,
    index: HashIndex<C::Key>,
    name: String,
}

impl<C: Keyed> Clone for Indexed<C> {
    fn clone(&self) -> Self {
        Self {
            column: self.column.clone(),
            index: self.index.clone(),
            name: self.name.clone(),
        }
    }
}

impl<C: Keyed> Indexed<C> {
    pub fn named(column: &str) -> Self {
        Self {
            column: C::default(),
            index: HashIndex::new(column.to_owned()),
            name: column.to_owned(),
        }
    }

    pub fn column(&self) -> &C {
        &self.column
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn reach(&self, value: C::Seek<'_>) -> Result<()> {
        self.index.reach_for(C::key_for(value))
    }

    pub fn push(&mut self, value: C::Seek<'_>) -> Result<usize> {
        let shared = self.already_here(value)?;
        let key = self.column.append(value, shared)?;
        self.index.append(key)
    }

    pub fn set(&mut self, row: Row, value: C::Seek<'_>) -> Result<()> {
        let shared = self.already_here(value)?;
        let key = self.column.write(row, value, shared)?;
        self.index.repoint_row(row.index(), key)
    }

    pub fn find(&self, value: C::Seek<'_>) -> Result<Vec<Row>> {
        self.index
            .rows_where(C::key_for(value), |row, key| {
                self.really_holds(row, key, value)
            })?
            .into_iter()
            .map(|row| self.at(row))
            .collect()
    }

    fn already_here(&self, value: C::Seek<'_>) -> Result<Option<C::Key>> {
        let sought = C::key_for(value);
        if !C::shares_bytes(sought) {
            return Ok(None);
        }

        let mut shared = None;
        self.index.row_where(sought, |row, key| {
            match self.really_holds(row, key, value)? {
                true => {
                    shared = Some(key);
                    Ok(true)
                }
                false => Ok(false),
            }
        })?;
        Ok(shared)
    }

    fn really_holds(&self, row: usize, key: C::Key, value: C::Seek<'_>) -> Result<bool> {
        self.at(row)?;
        self.column.holds(key, value)
    }

    fn at(&self, row: usize) -> Result<Row> {
        Row::of(row, Column::rows(&self.column), &self.name)
    }
}

impl<C: Keyed> Column for Indexed<C> {
    fn unused_heap_bytes(&self) -> Result<u64> {
        self.column.unused_heap_bytes()
    }
    fn compact_storage(&mut self) -> Result<bool> {
        if self.column.compact_storage()? {
            self.index.rebuild(&self.column)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn append_value_bytes(&mut self, bytes: &[u8]) -> Result<usize> {
        let row = self.column.append_value_bytes(bytes)?;
        self.index.append(self.column.key_at(row)?)
    }

    fn value_bytes(&self, row: Row) -> Result<Vec<u8>> {
        self.column.value_bytes(row)
    }

    fn schema_type(&self) -> String {
        self.column.schema_type()
    }

    fn rows(&self) -> usize {
        Column::rows(&self.column)
    }

    fn touch_tail(&self) -> Result<()> {
        self.column.touch_tail()?;
        self.index.touch_tail()
    }

    fn make_room(&self, rows: usize) -> Result<()> {
        self.column.make_room(rows)?;
        self.index.make_room(rows)
    }

    fn audit(&self) -> Result<()> {
        self.index.audit(&self.column)
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        self.column.describe(entity, field, out);
        self.index.describe(entity, field, out);
    }

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        self.column.residency(entity, field, out);
        self.index.residency(entity, field, out);
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.column.read_from(entity, field, file)?;
        self.index.read_from(entity, field, file)?;
        if !self.index.agrees_with(Column::rows(&self.column)) {
            self.index.rebuild(&self.column)?;
        }
        Ok(())
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        self.column.offer_to(entity, field, updater)?;
        self.index.offer_to(entity, field, updater)
    }

    fn rebase(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.column.rebase(entity, field, file)?;
        self.index.rebase(entity, field, file)
    }
}

#[cfg(test)]
#[path = "indexed_test.rs"]
mod indexed_test;
