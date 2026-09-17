//! A field that may be absent, kept honest by a presence column beside it.

use super::column::{Column, ColumnKind, ColumnSpec, Row};
use super::names::{presence_name, qualified};
use crate::error::Result;
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater};

#[derive(Clone, Default)]
pub struct NullableColumn<T: Record + Default> {
    values: Arena<T>,
    present: Arena<bool>,
}

impl<T: Record + Default> NullableColumn<T> {
    pub fn push(&mut self, value: Option<T>) -> Result<usize> {
        let row = self.values.push(value.unwrap_or_default())?;
        self.present.push(value.is_some())?;
        Ok(row)
    }

    pub fn get(&self, row: Row) -> Result<Option<T>> {
        if !*self.present.row(row.index())? {
            return Ok(None);
        }
        self.values.row(row.index()).copied().map(Some)
    }

    pub fn set(&mut self, row: Row, value: Option<T>) -> Result<()> {
        self.values.set(row.index(), value.unwrap_or_default())?;
        self.present.set(row.index(), value.is_some())
    }
}

impl<T: Record + Default> Column for NullableColumn<T> {
    fn rows(&self) -> usize {
        self.values.len()
    }

    fn touch_tail(&self) -> Result<()> {
        self.values.touch_tail()?;
        self.present.touch_tail()
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: qualified(entity, field),
            elem_size: T::SIZE,
            kind: ColumnKind::Nullable(Box::new(ColumnKind::Scalar)),
            counts_rows: true,
        });
        out.push(ColumnSpec {
            name: presence_name(entity, field),
            elem_size: <bool as Record>::SIZE,
            kind: ColumnKind::Presence,
            counts_rows: true,
        });
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.values = file.column(&qualified(entity, field))?;
        self.present = file.column(&presence_name(entity, field))?;
        Ok(())
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        updater.column(&qualified(entity, field), &self.values)?;
        updater.column(&presence_name(entity, field), &self.present)
    }
}

#[cfg(test)]
#[path = "nullable_test.rs"]
mod nullable_test;
