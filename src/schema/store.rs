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
    compaction_policy: crate::CompactionPolicy,
    maintenance: Option<crate::MaintenanceOutcome>,
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
            compaction_policy: self.compaction_policy,
            maintenance: self.maintenance.clone(),
        }
    }
}

#[derive(Clone)]
pub struct Recovery<S> {
    pub pending: S,
    pub prepared: S,
    pub attempt: Option<crate::storage::CommitAttempt>,
    pub known: std::collections::BTreeSet<Dot>,
}

impl StoreState {
    pub(crate) fn attach(&mut self, file: ColumnFile, path: PathBuf) {
        self.file = Some(file);
        self.path = Some(path);
    }

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
    const SCHEMA_NAME: &'static str;
    const SCHEMA_VERSION: u64;

    fn schema_metadata(&self) -> super::SchemaMetadata {
        super::SchemaMetadata {
            format_version: 1,
            name: Self::SCHEMA_NAME.to_owned(),
            version: Self::SCHEMA_VERSION,
            descriptor: self.schema_descriptor(),
        }
    }

    fn merge_rows_from(&mut self, theirs: &Self, report: &mut MergeReport) -> Result<()>;

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
                Conflict::Combined(c) => report.combined.push(c),
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

    fn storage_stats(&self) -> Result<crate::StorageStats> {
        let mut stats = self.state().attached()?.0.storage_stats()?;
        stats.unused_heap_bytes = self
            .tables()
            .iter()
            .try_fold(0, |sum, table| -> Result<u64> {
                Ok(sum + table.unused_heap_bytes()?)
            })?;
        stats.retained_value_bytes = stats
            .retained_value_bytes
            .saturating_sub(stats.unused_heap_bytes);
        Ok(stats)
    }

    fn compaction_policy(&self) -> crate::CompactionPolicy {
        self.state().compaction_policy
    }

    fn set_compaction_policy(&mut self, policy: crate::CompactionPolicy) -> Result<()> {
        policy.validate()?;
        self.state_mut().compaction_policy = policy;
        Ok(())
    }

    fn maintenance_outcome(&self) -> Option<&crate::MaintenanceOutcome> {
        self.state().maintenance.as_ref()
    }

    fn compact(&mut self) -> Result<crate::CompactionReport> {
        self.check_published()?;
        let (file, path) = self.state().attached()?;
        let file = file.clone();
        let path = path.to_owned();
        let mut disk = Self::default();
        disk.read(&file)?;
        if self.clock() != disk.clock() {
            return Err(Error::invalid("commit pending changes before compaction"));
        }
        for (table, held) in self.tables().iter().zip(disk.tables()) {
            if table.revision_records()? != held.revision_records()? {
                return Err(Error::invalid("commit pending changes before compaction"));
            }
        }
        let mut prepared = self.snapshot();
        for table in prepared.tables_mut() {
            table.compact_columns()?;
        }
        prepared.audit()?;
        let mut attached = Self::default();
        let (published, report) = file.rewrite(
            |updater| prepared.offer_to(updater),
            |candidate| {
                attached.read(candidate)?;
                attached.audit()
            },
        )?;
        attached.state_mut().file = Some(published);
        attached.state_mut().path = Some(path);
        attached.state_mut().compaction_policy = self.compaction_policy();
        attached.state_mut().maintenance = Some(crate::MaintenanceOutcome::Compacted(report));
        *self = attached;
        Ok(report)
    }

    fn auto_compact(&mut self) {
        let policy = self.compaction_policy();
        if policy == crate::CompactionPolicy::Disabled {
            return;
        }
        let result = (|| {
            if let crate::CompactionPolicy::Automatic {
                min_reclaim_bytes, ..
            } = policy
            {
                let file = self.state().attached()?.0;
                if file.file_bytes()? < min_reclaim_bytes {
                    return Ok(None);
                }
            }
            if policy.should_compact(self.storage_stats()?) {
                self.compact().map(Some)
            } else {
                Ok(None)
            }
        })();
        if let Err(err) = result {
            self.state_mut().maintenance = Some(crate::MaintenanceOutcome::Failed(err.to_string()));
        }
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
        if file.column_meta(SCHEMA).is_some() {
            self.schema_metadata()
                .check(super::SchemaMetadata::read(file)?)?;
        }
        let diff = self.diff(file);
        if let Some(mismatch) = diff.to_error() {
            return Err(mismatch);
        }

        let schema: Arena<u8> = file.column(SCHEMA)?;
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
        schema.extend_from_slice(&self.schema_metadata().encode()?)?;
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
        Self::default().schema_metadata().validate()?;
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
        self.synchronize(None, writer_id)?;
        self.generation()
            .ok_or_else(|| Error::invalid("commit did not attach a generation"))
    }

    fn merge_from(&mut self, source: impl AsRef<Path>, writer_id: WriterId) -> Result<MergeReport> {
        self.synchronize(Some(source.as_ref()), writer_id)
    }

    fn synchronize(&mut self, source: Option<&Path>, writer_id: WriterId) -> Result<MergeReport> {
        if self.recovery().is_none() {
            self.check_mutable()?;
        }
        let path = self.state().attached()?.1.to_owned();
        let peer = source.map(Self::open).transpose()?;
        let mut before = self.snapshot();
        before.set_recovery(None);
        let (mut pending, mut carried, mut attempt, mut retained) = match self.recovery() {
            Some(recovery) => (
                recovery.pending.snapshot(),
                recovery.prepared.snapshot(),
                recovery.attempt.clone(),
                true,
            ),
            None => (self.snapshot(), self.snapshot(), None, false),
        };
        pending.set_recovery(None);
        carried.set_recovery(None);
        pending.state().view.block(false);
        carried.state().view.block(false);
        let bootstrap = self
            .state()
            .file
            .as_ref()
            .is_some_and(|file| file.columns().is_empty());
        let mut known = self.recovery().map(|recovery| recovery.known.clone());
        for number in 1..=3 {
            let result = (|| -> Result<(Self, MergeReport)> {
                let file = ColumnFile::open(&path)?;
                let mut latest = Self::default();
                latest.state_mut().path = Some(path.clone());
                if !(bootstrap
                    && file.columns().is_empty()
                    && file.commit_token().file_uuid
                        == self.state().attached()?.0.commit_token().file_uuid)
                {
                    latest.read(&file)?;
                }
                latest.state_mut().file = Some(file.clone());
                let expected = file.commit_token();
                let mut updater = file.update(writer_id)?;
                let mut clock = carried.clock().clone();
                clock.absorb(latest.clock());
                if let Some(peer) = &peer {
                    clock.absorb(peer.clock());
                }
                for table in carried.tables() {
                    table.rows_agree()?;
                }
                for table in carried.tables_mut() {
                    table.stamp_unstamped(&mut clock, writer_id)?;
                }
                carried.state_mut().clock = clock;
                if known.is_none() {
                    let mut dots = std::collections::BTreeSet::new();
                    for table in carried.tables() {
                        for (dot, _) in table.revision_owners()? {
                            dots.insert(dot);
                        }
                    }
                    known = Some(dots);
                }
                let mut report = MergeReport::default();
                latest.merge_rows_from(&carried, &mut report)?;
                latest.absorb(carried.clock())?;
                if let Some(peer) = &peer {
                    latest.merge_rows_from(peer, &mut report)?;
                    latest.absorb(peer.clock())?;
                }
                latest.refresh()?;
                latest.audit()?;
                report.taken = 0;
                for table in latest.tables() {
                    for (dot, _) in table.revision_owners()? {
                        if !known.as_ref().expect("prepared local dots").contains(&dot) {
                            report.taken += 1;
                        }
                    }
                }
                carried = latest.snapshot();
                retained = true;
                latest.offer_to(&mut updater)?;
                let published = match updater.commit_file() {
                    Ok(file) => file,
                    Err(Error::CommitUncertain {
                        attempt: uncertain,
                        source,
                    }) => {
                        attempt = Some((*uncertain).clone());
                        return Err(Error::CommitUncertain {
                            attempt: uncertain,
                            source,
                        });
                    }
                    Err(err) => return Err(err),
                };
                attempt = Some(crate::storage::CommitAttempt::published(
                    &published, expected,
                ));
                let found = ColumnFile::read_commit_token(&path)?;
                if found != published.commit_token() {
                    return Err(Error::Conflict {
                        expected: published.commit_token(),
                        found,
                    });
                }
                let mut attached = Self::default();
                attached.read(&published)?;
                attached.state_mut().file = Some(published);
                attached.state_mut().path = Some(path.clone());
                let report = attached.finish_report(&before, report)?;
                Ok((attached, report))
            })();
            match result {
                Ok((mut attached, report)) => {
                    attached.state_mut().compaction_policy = self.compaction_policy();
                    attached.state_mut().maintenance = self.state().maintenance.clone();
                    *self = attached;
                    self.auto_compact();
                    return Ok(report);
                }
                Err(err) => {
                    let retryable = matches!(
                        err,
                        Error::Io(_) | Error::Conflict { .. } | Error::CommitUncertain { .. }
                    );
                    if retryable && number < 3 {
                        continue;
                    }
                    if retained {
                        pending.state().view.block(true);
                        carried.state().view.block(true);
                        self.set_recovery(Some(Recovery {
                            pending,
                            prepared: carried,
                            attempt,
                            known: known.unwrap_or_default(),
                        }));
                        self.state().view.block(true);
                    }
                    return Err(if retryable {
                        Error::RetryExhausted {
                            attempts: number,
                            source: Box::new(err),
                        }
                    } else {
                        err
                    });
                }
            }
        }
        unreachable!("three publication attempts return a result")
    }
}

#[cfg(test)]
#[path = "store_test.rs"]
mod store_test;
