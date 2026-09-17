//! A unit enum stored as a discriminant, decoded only into a variant it names.

use std::marker::PhantomData;

use super::column::{Column, ColumnKind, ColumnSpec, Residency, Row};
use super::names::qualified;
use crate::error::Result;
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater};

pub trait UnitEnum: Copy + 'static {
    const NAME: &'static str;

    fn to_u8(self) -> u8;

    fn from_u8(code: u8) -> Result<Self>;
}

#[derive(Clone)]
pub struct EnumColumn<E: UnitEnum> {
    codes: Arena<u8>,
    marker: PhantomData<fn() -> E>,
}

impl<E: UnitEnum> Default for EnumColumn<E> {
    fn default() -> Self {
        Self {
            codes: Arena::new(),
            marker: PhantomData,
        }
    }
}

impl<E: UnitEnum> EnumColumn<E> {
    pub fn push(&mut self, value: E) -> Result<usize> {
        self.codes.push(value.to_u8())
    }

    pub fn get(&self, row: Row) -> Result<E> {
        E::from_u8(*self.codes.row(row.index())?)
    }

    pub fn set(&mut self, row: Row, value: E) -> Result<()> {
        self.codes.set(row.index(), value.to_u8())
    }
}

impl<E: UnitEnum> Column for EnumColumn<E> {
    fn rows(&self) -> usize {
        self.codes.len()
    }

    fn touch_tail(&self) -> Result<()> {
        self.codes.touch_tail()
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: qualified(entity, field),
            elem_size: <u8 as Record>::SIZE,
            kind: ColumnKind::Enum { name: E::NAME },
            counts_rows: true,
        });
    }

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        self.codes.residency(entity, field, out);
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.codes = file.column(&qualified(entity, field))?;
        Ok(())
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        updater.column(&qualified(entity, field), &self.codes)
    }
}

#[cfg(test)]
#[path = "enums_test.rs"]
mod enums_test;
