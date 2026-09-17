//! The container that owns the file: create, open, read, commit, snapshot.

use std::fs::File;
use std::path::{Path, PathBuf};

use super::column::{ColumnSpec, Residency};
use super::diff::SchemaDiff;
use super::table::AnyTable;
use crate::error::{Error, Result};
use crate::storage::{ColumnFile, ColumnFileUpdater, ColumnFileWriter, WriterId};

#[derive(Default, Clone)]
pub struct StoreState {
    file: Option<ColumnFile>,
    path: Option<PathBuf>,
    unknown: Vec<String>,
}

impl StoreState {
    fn attached(&self) -> Result<(&ColumnFile, &Path)> {
        match (&self.file, &self.path) {
            (Some(file), Some(path)) => Ok((file, path.as_path())),
            _ => Err(Error::invalid(
                "this store is not attached to a file; call create or open first",
            )),
        }
    }
}

pub trait Store: Default + Clone + Sized {
    fn state(&self) -> &StoreState;

    fn state_mut(&mut self) -> &mut StoreState;

    fn tables(&self) -> Vec<&dyn AnyTable>;

    fn tables_mut(&mut self) -> Vec<&mut dyn AnyTable>;

    fn specs(&self) -> Vec<ColumnSpec> {
        self.tables()
            .into_iter()
            .flat_map(|table| table.column_specs())
            .collect()
    }

    fn residency(&self) -> Vec<Residency> {
        self.tables()
            .into_iter()
            .flat_map(|table| table.column_residency())
            .collect()
    }

    fn generation(&self) -> Option<u64> {
        self.state().file.as_ref().map(ColumnFile::generation)
    }

    fn unknown_columns(&self) -> &[String] {
        &self.state().unknown
    }

    fn verify(&self) -> Result<()> {
        self.state().attached()?.0.verify()?;
        self.audit()
    }

    fn audit(&self) -> Result<()> {
        for table in self.tables() {
            table.audit_columns()?;
        }
        Ok(())
    }

    fn snapshot(&self) -> Self {
        self.clone()
    }

    fn diff(&self, file: &ColumnFile) -> SchemaDiff {
        SchemaDiff::of(&self.specs(), file)
    }

    fn read(&mut self, file: &ColumnFile) -> Result<()> {
        let diff = self.diff(file);
        if let Some(mismatch) = diff.to_error() {
            return Err(mismatch);
        }

        for table in self.tables_mut() {
            table.read_columns(file)?;
        }
        self.state_mut().unknown = diff.unknown;
        Ok(())
    }

    fn offer_to(&self, updater: &mut ColumnFileUpdater) -> Result<()> {
        for table in self.tables() {
            table.offer_columns(updater)?;
        }
        for name in &self.state().unknown {
            updater.keep_column(name)?;
        }
        Ok(())
    }

    fn create(path: impl AsRef<Path>, writer_id: WriterId) -> Result<Self> {
        let path = path.as_ref();
        let handle = File::options()
            .write(true)
            .read(true)
            .create_new(true)
            .open(path)?;
        ColumnFileWriter::new(handle, writer_id)?.finish()?;

        let mut store = Self::default();
        store.state_mut().file = Some(ColumnFile::open(path)?);
        store.state_mut().path = Some(path.to_owned());
        store.commit(writer_id)?;
        Ok(store)
    }

    fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = ColumnFile::open(path)?;

        let mut store = Self::default();
        store.read(&file)?;
        store.state_mut().file = Some(file);
        store.state_mut().path = Some(path.to_owned());
        Ok(store)
    }

    fn open_or_create(path: impl AsRef<Path>, writer_id: WriterId) -> Result<Self> {
        match Self::open(path.as_ref()) {
            Err(Error::Io(io)) if io.kind() == std::io::ErrorKind::NotFound => {
                Self::create(path, writer_id)
            }
            other => other,
        }
    }

    fn commit(&mut self, writer_id: WriterId) -> Result<u64> {
        for table in self.tables() {
            table.rows_agree()?;
        }

        let held = self.state().attached()?.0.generation();
        let mut updater = self.state().attached()?.0.update(writer_id)?;
        self.offer_to(&mut updater)?;
        let published = updater.commit()?;

        if published == held {
            return Ok(published);
        }

        let path = self.state().attached()?.1.to_owned();
        let reopened = ColumnFile::open(&path)?;
        self.read(&reopened)?;
        self.state_mut().file = Some(reopened);
        Ok(published)
    }
}

#[cfg(test)]
#[path = "store_test.rs"]
mod store_test;
