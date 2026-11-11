#![cfg(feature = "macros")]
mod support;

use dbsync::schema::{Store, Table};
use dbsync::{CompactionPolicy, MaintenanceOutcome, Result, Uuid, WriterId};

#[dbsync::entity]
#[merge(history, with = choose)]
struct Note {
    #[index]
    title: String,
    #[index]
    optional: Option<String>,
    number: u64,
}
fn choose(input: &dbsync::schema::MergeInput<'_, NoteValue>) -> NoteValue {
    input
        .candidates
        .iter()
        .max_by_key(|c| c.version.dot())
        .unwrap()
        .value
        .clone()
}
#[dbsync::container(schema = "database", version = 1)]
struct Database {
    notes: Set<Note>,
}
struct Directory(std::path::PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("dbsync-storage-{}", Uuid::now_v7()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn writer(n: u64) -> WriterId {
    WriterId::from_raw(n)
}
fn records(db: &Database) -> Result<Vec<dbsync::schema::RevisionRecord>> {
    db.tables()
        .into_iter()
        .map(|t| t.revision_records())
        .collect::<Result<Vec<_>>>()
        .map(|v| v.into_iter().flatten().collect())
}

#[test]
fn compaction_preserves_history_indexes_resolvers_and_bidirectional_merge() -> Result<()> {
    let dir = Directory::new();
    let path = dir.file("local");
    let mut local = Database::create(&path, writer(1))?;
    let id = local.notes.push(
        "a long shared value that lives on the heap",
        Some("another long nullable value on the heap"),
        0,
    )?;
    local.commit(writer(1))?;
    support::copy_store(&path, dir.file("peer"))?;
    let mut peer = Database::open(dir.file("peer"))?;
    for n in 1..15 {
        local.notes.set_number(id, n)?;
        local.commit(writer(1))?;
    }
    peer.notes
        .set_title(id, "a different long title from the remote replica")?;
    peer.commit(writer(2))?;
    local.merge_from(dir.file("peer"), writer(1))?;
    let snapshot = local.snapshot();
    let before = records(&local)?;
    let conflicts = local.conflicts()?;
    let statuses = local.statuses()?;
    let clock = local.clock().clone();
    let rows = local.notes.rows();
    let report = local.compact()?;
    assert!(report.after_bytes <= report.before_bytes);
    assert_eq!(records(&local)?, before);
    assert_eq!(records(&snapshot)?, before);
    assert_eq!(local.notes.rows(), rows);
    assert_eq!(local.conflicts()?, conflicts);
    assert_eq!(local.statuses()?, statuses);
    assert_eq!(*local.clock(), clock);
    local.verify()?;
    peer.merge_from(&path, writer(2))?;
    peer.compact()?;
    local.merge_from(dir.file("peer"), writer(1))?;
    local.verify()?;
    assert_eq!(records(&local)?, before);
    local.remove(id)?;
    local.commit(writer(1))?;
    let deleted = records(&local)?;
    local.compact()?;
    assert_eq!(records(&local)?, deleted);
    assert_eq!(local.status(id)?, dbsync::schema::RowStatus::Deleted);
    local.notes.rows_by_dot()?;
    Ok(())
}

#[test]
fn automatic_compaction_is_configurable_and_explicit_compaction_rejects_pending_edits() -> Result<()>
{
    let dir = Directory::new();
    let mut db = Database::create(dir.file("db"), writer(1))?;
    assert!(
        db.set_compaction_policy(CompactionPolicy::Automatic {
            min_reclaim_bytes: 0,
            min_reclaim_fraction: f64::NAN
        })
        .is_err()
    );
    db.set_compaction_policy(CompactionPolicy::Automatic {
        min_reclaim_bytes: 0,
        min_reclaim_fraction: 0.0,
    })?;
    let id = db.notes.push("first", None, 1)?;
    assert!(db.compact().is_err());
    db.commit(writer(1))?;
    assert!(matches!(
        db.maintenance_outcome(),
        Some(MaintenanceOutcome::Compacted(_))
    ));
    let policy = db.compaction_policy();
    db.notes.set_title(id, "second")?;
    assert!(db.compact().is_err());
    db.commit(writer(1))?;
    assert_eq!(db.compaction_policy(), policy);
    assert_eq!(db.notes.title(id)?.as_ref(), "second");
    db.verify()?;
    Ok(())
}
