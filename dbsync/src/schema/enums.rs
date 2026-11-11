//! A unit enum stored as a discriminant, decoded only into a variant it names.

use std::marker::PhantomData;

use super::column::{Column, ColumnKind, ColumnSpec, Residency, Row};
use super::index::Keys;
use super::indexed::Keyed;
use super::names::qualified;
use crate::error::Result;
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater};

pub trait UnitEnum: Copy + 'static {
    const NAME: &'static str;
    const VARIANTS: &'static [(&'static str, u8)];

    fn to_u8(self) -> u8;

    fn from_u8(code: u8) -> Result<Self>;
}

pub fn enum_type<E: UnitEnum>() -> String {
    let variants: Vec<String> = E::VARIANTS
        .iter()
        .map(|(name, code)| format!("{name}={code}"))
        .collect();
    format!("enum {}|{}", E::NAME, variants.join(","))
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
    fn append_value_bytes(&mut self, bytes: &[u8]) -> Result<usize> {
        let [code] = bytes else {
            return Err(crate::Error::corrupt("enum", "invalid enum width"));
        };
        self.push(E::from_u8(*code)?)
    }

    fn value_bytes(&self, row: Row) -> Result<Vec<u8>> {
        Ok(vec![self.get(row)?.to_u8()])
    }

    fn understands(&self, bytes: &[u8]) -> bool {
        matches!(bytes, [code] if E::from_u8(*code).is_ok())
    }

    fn blank_value_bytes(&self) -> Option<Vec<u8>> {
        E::VARIANTS.first().map(|(_, code)| vec![*code])
    }

    fn schema_type(&self) -> String {
        enum_type::<E>()
    }

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

    fn rebase(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        file.rebase(&qualified(entity, field), &mut self.codes)
    }
}

impl<E: UnitEnum> Keys for EnumColumn<E> {
    type Key = u8;

    fn key_at(&self, row: usize) -> Result<u8> {
        self.codes.key_at(row)
    }
}

impl<E: UnitEnum> Keyed for EnumColumn<E> {
    type Seek<'a> = E;

    fn key_for(value: E) -> u8 {
        value.to_u8()
    }

    fn append(&mut self, value: E, _shared: Option<u8>) -> Result<u8> {
        self.codes.push(value.to_u8())?;
        Ok(value.to_u8())
    }

    fn write(&mut self, row: Row, value: E, _shared: Option<u8>) -> Result<u8> {
        self.codes.set(row.index(), value.to_u8())?;
        Ok(value.to_u8())
    }
}

#[cfg(test)]
#[path = "enums_test.rs"]
mod enums_test;
