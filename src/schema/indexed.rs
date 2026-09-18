//! A column with an index over the values it holds, so a row can be found by one.

use super::column::{Column, ColumnSpec, Residency, Row};
use super::index::IndexKey;
use super::index::{HashIndex, Keys};
use crate::error::{Error, Result};
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater};

pub trait Keyed: Column + Keys + Default + Clone {
    type Seek<'a>: Copy;

    fn key_for(value: Self::Seek<'_>) -> Self::Key;

    fn absent(_value: Self::Seek<'_>) -> bool {
        false
    }

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

pub type Unique<C> = Indexed<C, true>;

pub struct Indexed<C: Keyed, const UNIQUE: bool> {
    column: C,
    index: HashIndex<C::Key>,
    name: String,
}

impl<C: Keyed, const UNIQUE: bool> Clone for Indexed<C, UNIQUE> {
    fn clone(&self) -> Self {
        Self {
            column: self.column.clone(),
            index: self.index.clone(),
            name: self.name.clone(),
        }
    }
}

impl<C: Keyed, const UNIQUE: bool> Indexed<C, UNIQUE> {
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
        let claimed = match UNIQUE && !C::absent(value) {
            true => self.one(value)?.filter(|held| *held != row),
            false => None,
        };
        if let Some(held) = claimed {
            return Err(self.taken(held));
        }
        let shared = self.already_here(value)?;
        let key = self.column.write(row, value, shared)?;
        self.index.repoint_row(row.index(), key)
    }

    pub fn rows_holding(&self, value: C::Seek<'_>) -> Result<Vec<Row>> {
        match C::absent(value) {
            true => Ok(Vec::new()),
            false => self.find(value),
        }
    }

    pub fn taken_by(&self, held: Row) -> Error {
        self.taken(held)
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

    fn one(&self, value: C::Seek<'_>) -> Result<Option<Row>> {
        match self.index.row_where(C::key_for(value), |row, key| {
            self.really_holds(row, key, value)
        })? {
            Some(row) => self.at(row).map(Some),
            None => Ok(None),
        }
    }

    fn really_holds(&self, row: usize, key: C::Key, value: C::Seek<'_>) -> Result<bool> {
        self.at(row)?;
        self.column.holds(key, value)
    }

    fn at(&self, row: usize) -> Result<Row> {
        Row::of(row, Column::rows(&self.column), &self.name)
    }

    fn taken(&self, held: Row) -> Error {
        Error::invalid(format!(
            "{} already holds row {}, and no two rows may share one value",
            self.name,
            held.index()
        ))
    }
}

impl<C: Keyed> Indexed<C, true> {
    pub fn find_one(&self, value: C::Seek<'_>) -> Result<Option<Row>> {
        match C::absent(value) {
            true => Ok(None),
            false => self.one(value),
        }
    }

    pub fn refuse_duplicate(&self, value: C::Seek<'_>) -> Result<()> {
        if C::absent(value) {
            return Ok(());
        }
        match self.one(value)? {
            Some(held) => Err(self.taken(held)),
            None => Ok(()),
        }
    }
}

impl<C: Keyed, const UNIQUE: bool> Column for Indexed<C, UNIQUE> {
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
}

#[cfg(test)]
#[path = "indexed_test.rs"]
mod indexed_test;
