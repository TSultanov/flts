//! The container that owns the file: create, open, read, commit, snapshot.

use std::fs::File;
use std::path::{Path, PathBuf};

use super::clock::VersionVector;
use super::column::{ColumnKind, ColumnSpec, Residency};
use super::diff::SchemaDiff;
use super::table::AnyTable;
use super::version::Dot;
use super::{Conflict, MergeReport, RowStatus, View, VisibilityChange};
use crate::error::{Error, Result};
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater, ColumnFileWriter, WriterId};
use std::collections::{BTreeMap, HashMap};

pub const CLOCK: &str = "dbsync.clock";
pub const SCHEMA: &str = "dbsync.schema";

#[derive(Default)]
pub struct StoreState {
    file: Option<ColumnFile>,
    path: Option<PathBuf>,
    unknown: Vec<String>,
    clock: VersionVector,
    dots: Arena<Dot>,
    schema_bytes: Arena<u8>,
    view: View,
}

impl Clone for StoreState {
    fn clone(&self) -> Self {
        Self {
            file: self.file.clone(),
            path: self.path.clone(),
            unknown: self.unknown.clone(),
            clock: self.clock.clone(),
            dots: self.dots.clone(),
            schema_bytes: self.schema_bytes.clone(),
            view: self.view.fork(),
        }
    }
}

#[derive(Clone)]
pub struct Recovery<S> {
    pub pending: S,
    pub prepared: S,
    pub attempt: crate::storage::CommitAttempt,
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
    fn recovery(&self) -> Option<&Recovery<Self>>;
    fn set_recovery(&mut self, recovery: Option<Recovery<Self>>);

    fn bind_tables(&mut self) {
        let view = self.state().view.clone();
        for table in self.tables_mut() {
            view.register(table.entity());
            table.bind_view(view.clone());
        }
    }

    fn check_mutable(&self) -> Result<()> {
        self.state().view.check_mutable()
    }

    fn refresh(&self) -> Result<()> {
        for table in self.tables() {
            table.refresh_view()?;
        }
        Ok(())
    }

    fn schema_descriptor(&self) -> Vec<u8> {
        let mut schemas: Vec<_> = self
            .tables()
            .iter()
            .map(|t| t.schema_descriptor())
            .collect();
        schemas.sort();
        format!("row ancestry 1 {:?}", schemas).into_bytes()
    }

    fn audit_dots(&self) -> Result<()> {
        let mut owners = HashMap::new();
        for table in self.tables() {
            for (dot, id) in table.revision_owners()? {
                if !self.clock().dominates(dot) {
                    return Err(Error::corrupt(
                        "clock",
                        "clock does not cover a stored revision",
                    ));
                }
                if owners.insert(dot, (table.entity(), id)).is_some() {
                    return Err(Error::corrupt("revision", "dot reused across the store"));
                }
            }
        }
        Ok(())
    }

    fn audit_references(&self) -> Result<()> {
        let mut identities = std::collections::BTreeSet::new();
        for table in self.tables() {
            for id in table.identities()? {
                identities.insert((table.entity(), id));
            }
        }
        for table in self.tables() {
            for parent in table.all_references()? {
                if !parent.id.is_nil() && !identities.contains(&(parent.entity, parent.id)) {
                    return Err(Error::corrupt(
                        "reference",
                        format!(
                            "{} names missing {} {}",
                            parent.column, parent.entity, parent.id
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    fn conflicts(&self) -> Result<Vec<Conflict>> {
        self.refresh()?;
        let mut conflicts = self.state().view.conflicts();
        for table in self.tables() {
            conflicts.extend(table.row_conflicts()?);
        }
        conflicts.sort();
        conflicts.dedup();
        Ok(conflicts)
    }

    fn statuses(&self) -> Result<BTreeMap<(&'static str, crate::Uuid), RowStatus>> {
        self.refresh()?;
        Ok(self.state().view.statuses())
    }

    fn finish_report(&self, before: &Self, mut report: MergeReport) -> Result<MergeReport> {
        let old = before.conflicts()?;
        for conflict in self.conflicts()? {
            if old.contains(&conflict) {
                continue;
            }
            match conflict {
                Conflict::Concurrent(c) => report.concurrent.push(c),
                Conflict::Collision(c) => report.collided.push(c),
                Conflict::Reference(r) => report.references.push(r),
            }
        }
        let old_statuses = before.statuses()?;
        for ((entity, id), after) in self.statuses()? {
            let before = old_statuses.get(&(entity, id)).cloned();
            if before.as_ref() != Some(&after) {
                report.visibility.push(VisibilityChange {
                    entity,
                    id,
                    before,
                    after,
                });
            }
        }
        report.clock_changed = self.clock() != before.clock();
        Ok(report)
    }

    fn state(&self) -> &StoreState;

    fn state_mut(&mut self) -> &mut StoreState;

    fn tables(&self) -> Vec<&dyn AnyTable>;

    fn tables_mut(&mut self) -> Vec<&mut dyn AnyTable>;

    fn audit(&self) -> Result<()>;

    fn clock(&self) -> &VersionVector {
        &self.state().clock
    }

    fn tick(&mut self, writer: WriterId) -> Result<Dot> {
        self.check_mutable()?;
        self.state_mut().clock.tick(writer)
    }

    fn absorb(&mut self, told: &VersionVector) -> Result<()> {
        self.check_mutable()?;
        self.state_mut().clock.absorb(told);
        Ok(())
    }

    fn check_published(&self) -> Result<()> {
        self.check_mutable()?;
        for table in self.tables() {
            if !table.published() {
                return Err(Error::invalid(format!(
                    "{} holds a write nobody has published; commit before merging,                      because a write with no name of its own cannot be told apart                      from another copy's",
                    table.entity()
                )));
            }
        }
        Ok(())
    }

    fn specs(&self) -> Vec<ColumnSpec> {
        let mut out: Vec<ColumnSpec> = self
            .tables()
            .into_iter()
            .flat_map(|table| table.column_specs())
            .collect();
        self.clock().describe(CLOCK, &mut out);
        out.push(ColumnSpec {
            name: SCHEMA.to_owned(),
            elem_size: 1,
            kind: ColumnKind::Scalar,
            counts_rows: false,
        });
        out
    }

    fn residency(&self) -> Vec<Residency> {
        let mut out: Vec<Residency> = self
            .tables()
            .into_iter()
            .flat_map(|table| table.column_residency())
            .collect();
        self.clock().residency(CLOCK, &mut out);
        out.push(Residency {
            name: SCHEMA.to_owned(),
            resident: 1,
            chunks: 1,
        });
        out
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

    fn snapshot(&self) -> Self {
        let mut copy = self.clone();
        copy.bind_tables();
        copy
    }

    fn diff(&self, file: &ColumnFile) -> SchemaDiff {
        SchemaDiff::of(&self.specs(), file)
    }

    fn read(&mut self, file: &ColumnFile) -> Result<()> {
        self.check_mutable()?;
        let diff = self.diff(file);
        if let Some(mismatch) = diff.to_error() {
            return Err(mismatch);
        }

        let schema: Arena<u8> = file.column(SCHEMA)?;
        let actual = schema
            .iter()
            .map(|v| v.copied())
            .collect::<Result<Vec<_>>>()?;
        if actual != self.schema_descriptor() {
            return Err(Error::unsupported("schema does not match this store"));
        }
        if !diff.unknown.is_empty() {
            return Err(Error::unsupported(format!(
                "unknown columns: {:?}",
                diff.unknown
            )));
        }
        self.bind_tables();
        for table in self.tables_mut() {
            table.read_columns(file)?;
        }
        let dots: Arena<Dot> = file.column(CLOCK)?;
        let mut clock = VersionVector::default();
        clock.read_from(&dots)?;
        self.state_mut().clock = clock;
        self.state_mut().dots = dots;
        self.state_mut().unknown = diff.unknown;
        self.state_mut().schema_bytes = schema;
        self.audit_dots()?;
        self.check_published()?;
        self.audit_references()?;
        self.refresh()
    }

    fn offer_to(&self, updater: &mut ColumnFileUpdater) -> Result<()> {
        self.check_mutable()?;
        for table in self.tables() {
            table.offer_columns(updater)?;
        }
        self.clock().offer_to(CLOCK, &self.state().dots, updater)?;
        let mut schema = Arena::new();
        schema.extend_from_slice(&self.schema_descriptor())?;
        updater.column(
            SCHEMA,
            if self.state().schema_bytes.is_empty() {
                &schema
            } else {
                &self.state().schema_bytes
            },
        )?;
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
        self.check_mutable()?;
        let expected = self.state().attached()?.0.commit_token();
        let mut updater = self.state().attached()?.0.update(writer_id)?;
        let mut prepared = self.snapshot();
        for table in prepared.tables() {
            table.rows_agree()?;
        }
        prepared.refresh()?;
        prepared.audit()?;
        let mut clock = prepared.clock().clone();
        for table in prepared.tables_mut() {
            table.stamp_unstamped(&mut clock, writer_id)?;
        }
        prepared.state_mut().clock = clock;
        prepared.audit_dots()?;
        prepared.offer_to(&mut updater)?;
        match updater.commit_file() {
            Ok(file) => {
                let generation = file.generation();
                let mut attached = Self::default();
                attached.state_mut().path = prepared.state().path.clone();
                if let Err(err) = attached.read(&file) {
                    let attempt = crate::storage::CommitAttempt::published(&file, expected);
                    let pending = self.snapshot();
                    pending.state().view.block(true);
                    prepared.state().view.block(true);
                    self.set_recovery(Some(Recovery {
                        pending,
                        prepared,
                        attempt: attempt.clone(),
                    }));
                    self.state().view.block(true);
                    return Err(Error::CommitUncertain {
                        attempt: Box::new(attempt),
                        source: std::io::Error::other(err),
                    });
                }
                attached.state_mut().file = Some(file);
                *self = attached;
                Ok(generation)
            }
            Err(Error::CommitUncertain { attempt, source }) => {
                let pending = self.snapshot();
                pending.state().view.block(true);
                prepared.state().view.block(true);
                self.set_recovery(Some(Recovery {
                    pending,
                    prepared,
                    attempt: (*attempt).clone(),
                }));
                self.state().view.block(true);
                Err(Error::CommitUncertain { attempt, source })
            }
            Err(err) => Err(err),
        }
    }

    fn recover_commit(&mut self) -> Result<u64> {
        let recovery = self
            .recovery()
            .ok_or_else(|| Error::invalid("no uncertain commit to recover"))?
            .clone();
        let file = recovery.attempt.inspect()?;
        let generation = file.generation();
        let token = file.commit_token();
        if token == recovery.attempt.attempted {
            let mut restored = recovery.prepared;
            restored.state_mut().file = Some(file);
            restored.state().view.block(false);
            restored.set_recovery(None);
            *self = restored;
            return Ok(generation);
        }
        if token == recovery.attempt.expected {
            let mut restored = recovery.pending;
            restored.state_mut().file = Some(file);
            restored.state().view.block(false);
            restored.set_recovery(None);
            *self = restored;
            return Ok(generation);
        }
        let mut found = Self::default();
        found.read(&file)?;
        let mut records = HashMap::new();
        for table in found.tables() {
            for record in table.revision_records()? {
                records.insert(record.version.dot(), record);
            }
        }
        let mut included = true;
        for table in recovery.prepared.tables() {
            for record in table.revision_records()? {
                if records.get(&record.version.dot()) != Some(&record) {
                    included = false;
                    break;
                }
            }
        }
        if included {
            found.state_mut().file = Some(file);
            found.state_mut().path = self.state().path.clone();
            *self = found;
            return Ok(generation);
        }
        Err(Error::invalid(
            "commit outcome cannot be established; pending edits remain retained",
        ))
    }
}

#[cfg(test)]
#[path = "store_test.rs"]
mod store_test;
