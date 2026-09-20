#![cfg(feature = "macros")]

use dbsync::schema::{MergeInput, Store, Table};
use dbsync::{Result, Uuid, WriterId};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[dbsync::entity]
struct Folder {
    name: String,
}

#[dbsync::entity]
#[merge(history, with = combine)]
struct Note {
    #[index]
    title: String,
    #[index]
    score: u32,
    folder: One<Folder>,
}

fn combine(input: &MergeInput<'_, NoteValue>) -> NoteValue {
    let first = input
        .candidates
        .iter()
        .min_by_key(|row| &row.value.title)
        .unwrap();
    let title = input
        .candidates
        .iter()
        .map(|row| row.value.title.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join("+");
    NoteValue {
        title,
        score: input.candidates.iter().map(|row| row.value.score).sum(),
        folder: first.value.folder,
    }
}

#[dbsync::container]
struct Database {
    folders: Set<Folder>,
    notes: Set<Note>,
}

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("dbsync-automatic-{}", Uuid::now_v7()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
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

#[test]
fn one_call_combines_pending_edits_and_persists_the_derived_row() -> Result<()> {
    let dir = Directory::new();
    let mut local = Database::create(dir.path("local"), writer(1))?;
    let a = local.folders.push("A")?;
    let b = local.folders.push("B")?;
    let id = local.notes.push("base", 0, a)?;
    local.commit(writer(1))?;
    std::fs::copy(dir.path("local"), dir.path("peer"))?;
    let mut peer = Database::open(dir.path("peer"))?;
    local.notes.set_title(id, "alpha")?;
    local.notes.set_score(id, 2)?;
    peer.notes.set_title(id, "beta")?;
    peer.notes.set_score(id, 3)?;
    peer.notes.set_folder(id, b)?;
    peer.commit(writer(2))?;
    let report = local.merge_from(dir.path("peer"), writer(1))?;
    assert_eq!(report.combined.len(), 1);
    assert_eq!(local.at(id)?.title()?, "alpha+beta");
    assert_eq!(local.at(id)?.score()?, 5);
    assert_eq!(local.at(id)?.sources()?.len(), 2);
    assert!(local.at(id)?.version().is_err());
    for source in local.at(id)?.sources()? {
        let original = local.at_version(id, source)?;
        assert!(!original.version()?.is_current());
        assert!(matches!(original.title()?.as_ref(), "alpha" | "beta"));
    }
    assert_eq!(local.notes.find_by_title("alpha+beta")?, [id]);
    assert!(local.notes.find_by_title("alpha")?.is_empty());
    assert_eq!(local.notes.find_by_score(5)?, [id]);
    assert_eq!(local.children_of::<Folder, Note>(a)?, [id]);
    assert!(local.children_of::<Folder, Note>(b)?.is_empty());
    assert_eq!(local.at(id)?.parent_id::<Folder>()?, a);
    let reopened = Database::open(dir.path("local"))?;
    assert_eq!(reopened.at(id)?.title()?, "alpha+beta");
    let rows = local.notes.rows();
    let bytes = std::fs::metadata(dir.path("local"))?.len();
    let generation = local.generation();
    let again = local.merge_from(dir.path("peer"), writer(1))?;
    assert!(!again.changed_anything(), "{again:?}");
    assert_eq!(local.notes.rows(), rows);
    assert_eq!(local.generation(), generation);
    assert_eq!(std::fs::metadata(dir.path("local"))?.len(), bytes);
    local.notes.set_title(id, "edited")?;
    assert_eq!(local.notes.score(id)?, 5);
    local.commit(writer(1))?;
    assert_eq!(local.at(id)?.sources()?.len(), 1);
    assert_eq!(local.at(id)?.parents()?.len(), 2);
    assert_eq!(Database::open(dir.path("local"))?.notes.score(id)?, 5);
    Ok(())
}

#[test]
fn committing_a_stale_handle_merges_instead_of_losing_saved_changes() -> Result<()> {
    let dir = Directory::new();
    let mut a = Database::create(dir.path("db"), writer(1))?;
    let parent = a.folders.push("A")?;
    let note = a.notes.push("base", 0, parent)?;
    a.commit(writer(1))?;
    let mut b = Database::open(dir.path("db"))?;
    a.notes.set_title(note, "alpha")?;
    a.notes.set_score(note, 2)?;
    a.commit(writer(1))?;
    b.notes.set_title(note, "beta")?;
    b.notes.set_score(note, 3)?;
    b.commit(writer(1))?;
    assert_eq!(b.notes.title(note)?, "alpha+beta");
    assert_eq!(b.notes.score(note)?, 5);
    assert_eq!(Database::open(dir.path("db"))?.notes.score(note)?, 5);
    Ok(())
}

#[test]
fn repeated_values_with_distinct_uuids_are_independent() -> Result<()> {
    let mut db = Database::default();
    let parent = db.folders.push("A")?;
    let first = db.notes.push("same", 1, parent)?;
    let second = db.notes.push("same", 1, parent)?;
    assert_ne!(first, second);
    assert_eq!(db.notes.find_by_title("same")?, [first, second]);
    assert_eq!(db.children_of::<Folder, Note>(parent)?, [first, second]);
    db.audit()?;
    Ok(())
}

#[test]
fn deletion_precedes_the_custom_resolver_and_hidden_children_reappear() -> Result<()> {
    let dir = Directory::new();
    let mut a = Database::create(dir.path("a"), writer(1))?;
    let parent = a.folders.push("A")?;
    let note = a.notes.push("base", 0, parent)?;
    a.commit(writer(1))?;
    let original = a.at(note)?.version()?.dot();
    std::fs::copy(dir.path("a"), dir.path("b"))?;
    let mut b = Database::open(dir.path("b"))?;
    a.remove(note)?;
    b.notes.set_title(note, "edited")?;
    b.commit(writer(2))?;
    a.merge_from(dir.path("b"), writer(1))?;
    assert!(a.at(note).is_err());
    a.restore(note, original)?;
    a.commit(writer(1))?;
    assert_eq!(a.notes.title(note)?, "base");
    let parent_version = a.at(parent)?.version()?.dot();
    a.remove(parent)?;
    assert_eq!(a.all::<Note>().count(), 0);
    a.restore(parent, parent_version)?;
    assert_eq!(a.all::<Note>().count(), 1);
    Ok(())
}

#[test]
fn hidden_children_can_be_reparented_without_restoring_the_deleted_parent() -> Result<()> {
    let mut db = Database::default();
    let a = db.folders.push("A")?;
    let b = db.folders.push("B")?;
    let note = db.notes.push("note", 1, a)?;
    db.remove(a)?;
    assert!(db.at(note).is_err());
    db.at_hidden_mut(note)?.set_folder(b)?;
    assert_eq!(db.at(note)?.parent_id::<Folder>()?, b);
    assert_eq!(db.children_of::<Folder, Note>(b)?, [note]);
    Ok(())
}

mod ancestry {
    #[dbsync::entity]
    #[merge(with = combine)]
    pub struct Counter {
        number: u32,
    }

    fn combine(input: &dbsync::schema::MergeInput<'_, CounterValue>) -> CounterValue {
        CounterValue {
            number: input.ancestry.len() as u32,
        }
    }

    #[dbsync::container]
    pub struct Database {
        counters: Set<Counter>,
    }
}

#[test]
fn excluded_old_writes_do_not_change_the_resolvers_ancestry_or_cached_result() -> Result<()> {
    let dir = Directory::new();
    let mut a = ancestry::Database::create(dir.path("a"), writer(1))?;
    let id = a.counters.push(0)?;
    a.commit(writer(1))?;
    let original = a.at(id)?.version()?.dot();
    std::fs::copy(dir.path("a"), dir.path("old"))?;
    let mut old = ancestry::Database::open(dir.path("old"))?;
    a.remove(id)?;
    a.commit(writer(1))?;
    std::fs::copy(dir.path("a"), dir.path("b"))?;
    let mut b = ancestry::Database::open(dir.path("b"))?;
    a.restore(id, original)?;
    a.commit(writer(1))?;
    b.restore(id, original)?;
    b.commit(writer(2))?;
    a.merge_from(dir.path("b"), writer(1))?;
    assert_eq!(a.counters.number(id)?, 4);
    old.counters.set_number(id, 999)?;
    old.commit(writer(3))?;
    a.merge_from(dir.path("old"), writer(1))?;
    assert_eq!(a.counters.number(id)?, 4);
    assert_eq!(
        ancestry::Database::open(dir.path("a"))?
            .counters
            .number(id)?,
        4
    );
    assert_eq!(a.history(id)?.len(), 5);
    Ok(())
}

mod invalid {
    #[dbsync::entity]
    #[merge(with = combine)]
    pub struct Link {
        parent: One<Link>,
        number: u32,
    }

    fn combine(_: &dbsync::schema::MergeInput<'_, LinkValue>) -> LinkValue {
        LinkValue {
            parent: dbsync::Id::from_uuid(dbsync::Uuid::from_u128(99)),
            number: 0,
        }
    }

    #[dbsync::container]
    pub struct Database {
        links: Set<Link>,
    }
}

#[test]
fn invalid_resolver_references_leave_the_destination_and_pending_edits_intact() -> Result<()> {
    let dir = Directory::new();
    let mut a = invalid::Database::create(dir.path("a"), writer(1))?;
    let id = a.links.push(dbsync::Id::NIL, 0)?;
    a.commit(writer(1))?;
    std::fs::copy(dir.path("a"), dir.path("b"))?;
    let mut b = invalid::Database::open(dir.path("b"))?;
    a.links.set_number(id, 1)?;
    b.links.set_number(id, 2)?;
    b.commit(writer(2))?;
    let bytes = std::fs::read(dir.path("a"))?;
    let clock = a.clock().clone();
    assert!(a.merge_from(dir.path("b"), writer(1)).is_err());
    assert_eq!(a.links.number(id)?, 1);
    assert!(!a.links.is_published());
    assert_eq!(*a.clock(), clock);
    assert_eq!(std::fs::read(dir.path("a"))?, bytes);
    assert!(a.recovery().is_none());
    Ok(())
}

mod plain_policy {
    #[dbsync::entity]
    pub struct Item {
        value: u32,
    }
    #[dbsync::container]
    pub struct Database {
        items: Set<Item>,
    }
}

mod named_policy {
    #[dbsync::entity]
    #[merge(with = newest)]
    pub struct Item {
        value: u32,
    }

    fn newest(input: &dbsync::schema::MergeInput<'_, ItemValue>) -> ItemValue {
        ItemValue {
            value: input.candidates.iter().map(|row| row.value.value).sum(),
        }
    }

    #[dbsync::container]
    pub struct Database {
        items: Set<Item>,
    }
}

#[test]
fn a_custom_function_named_newest_is_not_the_builtin_policy() -> Result<()> {
    let dir = Directory::new();
    named_policy::Database::create(dir.path("custom"), writer(1))?;
    let err = plain_policy::Database::open(dir.path("custom"))
        .err()
        .unwrap();
    assert!(err.is_unsupported(), "{err}");
    Ok(())
}
