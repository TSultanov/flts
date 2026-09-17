//! A string field: blob offsets in one column, the bytes they name in another.

use std::borrow::Cow;

use super::column::{Column, ColumnKind, ColumnSpec, Row};
use super::names::{heap_name, qualified};
use crate::error::Result;
use crate::record::Record;
use crate::storage::blob::Blob;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater};

#[derive(Clone, Default)]
pub struct TextColumn {
    blobs: Arena<Blob>,
    heap: Arena<u8>,
}

impl TextColumn {
    pub fn heap_len(&self) -> usize {
        self.heap.len()
    }

    pub fn push(&mut self, value: &str) -> Result<usize> {
        let blob = self.heap.push_str(value)?;
        self.blobs.push(blob)
    }

    pub fn get(&self, row: Row) -> Result<Cow<'_, str>> {
        self.heap.text(*self.blobs.row(row.index())?)
    }

    pub fn set(&mut self, row: Row, value: &str) -> Result<()> {
        let blob = self.heap.push_str(value)?;
        self.blobs.set(row.index(), blob)
    }
}

impl Column for TextColumn {
    fn rows(&self) -> usize {
        self.blobs.len()
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: qualified(entity, field),
            elem_size: <Blob as Record>::SIZE,
            kind: ColumnKind::Text,
            counts_rows: true,
        });
        out.push(ColumnSpec {
            name: heap_name(entity, field),
            elem_size: <u8 as Record>::SIZE,
            kind: ColumnKind::Heap,
            counts_rows: false,
        });
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.blobs = file.column(&qualified(entity, field))?;
        self.heap = file.column(&heap_name(entity, field))?;
        Ok(())
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        updater.column(&qualified(entity, field), &self.blobs)?;
        updater.column(&heap_name(entity, field), &self.heap)
    }
}

#[derive(Clone, Default)]
pub struct NullableTextColumn {
    blobs: Arena<Option<Blob>>,
    heap: Arena<u8>,
}

impl NullableTextColumn {
    pub fn heap_len(&self) -> usize {
        self.heap.len()
    }

    pub fn push(&mut self, value: Option<&str>) -> Result<usize> {
        let blob = self.store(value)?;
        self.blobs.push(blob)
    }

    pub fn get(&self, row: Row) -> Result<Option<Cow<'_, str>>> {
        match *self.blobs.row(row.index())? {
            Some(blob) => self.heap.text(blob).map(Some),
            None => Ok(None),
        }
    }

    pub fn set(&mut self, row: Row, value: Option<&str>) -> Result<()> {
        let blob = self.store(value)?;
        self.blobs.set(row.index(), blob)
    }

    fn store(&mut self, value: Option<&str>) -> Result<Option<Blob>> {
        match value {
            Some(text) => self.heap.push_str(text).map(Some),
            None => Ok(None),
        }
    }
}

impl Column for NullableTextColumn {
    fn rows(&self) -> usize {
        self.blobs.len()
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: qualified(entity, field),
            elem_size: <Option<Blob> as Record>::SIZE,
            kind: ColumnKind::Nullable(Box::new(ColumnKind::Text)),
            counts_rows: true,
        });
        out.push(ColumnSpec {
            name: heap_name(entity, field),
            elem_size: <u8 as Record>::SIZE,
            kind: ColumnKind::Heap,
            counts_rows: false,
        });
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.blobs = file.column(&qualified(entity, field))?;
        self.heap = file.column(&heap_name(entity, field))?;
        Ok(())
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        updater.column(&qualified(entity, field), &self.blobs)?;
        updater.column(&heap_name(entity, field), &self.heap)
    }
}

#[cfg(test)]
#[path = "text_test.rs"]
mod text_test;
