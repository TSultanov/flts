#![cfg(feature = "macros")]

use dbsync::schema::{Column, Dot, Store, TextColumn};
use dbsync::{Arena, ColumnFile, Record, Result, Uuid, WriterId};
use std::path::{Path, PathBuf};

#[dbsync::entity]
struct Item {
    text: String,
}

#[dbsync::container]
struct Database {
    items: Set<Item>,
}

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("dbsync-integrity-{}", Uuid::now_v7()));
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

fn replace<T: Record>(path: &Path, name: &str, values: &[T]) -> Result<()> {
    let file = ColumnFile::open(path)?;
    let mut update = file.update(WriterId::from_raw(99))?;
    for column in file.columns() {
        if column.name != name {
            update.keep_column(&column.name)?;
        }
    }
    let mut arena = Arena::new();
    arena.extend_from_slice(values)?;
    update.column(name, &arena)?;
    update.commit()?;
    Ok(())
}

#[test]
fn an_existing_dot_with_a_different_payload_is_rejected_without_mutation() -> Result<()> {
    let dir = Directory::new();
    let mut db = Database::create(dir.path("a"), WriterId::from_raw(1))?;
    let id = db.items.push("original")?;
    db.commit(WriterId::from_raw(1))?;
    std::fs::copy(dir.path("a"), dir.path("b"))?;
    let file = ColumnFile::open(dir.path("b"))?;
    let mut update = file.update(WriterId::from_raw(99))?;
    for column in file.columns() {
        if column.name != "item.text" && column.name != "item.text.bytes" {
            update.keep_column(&column.name)?;
        }
    }
    let mut text = TextColumn::default();
    text.push("different payload")?;
    text.offer_to("item", "text", &mut update)?;
    update.commit()?;
    let other = Database::open(dir.path("b"))?;
    let clock = db.clock().clone();
    let err = db.merge_snapshot(&other).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert_eq!(db.items.text(id)?, "original");
    assert_eq!(*db.clock(), clock);
    assert_eq!(db.history(id)?.len(), 1);
    Ok(())
}

#[test]
fn an_existing_dot_with_different_valid_ancestry_is_rejected() -> Result<()> {
    let dir = Directory::new();
    let mut db = Database::create(dir.path("a"), WriterId::from_raw(1))?;
    let id = db.items.push("one")?;
    db.commit(WriterId::from_raw(1))?;
    let first = db.at(id)?.version()?.dot();
    db.items.set_text(id, "two")?;
    db.commit(WriterId::from_raw(1))?;
    db.items.set_text(id, "three")?;
    db.commit(WriterId::from_raw(1))?;
    std::fs::copy(dir.path("a"), dir.path("b"))?;
    replace(&dir.path("b"), "item.version.parents", &[first, first])?;
    let other = Database::open(dir.path("b"))?;
    assert!(db.merge_snapshot(&other).unwrap_err().is_corrupt());
    assert_eq!(db.items.text(id)?, "three");
    assert_eq!(db.history(id)?.len(), 3);
    Ok(())
}

#[test]
fn missing_revision_parents_and_ancestry_cycles_are_corruption() -> Result<()> {
    let dir = Directory::new();
    let mut db = Database::create(dir.path("a"), WriterId::from_raw(1))?;
    let id = db.items.push("one")?;
    db.commit(WriterId::from_raw(1))?;
    let first = db.at(id)?.version()?.dot();
    db.items.set_text(id, "two")?;
    db.commit(WriterId::from_raw(1))?;
    let second = db.at(id)?.version()?.dot();
    std::fs::copy(dir.path("a"), dir.path("missing"))?;
    replace(
        &dir.path("missing"),
        "item.version.parents",
        &[Dot::new(WriterId::from_raw(99), 1)],
    )?;
    let err = Database::open(dir.path("missing")).err().unwrap();
    assert!(err.is_corrupt(), "{err}");
    std::fs::copy(dir.path("a"), dir.path("cycle"))?;
    replace(&dir.path("cycle"), "item.version.parent_ends", &[1u64, 2])?;
    replace(&dir.path("cycle"), "item.version.parents", &[second, first])?;
    let err = Database::open(dir.path("cycle")).err().unwrap();
    assert!(err.is_corrupt(), "{err}");
    Ok(())
}

mod integer {
    #[dbsync::entity]
    pub struct Item {
        number: u32,
    }
    #[dbsync::container]
    pub struct Database {
        items: Set<Item>,
    }
}

mod float {
    #[dbsync::entity]
    pub struct Item {
        number: f32,
    }
    #[dbsync::container]
    pub struct Database {
        items: Set<Item>,
    }
}

mod enum_a {
    #[dbsync::field_enum]
    pub enum State {
        One = 0,
        Two = 1,
    }
    #[dbsync::entity]
    pub struct Item {
        state: State,
    }
    #[dbsync::container]
    pub struct Database {
        items: Set<Item>,
    }
}

mod enum_b {
    #[dbsync::field_enum]
    pub enum State {
        One = 1,
        Two = 0,
    }
    #[dbsync::entity]
    pub struct Item {
        state: State,
    }
    #[dbsync::container]
    pub struct Database {
        items: Set<Item>,
    }
}

#[test]
fn equal_width_type_changes_and_enum_code_changes_are_not_compatible() -> Result<()> {
    let dir = Directory::new();
    integer::Database::create(dir.path("number"), WriterId::from_raw(1))?;
    assert!(
        float::Database::open(dir.path("number"))
            .err()
            .unwrap()
            .is_unsupported()
    );
    enum_a::Database::create(dir.path("enum"), WriterId::from_raw(1))?;
    assert!(
        enum_b::Database::open(dir.path("enum"))
            .err()
            .unwrap()
            .is_unsupported()
    );
    Ok(())
}

#[test]
fn snapshot_mutations_do_not_change_the_original_visibility_or_history() -> Result<()> {
    let dir = Directory::new();
    let mut db = Database::create(dir.path("db"), WriterId::from_raw(1))?;
    let id = db.items.push("original")?;
    db.commit(WriterId::from_raw(1))?;
    let mut snapshot = db.snapshot();
    snapshot.remove(id)?;
    assert_eq!(db.items.text(id)?, "original");
    assert!(snapshot.items.text(id).is_err());
    assert_eq!(db.history(id)?.len(), 1);
    assert_eq!(snapshot.history(id)?.len(), 2);
    Ok(())
}

#[test]
fn identical_concurrent_payloads_are_retained_without_a_value_conflict() -> Result<()> {
    let dir = Directory::new();
    let mut a = Database::create(dir.path("a"), WriterId::from_raw(1))?;
    let id = a.items.push("base")?;
    a.commit(WriterId::from_raw(1))?;
    std::fs::copy(dir.path("a"), dir.path("b"))?;
    let mut b = Database::open(dir.path("b"))?;
    a.items.set_text(id, "same")?;
    b.items.set_text(id, "same")?;
    a.commit(WriterId::from_raw(1))?;
    b.commit(WriterId::from_raw(2))?;
    let report = a.merge_snapshot(&b)?;
    assert!(report.concurrent.is_empty());
    assert_eq!(a.history(id)?.len(), 3);
    assert_eq!(a.items.text(id)?, "same");
    assert!(a.conflicts()?.is_empty());
    Ok(())
}

#[test]
fn recording_ancestry_does_not_increment_the_unpublished_format_version() -> Result<()> {
    let dir = Directory::new();
    Database::create(dir.path("db"), WriterId::from_raw(1))?;
    let bytes = std::fs::read(dir.path("db"))?;
    assert_eq!(u16::from_le_bytes(bytes[4..6].try_into().unwrap()), 1);
    Ok(())
}

mod names {
    #[dbsync::entity]
    pub struct Item {
        row: u32,
        of: u32,
        restore: bool,
        dot: u32,
        value: u32,
    }
    #[dbsync::container]
    pub struct Database {
        items: Set<Item>,
    }
}

#[test]
fn field_names_do_not_shadow_revision_and_recovery_arguments() -> Result<()> {
    let dir = Directory::new();
    let mut db = names::Database::create(dir.path("db"), WriterId::from_raw(1))?;
    let id = db.items.push(1, 2, false, 4, 5)?;
    db.commit(WriterId::from_raw(1))?;
    let dot = db.at(id)?.version()?.dot();
    db.items.set_row(id, 10)?;
    db.items.set_of(id, 20)?;
    db.items.set_dot(id, 40)?;
    db.items.set_value(id, 50)?;
    assert_eq!(db.items.row(id)?, 10);
    assert_eq!(db.items.of(id)?, 20);
    assert_eq!(db.items.dot(id)?, 40);
    assert_eq!(db.items.value(id)?, 50);
    db.remove(id)?;
    db.restore(id, dot)?;
    assert_eq!(db.items.row(id)?, 1);
    assert_eq!(db.items.value(id)?, 5);
    assert!(!db.items.restore(id)?);
    db.commit(WriterId::from_raw(1))?;
    Ok(())
}

mod references {
    #[dbsync::entity]
    pub struct Item {
        parent: One<Item>,
    }
    #[dbsync::container]
    pub struct Database {
        items: Set<Item>,
    }
}

#[test]
fn references_to_ids_that_never_existed_are_corruption() -> Result<()> {
    let dir = Directory::new();
    let mut db = references::Database::create(dir.path("db"), WriterId::from_raw(1))?;
    db.items.push(dbsync::Id::NIL)?;
    db.commit(WriterId::from_raw(1))?;
    replace(
        &dir.path("db"),
        "item.parent",
        &[dbsync::Id::<references::Item>::mint()],
    )?;
    let err = references::Database::open(dir.path("db")).err().unwrap();
    assert!(err.is_corrupt(), "{err}");
    Ok(())
}

#[test]
fn duplicate_dots_for_different_entities_are_rejected() -> Result<()> {
    let dir = Directory::new();
    let mut db = Database::create(dir.path("db"), WriterId::from_raw(1))?;
    let first = db.items.push("first")?;
    db.items.push("second")?;
    db.commit(WriterId::from_raw(1))?;
    let version = db.at(first)?.version()?;
    replace(&dir.path("db"), "item.version", &[version, version])?;
    assert!(Database::open(dir.path("db")).err().unwrap().is_corrupt());
    Ok(())
}
