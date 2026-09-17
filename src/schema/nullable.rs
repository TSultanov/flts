//! A field that may be absent, kept honest by a presence column beside it.

use super::column::{Column, ColumnKind, ColumnSpec, Residency, Row};
use super::index::{IndexKey, Keys};
use super::indexed::Keyed;
use super::names::{presence_name, qualified};
use crate::error::{Error, Result};
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Maybe<T>(pub Option<T>);

impl<T: Record + Default> Record for Maybe<T> {
    const SIZE: usize = T::SIZE + 1;

    fn encode(&self, out: &mut [u8]) {
        out[0] = u8::from(self.0.is_some());
        self.0.unwrap_or_default().encode(&mut out[1..]);
    }

    fn decode(src: &[u8]) -> Self {
        Self((src[0] != 0).then(|| T::decode(&src[1..])))
    }
}

impl<T: IndexKey + Default> IndexKey for Maybe<T> {
    fn raw(self) -> u128 {
        self.0.map_or(u128::MAX, IndexKey::raw)
    }
}

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

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        self.values.residency(entity, field, out);
        out.push(Residency {
            name: presence_name(entity, field),
            resident: self.present.resident_chunks(),
            chunks: self.present.chunk_count(),
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

impl<T: IndexKey + Default> Keys for NullableColumn<T> {
    type Key = Maybe<T>;

    fn key_at(&self, row: usize) -> Result<Maybe<T>> {
        match self.present.get(row)?.copied() {
            Some(true) => Ok(Maybe(Some(self.values.key_at(row)?))),
            Some(false) => Ok(Maybe(None)),
            None => Err(Error::invalid(format!(
                "a column of {} keys has no row {row}",
                self.values.len()
            ))),
        }
    }
}

impl<T: IndexKey + Default> Keyed for NullableColumn<T> {
    type Seek<'a> = Option<T>;

    fn key_for(value: Option<T>) -> Maybe<T> {
        Maybe(value)
    }

    fn absent(value: Option<T>) -> bool {
        value.is_none()
    }

    fn append(&mut self, value: Option<T>) -> Result<Maybe<T>> {
        self.push(value)?;
        Ok(Maybe(value))
    }

    fn write(&mut self, row: Row, value: Option<T>) -> Result<Maybe<T>> {
        self.set(row, value)?;
        Ok(Maybe(value))
    }
}

#[cfg(test)]
#[path = "nullable_test.rs"]
mod nullable_test;
