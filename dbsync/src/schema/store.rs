//! The container that owns a directory: every writer's shards, and the cache built from them.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::path::{Path, PathBuf};

use super::clock::VersionVector;
use super::column::{ColumnKind, ColumnSpec, Residency};
use super::diff::SchemaDiff;
use super::peers::{self, Peer};
use super::shard::{self, ShardFile, ShardRow};
use super::table::{Adoption, AnyTable};
use super::version::Dot;
use super::{
    Conflict, MergeReport, PruneReport, PurgeReport, Reference, RowStatus, View, VisibilityChange,
};
use crate::error::{Error, Result};
use crate::record::Record;
use crate::storage::{
    Arena, ColumnFile, ColumnFileUpdater, ColumnFileWriter, CommitToken, Durability, Flush,
    WriterId, fnv1a,
};

pub const CLOCK: &str = "dbsync.clock";
pub const SCHEMA: &str = "dbsync.schema";
pub const SHARDS: &str = "dbsync.shards";
#[cfg(test)]
thread_local! {
    /// Makes the next commit stop after its first table's append.
    pub(crate) static CUT_AFTER_APPEND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Makes the next drop of rows stop after its first shard is rewritten.
    pub(crate) static CUT_AFTER_REWRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Directory, in the local half, of a commit's rows not yet in every shard.
const STASH: &str = "pending";
const STASH_READY: &str = "ready";
pub const DURABLE: &str = "dbsync.durable";
const SETTLED: u64 = u64::MAX;
const UNFLUSHED_FOR: std::time::Duration = std::time::Duration::from_secs(1);
const UNFLUSHED_BYTES: u64 = 256 * 1024;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Layout {
    pub shared: PathBuf,
    pub local: PathBuf,
}

impl Layout {
    pub fn within(dir: impl AsRef<Path>) -> Self {
        Self {
            shared: dir.as_ref().to_owned(),
            local: dir.as_ref().join("local"),
        }
    }

    pub fn split(shared: impl AsRef<Path>, local: impl AsRef<Path>) -> Self {
        Self {
            shared: shared.as_ref().to_owned(),
            local: local.as_ref().to_owned(),
        }
    }

    pub fn cache(&self, schema_version: u64) -> PathBuf {
        self.local.join(format!("cache.v{schema_version}.r2.dbs"))
    }

    pub fn shard(&self, entity: &str, writer: WriterId, schema_version: u64) -> PathBuf {
        shard::shard_path(&self.shared, entity, writer, schema_version)
    }

    fn lease(&self) -> Result<Lease> {
        std::fs::create_dir_all(&self.local)?;
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.local.join("lock"))?;
        file.lock()?;
        Ok(Lease(file))
    }
}

impl<P: AsRef<Path>> From<P> for Layout {
    fn from(dir: P) -> Self {
        Self::within(dir)
    }
}

struct Lease(File);

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ShardMark {
    pub entity: u64,
    pub writer: WriterId,
    pub schema_version: u64,
    pub file_uuid: u128,
    pub generation: u64,
    pub content_id: u64,
    pub rows: u64,
    pub floor: u64,
}

impl ShardMark {
    fn of(shard: &ShardFile, token: CommitToken, rows: usize, floor: u64) -> Self {
        Self {
            entity: fnv1a(shard.entity.as_bytes()),
            writer: shard.writer,
            schema_version: shard.schema_version,
            file_uuid: token.file_uuid,
            generation: token.generation,
            content_id: token.content_id,
            rows: rows as u64,
            floor,
        }
    }

    fn names(&self, shard: &ShardFile) -> bool {
        self.entity == fnv1a(shard.entity.as_bytes())
            && self.writer == shard.writer
            && self.schema_version == shard.schema_version
    }

    fn is_at(&self, token: CommitToken) -> bool {
        self.file_uuid == token.file_uuid
            && self.generation == token.generation
            && self.content_id == token.content_id
    }
}

impl Record for ShardMark {
    const SIZE: usize = 7 * size_of::<u64>() + size_of::<u128>();

    fn encode(&self, out: &mut [u8]) {
        out[..8].copy_from_slice(&self.entity.to_le_bytes());
        out[8..16].copy_from_slice(&self.writer.raw().to_le_bytes());
        out[16..24].copy_from_slice(&self.schema_version.to_le_bytes());
        out[24..40].copy_from_slice(&self.file_uuid.to_le_bytes());
        out[40..48].copy_from_slice(&self.generation.to_le_bytes());
        out[48..56].copy_from_slice(&self.content_id.to_le_bytes());
        out[56..64].copy_from_slice(&self.rows.to_le_bytes());
        out[64..72].copy_from_slice(&self.floor.to_le_bytes());
    }

    fn decode(src: &[u8]) -> Self {
        let word = |at: usize| u64::from_le_bytes(src[at..at + 8].try_into().expect("8 bytes"));
        Self {
            entity: word(0),
            writer: WriterId::from_raw(word(8)),
            schema_version: word(16),
            file_uuid: u128::from_le_bytes(src[24..40].try_into().expect("16 bytes")),
            generation: word(40),
            content_id: word(48),
            rows: word(56),
            floor: word(64),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Batch {
    pub table: usize,
    pub first: usize,
    pub rows: Vec<ShardRow>,
    pub mark: Option<ShardMark>,
}

#[derive(Clone, Debug)]
struct Outgoing {
    table: usize,
    writer: WriterId,
    rows: Vec<ShardRow>,
}

#[derive(Default)]
pub struct StoreState {
    file: Option<ColumnFile>,
    layout: Option<Layout>,
    unknown: Vec<String>,
    dropped: Vec<String>,
    clock: VersionVector,
    dots: Arena<Dot>,
    schema_bytes: Arena<u8>,
    marks: Vec<ShardMark>,
    held_marks: Arena<ShardMark>,
    outbox: Vec<Outgoing>,
    said: Option<Peer>,
    durability: Option<Durability>,
    unflushed_from: Option<u64>,
    flushed_at: Option<std::time::Instant>,
    view: View,
    compaction_policy: crate::CompactionPolicy,
    maintenance: Option<crate::MaintenanceOutcome>,
    pub(super) purges_settled: Vec<usize>,
}

impl Clone for StoreState {
    fn clone(&self) -> Self {
        Self {
            file: self.file.clone(),
            layout: self.layout.clone(),
            unknown: self.unknown.clone(),
            dropped: self.dropped.clone(),
            clock: self.clock.clone(),
            dots: self.dots.clone(),
            schema_bytes: self.schema_bytes.clone(),
            marks: self.marks.clone(),
            held_marks: self.held_marks.clone(),
            outbox: self.outbox.clone(),
            said: self.said.clone(),
            durability: self.durability,
            unflushed_from: self.unflushed_from,
            flushed_at: self.flushed_at,
            view: self.view.fork(),
            compaction_policy: self.compaction_policy,
            maintenance: self.maintenance.clone(),
            purges_settled: self.purges_settled.clone(),
        }
    }
}

impl StoreState {
    fn attached(&self) -> Result<(&ColumnFile, &Layout)> {
        match (&self.file, &self.layout) {
            (Some(file), Some(layout)) => Ok((file, layout)),
            _ => Err(Error::invalid(
                "this store is not attached to a directory; call create or open first",
            )),
        }
    }

    fn mark(&mut self, mark: ShardMark) {
        match self.marks.iter_mut().find(|held| {
            held.entity == mark.entity
                && held.writer == mark.writer
                && held.schema_version == mark.schema_version
        }) {
            Some(held) => *held = mark,
            None => self.marks.push(mark),
        }
    }

    pub(crate) fn detached_at(&mut self, layout: Layout, marks: &[ShardMark]) {
        self.layout = Some(layout);
        self.marks = marks.to_vec();
    }

    fn placed(&self) -> Result<&Layout> {
        self.layout.as_ref().ok_or_else(|| {
            Error::invalid("this store is not attached to a directory; call create or open first")
        })
    }

    fn marks_are_held(&self) -> Result<bool> {
        if self.held_marks.len() != self.marks.len() {
            return Ok(false);
        }
        for (row, mark) in self.marks.iter().enumerate() {
            if self.held_marks.row(row)? != mark {
                return Ok(false);
            }
        }
        Ok(true)
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

    fn bind_tables(&mut self) {
        let view = self.state().view.clone();
        for table in self.tables_mut() {
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
        let fields: Vec<_> = self
            .tables()
            .iter()
            .flat_map(|table| table.described())
            .collect();
        super::SchemaMetadata::describe(&fields)
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

    fn dangling(&self) -> Result<Vec<Reference>> {
        let mut identities = BTreeSet::new();
        for table in self.tables() {
            for id in table.identities()? {
                identities.insert((table.entity(), id));
            }
        }
        let mut dangling = Vec::new();
        for table in self.tables() {
            for parent in table.all_references()? {
                if !parent.id.is_nil() && !identities.contains(&(parent.entity, parent.id)) {
                    dangling.push(parent);
                }
            }
        }
        dangling.sort();
        dangling.dedup();
        Ok(dangling)
    }

    fn conflicts(&self) -> Result<Vec<Conflict>> {
        let mut conflicts = self.state().view.conflicts()?;
        for table in self.tables() {
            conflicts.extend(table.row_conflicts()?);
        }
        conflicts.sort();
        conflicts.dedup();
        Ok(conflicts)
    }

    fn statuses(&self) -> Result<BTreeMap<(&'static str, crate::Uuid), RowStatus>> {
        self.state().view.statuses()
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

    fn layout(&self) -> Option<&Layout> {
        self.state().layout.as_ref()
    }

    fn shard_marks(&self) -> &[ShardMark] {
        &self.state().marks
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
                    "{} holds a write nobody has published; commit before merging, \
                     because a write with no name of its own cannot be told apart \
                     from another copy's",
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
        self.state().view.describe(&mut out);
        self.clock().describe(CLOCK, &mut out);
        out.push(ColumnSpec {
            name: SHARDS.to_owned(),
            elem_size: <ShardMark as Record>::SIZE,
            kind: ColumnKind::Clock,
            counts_rows: false,
        });
        out.push(ColumnSpec {
            name: DURABLE.to_owned(),
            elem_size: <u64 as Record>::SIZE,
            kind: ColumnKind::Clock,
            counts_rows: false,
        });
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
        self.state().view.residency(&mut out);
        self.clock().residency(CLOCK, &mut out);
        out.push(Residency {
            name: SHARDS.to_owned(),
            resident: 1,
            chunks: 1,
        });
        out.push(Residency {
            name: DURABLE.to_owned(),
            resident: 1,
            chunks: 1,
        });
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

    fn durability(&self) -> Durability {
        self.state().durability.unwrap_or(Durability::Relaxed)
    }

    fn set_durability(&mut self, durability: Durability) {
        self.state_mut().durability = Some(durability);
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
        let (file, layout) = self.state().attached()?;
        let file = file.clone();
        let layout = layout.clone();
        if !self.state().outbox.is_empty() {
            return Err(Error::invalid("commit pending changes before compaction"));
        }
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
            table.fold_indexes(true)?;
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
        attached.state_mut().layout = Some(layout);
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
            let file = self.state().attached()?.0;
            let file_bytes = file.file_bytes()?;
            if let crate::CompactionPolicy::Automatic {
                min_reclaim_bytes, ..
            } = policy
                && file_bytes < min_reclaim_bytes
            {
                return Ok(None);
            }
            if policy.should_compact(file_bytes, file.obsolete_bytes()?) {
                self.compact().map(Some)
            } else {
                Ok(None)
            }
        })();
        if let Err(err) = result {
            self.state_mut().maintenance = Some(crate::MaintenanceOutcome::Failed(err.to_string()));
        }
    }

    /// Rewrites the cache and `writer_id`'s shards where `policy` says it
    /// pays, whatever the store's own policy. A store whose policy is
    /// `Disabled` never compacts inside a commit; this is how it compacts
    /// at a time of its choosing.
    fn compact_by(&mut self, writer_id: WriterId, policy: crate::CompactionPolicy) -> Result<()> {
        self.check_published()?;
        policy.validate()?;
        let _lease = self.state().attached()?.1.lease()?;
        let own = self.compaction_policy();
        self.state_mut().compaction_policy = policy;
        self.auto_compact();
        let shards = self.rewrite_shards(writer_id, Some(policy));
        self.state_mut().compaction_policy = own;
        shards.map(|_| ())
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

    fn audit_view(&self) -> Result<()> {
        for table in self.tables() {
            table.audit_standing()?;
        }
        let mut again = self.clone();
        again.state_mut().view = View::default();
        again.bind_tables();
        again.refresh()?;
        if again.state().view.statuses()? != self.state().view.statuses()? {
            return Err(Error::corrupt(
                "view",
                "the visibility on record is not what the rows imply",
            ));
        }
        Ok(())
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
        self.state().view.read_from(file)?;
        for table in self.tables_mut() {
            table.read_columns(file)?;
        }
        let dots: Arena<Dot> = file.column(CLOCK)?;
        let mut clock = VersionVector::default();
        clock.read_from(&dots)?;
        let held_marks: Arena<ShardMark> = file.column(SHARDS)?;
        let marks = held_marks
            .iter()
            .map(|mark| mark.copied())
            .collect::<Result<Vec<_>>>()?;
        let unflushed_from = file
            .column::<u64>(DURABLE)?
            .get(0)?
            .copied()
            .filter(|from| *from != SETTLED);
        let state = self.state_mut();
        state.unflushed_from = unflushed_from;
        state.clock = clock;
        state.dots = dots;
        state.marks = marks;
        state.held_marks = held_marks;
        state.dropped = diff.to_drop();
        state.unknown = diff.unknown;
        state.schema_bytes = schema;
        Ok(())
    }

    fn offer_to(&self, updater: &mut ColumnFileUpdater) -> Result<()> {
        self.check_mutable()?;
        for name in &self.state().dropped {
            updater.remove_column(name);
        }
        for table in self.tables() {
            table.offer_columns(updater)?;
        }
        self.state().view.offer_to(updater)?;
        self.clock().offer_to(CLOCK, &self.state().dots, updater)?;
        if self.state().marks_are_held()? {
            updater.column(SHARDS, &self.state().held_marks)?;
        } else {
            let mut marks = Arena::new();
            marks.extend_from_slice(&self.state().marks)?;
            updater.column(SHARDS, &marks)?;
        }
        let mut durable = Arena::new();
        durable.push(self.state().unflushed_from.unwrap_or(SETTLED))?;
        updater.column(DURABLE, &durable)?;
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

    fn create(layout: impl Into<Layout>, writer_id: WriterId) -> Result<Self> {
        Self::default().schema_metadata().validate()?;
        let layout = layout.into();
        std::fs::create_dir_all(&layout.shared)?;
        let _lease = layout.lease()?;
        let cache = layout.cache(Self::SCHEMA_VERSION);
        if cache.exists() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} already holds a store", layout.shared.display()),
            )));
        }
        let mut store = Self::with_fresh_cache(layout)?;
        let mut report = MergeReport::default();
        let batches = store.collect(|_| true, &mut report)?;
        store.apply(batches, &mut report)?;
        store.write_cache(writer_id, true)?;
        Ok(store)
    }

    fn open(layout: impl Into<Layout>) -> Result<Self> {
        let store = Self::open_beside_older(layout)?;
        let (_, placed) = store.state().attached()?;
        for entity in shard::entities(&placed.shared, &placed.local)? {
            for found in shard::discover(&placed.shared, &entity)?.0 {
                if found.schema_version < Self::SCHEMA_VERSION
                    && !store.state().marks.iter().any(|mark| mark.names(&found))
                {
                    return Err(Error::unsupported(format!(
                        "{} was written by schema version {}; open this store through its                          migrations so those rows are converted",
                        found.path.display(),
                        found.schema_version
                    )));
                }
            }
        }
        Ok(store)
    }

    fn open_beside_older(layout: impl Into<Layout>) -> Result<Self> {
        let layout = layout.into();
        if !layout.shared.is_dir() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} is not a store directory", layout.shared.display()),
            )));
        }
        let _lease = layout.lease()?;
        let cache = layout.cache(Self::SCHEMA_VERSION);
        let held = ColumnFile::open(&cache).and_then(|file| {
            let mut store = Self::default();
            store.read(&file)?;
            if let Some(from) = store.state().unflushed_from {
                file.verify_from(from)?;
            }
            store.state_mut().file = Some(file);
            store.state_mut().layout = Some(layout.clone());
            Ok(store)
        });
        let (mut store, rebuilt) = match held {
            Ok(store) => (store, false),
            Err(Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
                (Self::with_fresh_cache(layout)?, true)
            }
            Err(err) if err.is_corrupt() => (Self::with_fresh_cache(layout)?, true),
            Err(Error::SchemaMismatch { expected, actual })
                if expected.can_read(&actual)? && actual.can_read(&expected)? =>
            {
                (Self::with_fresh_cache(layout)?, true)
            }
            Err(err) => return Err(err),
        };
        store.finish_stashed()?;
        let mut report = MergeReport::default();
        let batches = store.collect(|_| true, &mut report)?;
        let moved = batches.iter().any(|batch| batch.mark.is_some());
        store.apply(batches, &mut report)?;
        if rebuilt || moved {
            store.write_cache(WriterId::ANONYMOUS, true)?;
        }
        Ok(store)
    }

    /// Where a commit across tables keeps its rows until every table has them.
    fn stash_of(layout: &Layout, entity: &str) -> PathBuf {
        layout.local.join(STASH).join(format!("{entity}.dbs"))
    }

    /// Makes the outbox last before the first of its appends. One table's
    /// append lands whole or not at all; a commit to several can be cut
    /// between them, leaving rows that name rows no shard holds.
    fn stash_outbox(&self) -> Result<()> {
        let outbox = &self.state().outbox;
        let touched: BTreeSet<usize> = outbox.iter().map(|next| next.table).collect();
        if touched.len() < 2 {
            return Ok(());
        }
        let layout = self.state().attached()?.1;
        let schema = self.schema_metadata().encode()?;
        let ready = layout.local.join(STASH).join(STASH_READY);
        match std::fs::remove_file(&ready) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
            _ => {}
        }
        for at in touched {
            let table = self.tables()[at];
            let held = || outbox.iter().filter(|next| next.table == at);
            let rows: Vec<ShardRow> = held().flat_map(|next| next.rows.iter().cloned()).collect();
            let Some(writer) = held().map(|next| next.writer).next() else {
                continue;
            };
            shard::rewrite(
                &Self::stash_of(layout, table.entity()),
                writer,
                &schema,
                &table.field_names(),
                &rows,
                0,
                self.durability(),
            )?;
        }
        // Only a stash that names every table may be replayed.
        let done = std::fs::File::create(&ready)?;
        self.durability().settle(&done)?;
        Ok(())
    }

    fn drop_stash(&self) -> Result<()> {
        let layout = self.state().attached()?.1;
        match std::fs::remove_dir_all(layout.local.join(STASH)) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        }
    }

    /// Finishes a commit that was cut short: what it stashed and no shard
    /// holds yet is appended to the shard it was on its way to.
    fn finish_stashed(&mut self) -> Result<()> {
        let layout = self.state().attached()?.1.clone();
        if !layout.local.join(STASH).is_dir() {
            return Ok(());
        }
        // Cut while stashing: no append had begun, so nothing is torn.
        if !layout.local.join(STASH).join(STASH_READY).is_file() {
            return self.drop_stash();
        }
        let schema = self.schema_metadata().encode()?;
        for table in self.tables() {
            let stashed = match shard::read(&Self::stash_of(&layout, table.entity()), 0) {
                Ok(read) => read.read,
                Err(Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            let mut by_writer: BTreeMap<WriterId, Vec<ShardRow>> = BTreeMap::new();
            for row in stashed {
                by_writer
                    .entry(row.version.dot().writer())
                    .or_default()
                    .push(row);
            }
            for (writer, rows) in by_writer {
                let path = layout.shard(table.entity(), writer, Self::SCHEMA_VERSION);
                let held: BTreeSet<Dot> = match shard::read(&path, 0) {
                    Ok(read) => read.read.iter().map(|row| row.version.dot()).collect(),
                    Err(Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
                        BTreeSet::new()
                    }
                    Err(err) => return Err(err),
                };
                let missing: Vec<ShardRow> = rows
                    .into_iter()
                    .filter(|row| !held.contains(&row.version.dot()))
                    .collect();
                if !missing.is_empty() {
                    shard::append(
                        &path,
                        writer,
                        &schema,
                        &table.field_names(),
                        &missing,
                        self.durability(),
                    )?;
                }
            }
        }
        self.drop_stash()
    }

    fn with_fresh_cache(layout: Layout) -> Result<Self> {
        std::fs::create_dir_all(&layout.local)?;
        let earlier = layout
            .local
            .join(format!("cache.v{}.dbs", Self::SCHEMA_VERSION));
        if earlier.exists() {
            std::fs::remove_file(earlier)?;
        }
        let cache = layout.cache(Self::SCHEMA_VERSION);
        ColumnFileWriter::create(&cache, WriterId::ANONYMOUS)?.finish()?;
        let mut store = Self::default();
        store.state_mut().file = Some(ColumnFile::open(&cache)?);
        store.state_mut().layout = Some(layout);
        Ok(store)
    }

    fn rebuild_cache(&mut self) -> Result<()> {
        let layout = self.state().attached()?.1.clone();
        if !self.state().outbox.is_empty() || !self.tables().iter().all(|table| table.published()) {
            return Err(Error::invalid(
                "commit pending changes before rebuilding the cache",
            ));
        }
        let _lease = layout.lease()?;
        let mut rebuilt = Self::with_fresh_cache(layout)?;
        let mut report = MergeReport::default();
        let batches = rebuilt.collect(|_| true, &mut report)?;
        rebuilt.apply(batches, &mut report)?;
        rebuilt.write_cache(WriterId::ANONYMOUS, true)?;
        let state = rebuilt.state_mut();
        state.compaction_policy = self.state().compaction_policy;
        state.durability = self.state().durability;
        state.maintenance = self.state().maintenance.clone();
        state.said = self.state().said.clone();
        *self = rebuilt;
        Ok(())
    }

    fn open_or_create(layout: impl Into<Layout>, writer_id: WriterId) -> Result<Self> {
        let layout = layout.into();
        match Self::open(layout.clone()) {
            Err(Error::Io(io)) if io.kind() == std::io::ErrorKind::NotFound => {
                Self::create(layout, writer_id)
            }
            other => other,
        }
    }

    fn collect(
        &self,
        wanted: impl Fn(WriterId) -> bool,
        report: &mut MergeReport,
    ) -> Result<Vec<Batch>> {
        let layout = self.state().placed()?;
        let schema = self.schema_metadata();
        let mut batches = Vec::new();
        for (index, table) in self.tables().into_iter().enumerate() {
            let (shards, strangers) = shard::discover(&layout.shared, table.entity())?;
            report.strangers.extend(strangers);
            for found in shards {
                if !wanted(found.writer) {
                    continue;
                }
                let held = self
                    .state()
                    .marks
                    .iter()
                    .find(|mark| mark.names(&found))
                    .copied();
                let token = match shard::token_of(&found.path) {
                    Ok(token) => token,
                    Err(Error::Io(_) | Error::Corrupt { .. }) => {
                        report.unreadable.push(found.path);
                        continue;
                    }
                    Err(err) => return Err(err),
                };
                if held.is_some_and(|mark| mark.is_at(token)) {
                    continue;
                }
                if found.schema_version != Self::SCHEMA_VERSION {
                    report.incompatible.push(found.path);
                    continue;
                }
                let from = match held {
                    Some(mark) if mark.file_uuid == token.file_uuid => mark.rows as usize,
                    _ => 0,
                };
                // The last row read before is read again: where files are
                // synced, a file of the same lineage can arrive holding other
                // rows at the places already read, and then all of it is news.
                let read = match shard::read(&found.path, from.saturating_sub(1)) {
                    Ok(mut read) if from > 0 => match read.read.first() {
                        Some(last) if table.agrees_with(last)? == Some(true) => {
                            read.read.remove(0);
                            Ok(read)
                        }
                        _ => shard::read(&found.path, 0),
                    },
                    // Shorter than what was read: not the file that was read.
                    Err(Error::Corrupt { .. }) if from > 0 => shard::read(&found.path, 0),
                    other => other,
                };
                let from = match &read {
                    Ok(read) if read.read.len() == read.rows => 0,
                    _ => from,
                };
                let read = match read {
                    Ok(read) => read,
                    Err(Error::Io(_) | Error::Corrupt { .. }) => {
                        report.unreadable.push(found.path);
                        continue;
                    }
                    Err(err) => return Err(err),
                };
                if read.rows > 0 {
                    let required = table
                        .field_names()
                        .into_iter()
                        .filter(|field| !table.optional_fields().contains(field));
                    let readable = match super::SchemaMetadata::decode(&read.schema) {
                        Ok(written) => {
                            let mut readable = schema.can_read(&written)?;
                            for field in required {
                                readable &= written.names(table.entity(), field)?;
                            }
                            readable
                        }
                        Err(_) => false,
                    };
                    if !readable {
                        report.incompatible.push(found.path);
                        continue;
                    }
                }
                batches.push(Batch {
                    table: index,
                    first: from,
                    mark: Some(ShardMark::of(&found, read.token, read.rows, read.floor)),
                    rows: read.read,
                });
            }
        }
        let known: Vec<&str> = self.tables().iter().map(|table| table.entity()).collect();
        for entity in shard::entities(&layout.shared, &layout.local)? {
            if known.contains(&entity.as_str()) {
                continue;
            }
            for found in shard::discover(&layout.shared, &entity)?.0 {
                if found.schema_version != Self::SCHEMA_VERSION
                    && !self.state().marks.iter().any(|mark| mark.names(&found))
                {
                    report.incompatible.push(found.path);
                }
            }
        }
        report.strangers.sort();
        report.unreadable.sort();
        report.incompatible.sort();
        Ok(batches)
    }

    fn apply(&mut self, batches: Vec<Batch>, report: &mut MergeReport) -> Result<()> {
        let quiet = |batch: &Batch| batch.rows.is_empty() && !self.may_lack_rows(batch);
        if batches.iter().all(quiet) {
            for mark in batches.into_iter().filter_map(|batch| batch.mark) {
                self.state_mut().mark(mark);
            }
            return Ok(());
        }
        let kept = self.snapshot();
        let said = self.adopt(batches, report);
        if said.is_err() {
            *self = kept;
        }
        said
    }

    // Whether `batch` is all of a file not read before, from a writer that drops rows: the
    // floor tells how far its drops reach, not when the last one was.
    fn may_lack_rows(&self, batch: &Batch) -> bool {
        let Some(mark) = batch.mark.filter(|_| batch.first == 0) else {
            return false;
        };
        let known = self.state().marks.iter().find(|held| {
            held.entity == mark.entity
                && held.writer == mark.writer
                && held.schema_version == mark.schema_version
        });
        mark.floor > 0 && known.is_none_or(|held| held.file_uuid != mark.file_uuid)
    }

    fn adopt(&mut self, batches: Vec<Batch>, report: &mut MergeReport) -> Result<()> {
        let mut named: BTreeMap<usize, BTreeSet<crate::Uuid>> = BTreeMap::new();
        for batch in &batches {
            named
                .entry(batch.table)
                .or_default()
                .extend(batch.rows.iter().map(|row| row.id));
        }
        let named: BTreeMap<usize, Vec<crate::Uuid>> = named
            .into_iter()
            .map(|(table, ids)| (table, ids.into_iter().collect()))
            .collect();

        let mut before = Vec::new();
        for (table, ids) in &named {
            before.extend(self.tables()[*table].conflicts_among(ids)?);
        }
        let clock = self.clock().clone();
        self.state().view.watch();

        let mut order: Vec<(Dot, usize, usize)> = Vec::new();
        for (at, batch) in batches.iter().enumerate() {
            for (row, held) in batch.rows.iter().enumerate() {
                order.push((held.version.dot(), at, row));
            }
        }
        order.sort_unstable();
        let mut floors: HashMap<(usize, WriterId), u64> = HashMap::new();
        let entities: Vec<u64> = self
            .tables()
            .iter()
            .map(|table| fnv1a(table.entity().as_bytes()))
            .collect();
        let marks = self.state().marks.iter();
        for mark in marks.chain(batches.iter().filter_map(|batch| batch.mark.as_ref())) {
            if let Some(table) = entities.iter().position(|entity| *entity == mark.entity) {
                let floor = floors.entry((table, mark.writer)).or_default();
                *floor = (*floor).max(mark.floor);
            }
        }
        let mut taken = BTreeSet::new();
        let mut stalled: Vec<Option<usize>> = vec![None; batches.len()];
        // Purges go first: none waits for anything, so a batch stalled ahead of one cannot hold it.
        let mut purges = BTreeSet::new();
        for (dot, at, row) in &order {
            let batch = &batches[*at];
            if !batch.rows[*row].version.is_purged() {
                continue;
            }
            match self.tables_mut()[batch.table].adopt_row(&batch.rows[*row], &|_| true) {
                Ok(Adoption::Taken) => {
                    report.taken += 1;
                    taken.insert((batch.table, batch.rows[*row].id));
                }
                Ok(_) => report.known += 1,
                // That revision is never taken; the rows around it are.
                Err(err) if err.is_fork() => report.forked.push(dot.writer()),
                Err(err) => return Err(err),
            }
            self.state_mut().clock.observe(*dot);
            purges.insert((*at, *row));
        }
        for (dot, at, row) in order {
            if purges.contains(&(at, row)) {
                continue;
            }
            if stalled[at].is_some() {
                report.waiting += 1;
                continue;
            }
            let batch = &batches[at];
            let table = batch.table;
            let gone = |dot: Dot| {
                floors
                    .get(&(table, dot.writer()))
                    .is_some_and(|floor| dot.counter() <= *floor)
            };
            match self.tables_mut()[table].adopt_row(&batch.rows[row], &gone) {
                Ok(Adoption::Taken) => {
                    report.taken += 1;
                    taken.insert((table, batch.rows[row].id));
                }
                Ok(Adoption::Known) => report.known += 1,
                Ok(Adoption::Waiting) => {
                    stalled[at] = Some(row);
                    report.waiting += 1;
                    continue;
                }
                // Not this store's damage to fail over: that revision is never
                // taken, the rest still merges.
                Err(err) if err.is_fork() => {
                    report.forked.push(dot.writer());
                    continue;
                }
                Err(err) => return Err(err),
            }
            self.state_mut().clock.observe(dot);
        }
        // What a writer dropped from its shard is dropped here too.
        for batch in &batches {
            let Some(mark) = batch.mark else {
                continue;
            };
            if !self.may_lack_rows(batch) || !self.tables()[batch.table].published() {
                continue;
            }
            let held: BTreeSet<Dot> = batch.rows.iter().map(|row| row.version.dot()).collect();
            let gone: BTreeSet<usize> = self.tables()[batch.table]
                .rows_dropped_by(mark.writer, mark.floor, &|dot| held.contains(&dot))?
                .into_iter()
                .collect();
            if !gone.is_empty() {
                self.tables_mut()[batch.table].drop_rows(&gone)?;
                if let Some(settled) = self.state_mut().purges_settled.get_mut(batch.table) {
                    *settled = 0;
                }
            }
        }
        for (table, ids) in &named {
            self.tables()[*table].refresh_rows(ids)?;
        }
        self.audit_dots()?;
        for (batch, stalled) in batches.into_iter().zip(stalled) {
            let Some(mut mark) = batch.mark else {
                continue;
            };
            if let Some(row) = stalled {
                mark.rows = (batch.first + row) as u64;
                mark.generation = 0;
            }
            self.state_mut().mark(mark);
        }

        let mut after = Vec::new();
        for (table, ids) in &named {
            after.extend(self.tables()[*table].conflicts_among(ids)?);
        }
        after.sort();
        after.dedup();
        for conflict in after {
            if before.contains(&conflict) {
                continue;
            }
            match conflict {
                Conflict::Concurrent(c) => report.concurrent.push(c),
                Conflict::Combined(c) => report.combined.push(c),
                Conflict::Reference(r) => report.references.push(r),
            }
        }
        let mut changed = BTreeMap::new();
        for change in self.state().view.watched() {
            changed
                .entry((change.entity, change.id))
                .or_insert(change.before);
        }
        for ((entity, id), before) in changed {
            let Some(after) = self.state().view.status(entity, id)? else {
                continue;
            };
            if before.as_ref() == Some(&after) {
                continue;
            }
            if let RowStatus::Suppressed(reasons) = &after {
                let known = match &before {
                    Some(RowStatus::Suppressed(known)) => known.as_slice(),
                    _ => &[],
                };
                for reason in reasons {
                    if !known.contains(reason) {
                        report.references.push(*reason);
                    }
                }
            }
            report.visibility.push(VisibilityChange {
                entity,
                id,
                before,
                after,
            });
        }
        report.references.sort();
        report.references.dedup();
        report.forked.sort();
        report.forked.dedup();
        let tables = self.tables();
        report.adopted.extend(
            taken
                .into_iter()
                .map(|(table, id)| (tables[table].entity(), id)),
        );
        report.adopted.sort_unstable();
        report.clock_changed = *self.clock() != clock;
        Ok(())
    }

    fn adopt_converted(
        &mut self,
        batches: Vec<Batch>,
        marks: Vec<ShardMark>,
        told: &VersionVector,
        report: &mut MergeReport,
    ) -> Result<()> {
        let _lease = self.state().attached()?.1.lease()?;
        self.apply(batches, report)?;
        if !self.clock().covers(told) {
            self.state_mut().clock.absorb(told);
            report.clock_changed = true;
        }
        for mark in marks {
            self.state_mut().mark(mark);
        }
        self.write_cache(WriterId::ANONYMOUS, false)
    }

    fn write_cache(&mut self, writer_id: WriterId, settled: bool) -> Result<()> {
        for table in self.tables_mut() {
            table.fold_indexes(false)?;
        }
        let (held, layout) = self.state().attached()?;
        let cache = layout.cache(Self::SCHEMA_VERSION);
        let current = match ColumnFile::read_commit_token(&cache) {
            Ok(found) if found == held.commit_token() => held.clone(),
            _ => ColumnFile::open(&cache)?,
        };
        let mut updater = current.update(writer_id)?;
        let due = self
            .state()
            .flushed_at
            .is_none_or(|at| at.elapsed() >= UNFLUSHED_FOR);
        let flush = match settled || due {
            true => Flush::Ordered,
            false => Flush::Never,
        };
        updater.flushing(flush);
        updater.lasting(self.durability());
        self.state_mut().unflushed_from = match flush {
            Flush::Never => Some(
                self.state()
                    .unflushed_from
                    .map_or(updater.starts_at(), |from| from.min(updater.starts_at())),
            ),
            _ => None,
        };
        self.offer_to(&mut updater)?;
        let published = updater.commit_file()?;
        let written = published.file_bytes()?;
        self.settle_onto(published, flush != Flush::Never)?;
        match self.state().unflushed_from {
            Some(from) if written.saturating_sub(from) > UNFLUSHED_BYTES => {
                self.write_cache(writer_id, true)
            }
            _ => Ok(()),
        }
    }

    fn settle_onto(&mut self, published: ColumnFile, flushed: bool) -> Result<()> {
        for table in self.tables_mut() {
            table.rebase_columns(&published)?;
        }
        self.state().view.rebase(&published)?;
        let state = self.state_mut();
        if flushed {
            state.flushed_at = Some(std::time::Instant::now());
        }
        state.dots = published.column(CLOCK)?;
        state.held_marks = published.column(SHARDS)?;
        state.schema_bytes = published.column(SCHEMA)?;
        state.dropped.clear();
        state.file = Some(published);
        Ok(())
    }

    /// Writes the cache as a file of its own, so that no chunk written
    /// before stays behind in it. Chunks nobody changed are copied undecoded.
    fn replace_cache(&mut self) -> Result<()> {
        for table in self.tables_mut() {
            table.fold_indexes(false)?;
        }
        let (held, layout) = self.state().attached()?;
        let cache = layout.cache(Self::SCHEMA_VERSION);
        let current = match ColumnFile::read_commit_token(&cache) {
            Ok(found) if found == held.commit_token() => held.clone(),
            _ => ColumnFile::open(&cache)?,
        };
        let published = current.replace(|updater| self.offer_to(updater))?;
        self.state_mut().unflushed_from = None;
        self.settle_onto(published, true)
    }

    fn flush_outbox(&mut self) -> Result<()> {
        let layout = self.state().attached()?.1.clone();
        let schema = self.schema_metadata().encode()?;
        while let Some(next) = self.state().outbox.first().cloned() {
            let (entity, fields) = {
                let tables = self.tables();
                let table = tables[next.table];
                (table.entity(), table.field_names())
            };
            let found = ShardFile {
                entity: entity.to_owned(),
                writer: next.writer,
                schema_version: Self::SCHEMA_VERSION,
                path: layout.shard(entity, next.writer, Self::SCHEMA_VERSION),
            };
            let (token, rows) = shard::append(
                &found.path,
                next.writer,
                &schema,
                &fields,
                &next.rows,
                self.durability(),
            )?;
            let floor = self
                .state()
                .marks
                .iter()
                .find(|mark| mark.names(&found))
                .map_or(0, |mark| mark.floor);
            let state = self.state_mut();
            state.mark(ShardMark::of(&found, token, rows, floor));
            state.outbox.remove(0);
            #[cfg(test)]
            if CUT_AFTER_APPEND.replace(false) {
                return Err(Error::invalid("injected cut between appends"));
            }
        }
        Ok(())
    }

    fn commit(&mut self, writer_id: WriterId) -> Result<u64> {
        self.check_mutable()?;
        if writer_id.is_anonymous() {
            return Err(Error::invalid(
                "a commit names the writer whose shards it appends to",
            ));
        }
        let _lease = self.state().attached()?.1.lease()?;
        self.commit_held(writer_id)
    }

    fn commit_held(&mut self, writer_id: WriterId) -> Result<u64> {
        let mut report = MergeReport::default();
        let own = self.collect(|writer| writer == writer_id, &mut report)?;

        let mut clock = self.clock().clone();
        for row in own.iter().flat_map(|batch| &batch.rows) {
            clock.observe(row.version.dot());
        }
        let mut outgoing = Vec::new();
        for (table, held) in self.tables_mut().into_iter().enumerate() {
            let rows = held.publish_rows(&mut clock, writer_id)?;
            if !rows.is_empty() {
                outgoing.push(Outgoing {
                    table,
                    writer: writer_id,
                    rows,
                });
            }
        }
        let state = self.state_mut();
        state.clock = clock;
        state.outbox.extend(outgoing);
        self.apply(own, &mut report)?;
        self.stash_outbox()?;
        self.flush_outbox()?;
        self.drop_stash()?;
        self.say_what_was_seen(writer_id, false)?;
        self.write_cache(writer_id, false)?;
        self.auto_compact();
        let policy = self.compaction_policy();
        if policy != crate::CompactionPolicy::Disabled
            && let Err(err) = self.rewrite_shards(writer_id, Some(policy))
        {
            self.state_mut().maintenance = Some(crate::MaintenanceOutcome::Failed(err.to_string()));
        }
        self.generation()
            .ok_or_else(|| Error::invalid("commit did not attach a generation"))
    }

    fn peers(&self) -> Result<Vec<Peer>> {
        peers::read_all(&self.state().placed()?.shared)
    }

    fn acknowledge(&mut self, writer_id: WriterId) -> Result<()> {
        self.check_published()?;
        let _lease = self.state().attached()?.1.lease()?;
        self.say_what_was_seen(writer_id, true)
    }

    fn say_what_was_seen(&mut self, writer_id: WriterId, always: bool) -> Result<()> {
        if writer_id.is_anonymous() {
            return Err(Error::invalid("only a named writer says what it has seen"));
        }
        let shared = self.state().placed()?.shared.clone();
        let said = match &self.state().said {
            Some(said) if said.writer == writer_id => Some(said.clone()),
            _ => peers::read(&shared, writer_id)?,
        };
        let news = match &said {
            None => true,
            Some(said) => self
                .clock()
                .dots()
                .iter()
                .any(|dot| dot.writer() != writer_id && !said.seen.dominates(*dot)),
        };
        if !news && !always {
            self.state_mut().said = said;
            return Ok(());
        }
        let now = Peer {
            writer: writer_id,
            seen: self.clock().clone(),
        };
        if said.as_ref() != Some(&now) {
            peers::publish(&shared, &now, self.durability())?;
        }
        self.state_mut().said = Some(now);
        Ok(())
    }

    fn compact_shards(&mut self, writer_id: WriterId) -> Result<crate::CompactionReport> {
        self.check_published()?;
        let _lease = self.state().attached()?.1.lease()?;
        self.rewrite_shards(writer_id, None)
    }

    fn rewrite_shards(
        &mut self,
        writer_id: WriterId,
        policy: Option<crate::CompactionPolicy>,
    ) -> Result<crate::CompactionReport> {
        let layout = self.state().attached()?.1.clone();
        let mut report = crate::CompactionReport {
            before_bytes: 0,
            after_bytes: 0,
        };
        let mut marks = Vec::new();
        for table in self.tables() {
            let found = ShardFile {
                entity: table.entity().to_owned(),
                writer: writer_id,
                schema_version: Self::SCHEMA_VERSION,
                path: layout.shard(table.entity(), writer_id, Self::SCHEMA_VERSION),
            };
            let file = match ColumnFile::open(&found.path) {
                Ok(file) => file,
                Err(Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            let before = file.file_bytes()?;
            if let Some(policy) = policy {
                let small = matches!(
                    policy,
                    crate::CompactionPolicy::Automatic { min_reclaim_bytes, .. }
                        if before < min_reclaim_bytes
                );
                if small || !policy.should_compact(before, file.obsolete_bytes()?) {
                    continue;
                }
            }
            drop(file);
            let held = shard::read(&found.path, 0)?;
            let mut fields: Vec<&str> = table.field_names();
            for (name, _) in held.read.iter().flat_map(|row| &row.values) {
                if !fields.contains(&name.as_str()) {
                    fields.push(name);
                }
            }
            let (token, rows) = shard::rewrite(
                &found.path,
                writer_id,
                &held.schema,
                &fields,
                &held.read,
                held.floor,
                self.durability(),
            )?;
            report.before_bytes += before;
            report.after_bytes += std::fs::metadata(&found.path)?.len();
            marks.push(ShardMark::of(&found, token, rows, held.floor));
        }
        if !marks.is_empty() {
            for mark in marks {
                self.state_mut().mark(mark);
            }
            self.write_cache(writer_id, false)?;
        }
        Ok(report)
    }

    fn prune(&mut self, writer_id: WriterId, others: &[WriterId]) -> Result<PruneReport> {
        self.check_published()?;
        if writer_id.is_anonymous() {
            return Err(Error::invalid("only a named writer prunes its own shards"));
        }
        if !self.state().outbox.is_empty() {
            return Err(Error::invalid("commit pending changes before pruning"));
        }
        let layout = self.state().attached()?.1.clone();
        let _lease = layout.lease()?;
        let mut report = PruneReport::default();

        let told = peers::read_all(&layout.shared)?;
        let mut heard = Vec::new();
        for other in others.iter().copied().filter(|other| *other != writer_id) {
            match told.iter().find(|peer| peer.writer == other) {
                Some(peer) if self.clock().counter_of(other) >= peer.seen.counter_of(other) => {
                    heard.push(peer.seen.clone());
                }
                _ => report.waiting_for.push(other),
            }
        }
        report.waiting_for.sort();
        report.waiting_for.dedup();
        let settled = |dot: Dot| heard.iter().all(|seen| seen.dominates(dot));

        let ours = self.schema_metadata();
        let mut superseded = vec![Vec::new(); self.tables().len()];
        for (at, table) in self.tables().into_iter().enumerate() {
            if table.keeps_history() || !report.waiting_for.is_empty() {
                continue;
            }
            let fields = table.field_names();
            let mut complete: HashMap<WriterId, bool> = HashMap::new();
            let whole = |author: WriterId| {
                let path = layout.shard(table.entity(), author, Self::SCHEMA_VERSION);
                shard::schema_of(&path)
                    .and_then(|bytes| super::SchemaMetadata::decode(&bytes))
                    .and_then(|written| {
                        let mut whole = ours.can_read(&written)?;
                        for field in &fields {
                            whole &= written.names(table.entity(), field)?;
                        }
                        Ok(whole)
                    })
                    .unwrap_or(false)
            };
            let known = std::cell::RefCell::new(&mut complete);
            let asked = |author: WriterId| {
                *known
                    .borrow_mut()
                    .entry(author)
                    .or_insert_with(|| whole(author))
            };
            superseded[at] = table.prunable_rows(writer_id, &settled, &asked)?;
        }
        report.dropped = self.rewrite_without(writer_id, &layout, superseded)?;
        Ok(report)
    }

    // Drops `superseded` and whatever a purge outranked: from this writer's shards what it
    // wrote, from memory and the cache every such row. Only tables that lose a row are read.
    fn rewrite_without(
        &mut self,
        writer_id: WriterId,
        layout: &Layout,
        superseded: Vec<Vec<usize>>,
    ) -> Result<usize> {
        let mut dropped = 0;
        let mut stale = false;
        let mut rewrites = Vec::new();
        let mut losing = Vec::new();
        for (at, (table, mut gone)) in self.tables().into_iter().zip(superseded).enumerate() {
            let purged = table.purged_payload_rows()?;
            stale |= !purged.is_empty();
            gone.extend(purged);
            if gone.is_empty() {
                continue;
            }
            let gone: BTreeSet<usize> = gone.into_iter().collect();
            let found = ShardFile {
                entity: table.entity().to_owned(),
                writer: writer_id,
                schema_version: Self::SCHEMA_VERSION,
                path: layout.shard(table.entity(), writer_id, Self::SCHEMA_VERSION),
            };
            let mut floor = self
                .state()
                .marks
                .iter()
                .find(|mark| mark.names(&found))
                .map_or(0, |mark| mark.floor);
            let mut kept = Vec::new();
            let authored = table.authored_rows(writer_id)?;
            let held = authored.len();
            for (row, held) in authored {
                match gone.contains(&row) {
                    true => floor = floor.max(held.version.dot().counter()),
                    false => kept.push(held),
                }
            }
            if kept.len() < held {
                dropped += held - kept.len();
                rewrites.push((found, table.field_names(), kept, floor));
            }
            losing.push((at, gone));
        }
        if rewrites.is_empty() && !stale {
            return Ok(dropped);
        }

        // Nothing is held of a drop that fails: a shard already rewritten is read again.
        let mut next = self.snapshot();
        for (at, gone) in &losing {
            next.tables_mut()[*at].drop_rows(gone)?;
            if let Some(settled) = next.state_mut().purges_settled.get_mut(*at) {
                *settled = 0;
            }
        }
        let schema = self.schema_metadata().encode()?;
        for (found, fields, kept, floor) in rewrites {
            let (token, rows) = shard::rewrite(
                &found.path,
                writer_id,
                &schema,
                &fields,
                &kept,
                floor,
                self.durability(),
            )?;
            next.state_mut()
                .mark(ShardMark::of(&found, token, rows, floor));
            #[cfg(test)]
            if CUT_AFTER_REWRITE.replace(false) {
                return Err(Error::invalid("injected cut between rewrites"));
            }
        }
        // A chunk written over stays in its file; what a purge shed must not.
        match stale {
            true => next.replace_cache()?,
            false => next.write_cache(writer_id, true)?,
        }
        *self = next;
        Ok(dropped)
    }

    fn purge_cascading(&mut self, entity: &str, id: crate::Uuid) -> Result<usize> {
        self.check_mutable()?;
        let Some(at) = self
            .tables()
            .iter()
            .position(|table| table.entity() == entity)
        else {
            return Err(Error::invalid(format!("this store holds no {entity}")));
        };
        let entity = self.tables()[at].entity();
        let purged = usize::from(self.tables_mut()[at].purge_one(id)?);
        Ok(purged + self.purge_dependants(vec![(entity, id)])?)
    }

    fn purge_dependants(&mut self, mut walk: Vec<(&'static str, crate::Uuid)>) -> Result<usize> {
        let cascading: BTreeSet<&'static str> = self
            .tables()
            .iter()
            .flat_map(|table| table.cascading_columns())
            .collect();
        let mut reached: BTreeSet<_> = walk.iter().copied().collect();
        let mut purged = 0;
        while let Some((entity, id)) = walk.pop() {
            for (child, column, held) in self.state().view.naming(entity, id)? {
                if !cascading.contains(column) || !reached.insert((child, held)) {
                    continue;
                }
                if let Some(at) = self
                    .tables()
                    .iter()
                    .position(|table| table.entity() == child)
                {
                    purged += usize::from(self.tables_mut()[at].purge_one(held)?);
                    walk.push((child, held));
                }
            }
        }
        Ok(purged)
    }

    fn reclaim_purged(&mut self, writer_id: WriterId) -> Result<PurgeReport> {
        self.check_published()?;
        if writer_id.is_anonymous() {
            return Err(Error::invalid(
                "only a named writer reclaims from its own shards",
            ));
        }
        if !self.state().outbox.is_empty() {
            return Err(Error::invalid("commit pending changes before reclaiming"));
        }
        let layout = self.state().attached()?.1.clone();
        let _lease = layout.lease()?;
        let mut report = PurgeReport::default();
        let settled = &self.state().purges_settled;
        let mut news = false;
        for (at, table) in self.tables().into_iter().enumerate() {
            news = news || table.has_purge_news(settled.get(at).copied().unwrap_or(0))?;
        }
        if !news {
            return self.settle_purges(report);
        }

        let mut purged = Vec::new();
        for table in self.tables() {
            for id in table.purged_identities()? {
                purged.push((table.entity(), id));
            }
        }
        report.purged = self.purge_dependants(purged)?;
        if report.purged > 0 {
            self.commit_held(writer_id)?;
        }
        let untouched = vec![Vec::new(); self.tables().len()];
        report.dropped = self.rewrite_without(writer_id, &layout, untouched)?;
        self.settle_purges(report)
    }

    /// Drops the payload of purged rows from the shards of `retired`, a
    /// writer that will never do it itself. Returns the rows dropped.
    ///
    /// A shard has one writer; this stands in for one that is gone for good.
    /// Callers see to it that a single device does so for any writer, since
    /// two rewrites of one file are a conflict wherever files are synced.
    fn scrub_purged(&mut self, writer_id: WriterId, retired: WriterId) -> Result<usize> {
        if retired.is_anonymous() || retired == writer_id {
            return Err(Error::invalid("a scrub names another writer's shards"));
        }
        // Sheds the same rows from memory and the cache first.
        self.reclaim_purged(writer_id)?;
        let layout = self.state().attached()?.1.clone();
        let _lease = layout.lease()?;
        let mut dropped = 0;
        let mut marks = Vec::new();
        for table in self.tables() {
            let purged: BTreeSet<crate::Uuid> = table.purged_identities()?.into_iter().collect();
            if purged.is_empty() {
                continue;
            }
            let found = ShardFile {
                entity: table.entity().to_owned(),
                writer: retired,
                schema_version: Self::SCHEMA_VERSION,
                path: layout.shard(table.entity(), retired, Self::SCHEMA_VERSION),
            };
            let held = match shard::read(&found.path, 0) {
                Ok(held) => held,
                Err(Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            let mut floor = held.floor;
            let mut kept = Vec::with_capacity(held.read.len());
            for row in &held.read {
                if purged.contains(&row.id) && !row.version.is_purged() {
                    floor = floor.max(row.version.dot().counter());
                } else {
                    kept.push(row.clone());
                }
            }
            if kept.len() == held.read.len() {
                continue;
            }
            dropped += held.read.len() - kept.len();
            let mut fields: Vec<&str> = table.field_names();
            for (name, _) in held.read.iter().flat_map(|row| &row.values) {
                if !fields.contains(&name.as_str()) {
                    fields.push(name);
                }
            }
            let (token, rows) = shard::rewrite(
                &found.path,
                retired,
                &held.schema,
                &fields,
                &kept,
                floor,
                self.durability(),
            )?;
            marks.push(ShardMark::of(&found, token, rows, floor));
        }
        if !marks.is_empty() {
            for mark in marks {
                self.state_mut().mark(mark);
            }
            self.write_cache(writer_id, false)?;
        }
        Ok(dropped)
    }

    /// Compares what `writer_id` published with what its shards hold now.
    ///
    /// A shard is one file, and where files are synced a file can be
    /// replaced whole by a copy from elsewhere. If that copy merely lacks
    /// rows written here since, as one another device rewrote with
    /// [`Store::scrub_purged`] does, they are appended again; rows at or under
    /// a shard's floor were dropped on purpose and stay dropped. If it holds
    /// rows of `writer_id` this store never wrote, another store is writing
    /// as `writer_id` too, and the caller had better become another writer;
    /// if one of them sits under a dot this store used, nothing is appended.
    fn check_own_shards(&mut self, writer_id: WriterId) -> Result<super::OwnShards> {
        self.check_published()?;
        if writer_id.is_anonymous() {
            return Err(Error::invalid("only a named writer has shards of its own"));
        }
        let layout = self.state().attached()?.1.clone();
        let _lease = layout.lease()?;
        let schema = self.schema_metadata().encode()?;
        let mut report = super::OwnShards::default();
        let mut appends = Vec::new();
        for table in self.tables() {
            let found = ShardFile {
                entity: table.entity().to_owned(),
                writer: writer_id,
                schema_version: Self::SCHEMA_VERSION,
                path: layout.shard(table.entity(), writer_id, Self::SCHEMA_VERSION),
            };
            let token = match shard::token_of(&found.path) {
                Ok(token) => Some(token),
                Err(Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => None,
                // Mid-delivery; the next call sees it whole.
                Err(Error::Io(_) | Error::Corrupt { .. }) => continue,
                Err(err) => return Err(err),
            };
            let mark = self.state().marks.iter().find(|mark| mark.names(&found));
            // As this store left it: nothing can differ.
            if let (Some(token), Some(mark)) = (token, mark)
                && mark.is_at(token)
            {
                continue;
            }
            let dropped = mark.map_or(0, |mark| mark.floor);
            let (held, floor) = match token {
                None => (BTreeMap::new(), 0),
                Some(_) => match shard::read(&found.path, 0) {
                    Ok(read) => (
                        read.read
                            .into_iter()
                            .map(|row| (row.version.dot(), row))
                            .collect::<BTreeMap<Dot, ShardRow>>(),
                        read.floor,
                    ),
                    Err(Error::Io(_) | Error::Corrupt { .. }) => continue,
                    Err(err) => return Err(err),
                },
            };
            let mut missing = Vec::new();
            let mut collided = false;
            for (_, row) in table.authored_rows(writer_id)? {
                let dot = row.version.dot();
                match held.get(&dot) {
                    Some(there) if *there != row => collided = true,
                    Some(_) => {}
                    None if dot.counter() > floor => missing.push(row),
                    None => {}
                }
            }
            // Rows this store dropped may come back in an older copy of the
            // file; anything newer under its name it would know.
            for dot in held.keys() {
                if dot.writer() == writer_id && dot.counter() > dropped && !table.holds_dot(*dot)? {
                    report.forked = true;
                }
            }
            if collided {
                report.forked = true;
                report.collided = true;
            }
            if !missing.is_empty() {
                appends.push((found.path, table.field_names(), missing));
            }
        }
        if report.collided {
            return Ok(report);
        }
        for (path, fields, missing) in appends {
            report.restored += missing.len();
            shard::append(
                &path,
                writer_id,
                &schema,
                &fields,
                &missing,
                self.durability(),
            )?;
        }
        Ok(report)
    }

    fn settle_purges(&mut self, report: PurgeReport) -> Result<PurgeReport> {
        let rows = self
            .tables()
            .iter()
            .map(|table| table.row_count())
            .collect();
        self.state_mut().purges_settled = rows;
        Ok(report)
    }

    fn refresh_peers(&mut self) -> Result<MergeReport> {
        self.check_published()?;
        let _lease = self.state().attached()?.1.lease()?;
        self.refresh_held()
    }

    fn refresh_held(&mut self) -> Result<MergeReport> {
        let mut report = MergeReport::default();
        let batches = self.collect(|_| true, &mut report)?;
        let moved = batches.iter().any(|batch| batch.mark.is_some());
        self.apply(batches, &mut report)?;
        if moved {
            self.write_cache(WriterId::ANONYMOUS, false)?;
        }
        Ok(report)
    }

    fn merge_from(
        &mut self,
        source: impl Into<Layout>,
        writer_id: WriterId,
    ) -> Result<MergeReport> {
        self.check_mutable()?;
        if writer_id.is_anonymous() {
            return Err(Error::invalid(
                "a merge names the writer whose shards stay its own",
            ));
        }
        let source = source.into();
        let layout = self.state().attached()?.1.clone();
        let _lease = layout.lease()?;
        self.commit_held(writer_id)?;
        for entity in shard::entities(&source.shared, &source.local)? {
            let (theirs, _) = shard::discover(&source.shared, &entity)?;
            for found in theirs {
                if found.writer == writer_id {
                    continue;
                }
                let held = layout.shard(&found.entity, found.writer, found.schema_version);
                let Ok(told) = shard::token_of(&found.path) else {
                    continue;
                };
                let newer = match shard::token_of(&held) {
                    Ok(ours) if ours.file_uuid == told.file_uuid => {
                        ours.generation < told.generation
                    }
                    Ok(_) => {
                        shard::epoch_of(&found.path).unwrap_or(0)
                            > shard::epoch_of(&held).unwrap_or(0)
                    }
                    Err(Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => true,
                    Err(_) => true,
                };
                if !newer {
                    continue;
                }
                let folder = held
                    .parent()
                    .ok_or_else(|| Error::invalid("a shard has a table directory"))?;
                std::fs::create_dir_all(folder)?;
                let arriving = folder.join(format!(".dbsync-{}.tmp", crate::Uuid::now_v7()));
                std::fs::copy(&found.path, &arriving)?;
                std::fs::rename(&arriving, &held)?;
            }
        }
        self.refresh_held()
    }
}

#[cfg(test)]
#[path = "store_test.rs"]
mod store_test;
