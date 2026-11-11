//! One field's storage, erased so a heterogeneous schema can be driven by a loop.

use super::names::qualified;
use crate::error::{Error, Result};
use crate::record::Record;
use crate::storage::arena::Arena;
use crate::storage::{ColumnFile, ColumnFileUpdater};

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ColumnKind {
    Identity,
    Scalar,
    Text,
    Bytes,
    Heap,
    Presence,
    Version,
    Clock,
    Bucket,
    Link,
    Sorted,
    Ref { parent: &'static str },
    Enum { name: &'static str },
    Nullable(Box<ColumnKind>),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Residency {
    pub name: String,
    pub resident: usize,
    pub chunks: usize,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ColumnSpec {
    pub name: String,
    pub elem_size: usize,
    pub kind: ColumnKind,
    pub counts_rows: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Row(usize);

impl Row {
    pub fn of(index: usize, rows: usize, entity: &str) -> Result<Self> {
        if index >= rows {
            return Err(Error::invalid(format!(
                "{entity} has no row {index}; it holds {rows}"
            )));
        }
        Ok(Row(index))
    }

    pub(crate) fn trusted(index: usize) -> Self {
        Row(index)
    }

    pub fn index(self) -> usize {
        self.0
    }
}

pub trait Column {
    fn unused_heap_bytes(&self) -> Result<u64> {
        Ok(0)
    }
    fn compact_storage(&mut self) -> Result<bool> {
        Ok(false)
    }

    fn append_value_bytes(&mut self, _bytes: &[u8]) -> Result<usize> {
        Err(Error::unsupported(
            "column does not accept logical row values",
        ))
    }

    fn value_bytes(&self, _row: Row) -> Result<Vec<u8>> {
        Err(Error::unsupported(
            "column does not expose its logical row value",
        ))
    }

    fn absent_value_bytes(&self) -> Option<Vec<u8>> {
        None
    }

    fn blank_value_bytes(&self) -> Option<Vec<u8>> {
        self.absent_value_bytes()
    }

    fn understands(&self, _bytes: &[u8]) -> bool {
        true
    }

    fn schema_type(&self) -> String;

    fn rows(&self) -> usize;

    fn touch_tail(&self) -> Result<()>;

    fn make_room(&self, _rows: usize) -> Result<()> {
        self.touch_tail()
    }

    fn audit(&self) -> Result<()> {
        Ok(())
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>);

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>);

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()>;

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()>;

    fn rebase(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()>;
}

impl<T: Record> Column for Arena<T> {
    fn append_value_bytes(&mut self, bytes: &[u8]) -> Result<usize> {
        if bytes.len() != T::SIZE {
            return Err(Error::corrupt(
                "value",
                "record width does not match column",
            ));
        }
        self.push(T::decode(bytes))
    }

    fn value_bytes(&self, row: Row) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        super::encode_record(self.row(row.index())?, &mut out);
        Ok(out)
    }

    fn blank_value_bytes(&self) -> Option<Vec<u8>> {
        Some(vec![0; T::SIZE])
    }

    fn schema_type(&self) -> String {
        std::any::type_name::<T>().to_owned()
    }

    fn rows(&self) -> usize {
        self.len()
    }

    fn touch_tail(&self) -> Result<()> {
        match self.chunk_count().checked_sub(1) {
            Some(last) => self.chunk(last).map(|_| ()),
            None => Ok(()),
        }
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: qualified(entity, field),
            elem_size: T::SIZE,
            kind: ColumnKind::Scalar,
            counts_rows: true,
        });
    }

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        out.push(Residency {
            name: qualified(entity, field),
            resident: self.resident_chunks(),
            chunks: self.chunk_count(),
        });
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        *self = file.column::<T>(&qualified(entity, field))?;
        Ok(())
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        updater.column(&qualified(entity, field), self)
    }

    fn rebase(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        file.rebase(&qualified(entity, field), self)
    }
}

pub struct Field<'a> {
    pub name: &'static str,
    pub column: &'a dyn Column,
}

pub struct FieldMut<'a> {
    pub name: &'static str,
    pub column: &'a mut dyn Column,
}

impl<'a> Field<'a> {
    pub fn new(name: &'static str, column: &'a dyn Column) -> Self {
        Self { name, column }
    }
}

impl<'a> FieldMut<'a> {
    pub fn new(name: &'static str, column: &'a mut dyn Column) -> Self {
        Self { name, column }
    }
}

#[cfg(test)]
#[path = "column_test.rs"]
mod column_test;
