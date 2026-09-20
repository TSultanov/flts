#![cfg(feature = "macros")]
use dbsync::schema::{Store, Table};
use dbsync::{Result, WriterId};
use std::path::PathBuf;

#[dbsync::entity]
struct Doc {
    #[index]
    key: String,
    title: String,
    body: String,
}

#[dbsync::container(schema = "library", version = 1)]
struct Library {
    docs: Set<Doc>,
}

fn writer(n: u64) -> WriterId {
    WriterId::from_raw(n)
}

struct Directory(PathBuf);

impl std::ops::Deref for Directory {
    type Target = std::path::Path;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn directory(name: &str) -> Directory {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "dbsync-review-{name}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    Directory(dir)
}

fn commit(s: &mut impl Store, n: u64) -> Result<()> {
    std::thread::sleep(std::time::Duration::from_millis(2));
    s.commit(writer(n))?;
    Ok(())
}

fn fork(dir: &std::path::Path, from: &str, to: &str) -> Result<Library> {
    std::fs::copy(dir.join(from), dir.join(to))?;
    Library::open(dir.join(to))
}

#[test]
fn different_ids_stay_independent_when_indexed_values_change() -> Result<()> {
    let dir = directory("collision");
    let mut base = Library::create(dir.join("base"), writer(1))?;
    commit(&mut base, 1)?;
    let mut a = fork(&dir, "base", "a")?;
    let mut b = fork(&dir, "base", "b")?;
    let x = a.docs.push("same", "A", "")?;
    commit(&mut a, 2)?;
    let y = b.docs.push("same", "B", "")?;
    commit(&mut b, 3)?;
    let mut c = fork(&dir, "b", "c")?;
    c.docs.set_key(y, "moved")?;
    commit(&mut c, 4)?;

    let mut left = a.clone();
    left.merge_snapshot(&b)?;
    left.merge_snapshot(&c)?;
    let mut gathered = b.clone();
    gathered.merge_snapshot(&c)?;
    let mut right = a.clone();
    right.merge_snapshot(&gathered)?;
    left.audit()?;
    right.audit()?;
    left.merge_snapshot(&right)?;
    right.merge_snapshot(&left)?;
    assert_eq!(
        left.docs.live_row_of(x)?.is_some(),
        right.docs.live_row_of(x)?.is_some()
    );
    Ok(())
}

#[test]
fn concurrent_field_edits_choose_one_complete_row() -> Result<()> {
    let dir = directory("fields");
    let mut a = Library::create(dir.join("a"), writer(1))?;
    let id = a.docs.push("key", "original title", "original body")?;
    commit(&mut a, 1)?;
    let mut b = fork(&dir, "a", "b")?;
    a.docs.set_title(id, "new title")?;
    commit(&mut a, 1)?;
    b.docs.set_body(id, "new body")?;
    commit(&mut b, 2)?;
    a.merge_snapshot(&b)?;
    assert_eq!(a.docs.title(id)?, "original title");
    assert_eq!(a.docs.body(id)?, "new body");
    Ok(())
}

#[test]
fn report_must_identify_the_actual_conflict_winner() -> Result<()> {
    let dir = directory("report");
    let mut a = Library::create(dir.join("a"), writer(1))?;
    let id = a.docs.push("key", "base", "")?;
    commit(&mut a, 1)?;
    let mut b = fork(&dir, "a", "b")?;
    a.docs.set_title(id, "local")?;
    commit(&mut a, 1)?;
    b.docs.set_title(id, "remote intermediate")?;
    commit(&mut b, 2)?;
    b.docs.set_title(id, "remote final")?;
    commit(&mut b, 2)?;
    let report = a.merge_snapshot(&b)?;
    let winner = a.docs.versions().at(a.docs.row_of(id)?)?.dot();
    assert!(
        report
            .concurrent
            .iter()
            .any(|conflict| conflict.kept == winner),
        "{report:?}; actual winner {winner:?}"
    );
    Ok(())
}

mod newer {
    #[dbsync::entity]
    pub struct Doc {
        #[index]
        key: String,
        title: String,
        body: String,
    }
    #[dbsync::entity]
    pub struct Note {
        text: String,
    }
    #[dbsync::container(schema = "library", version = 1)]
    pub struct Library {
        docs: Set<Doc>,
        notes: Set<Note>,
    }
}

#[test]
fn old_schema_must_not_acknowledge_unmerged_entities() -> Result<()> {
    let dir = directory("unknown-entity");
    let mut a = newer::Library::create(dir.join("a"), writer(1))?;
    commit(&mut a, 1)?;
    std::fs::copy(dir.join("a"), dir.join("b"))?;
    let mut b = newer::Library::open(dir.join("b"))?;
    b.notes.push("must survive")?;
    commit(&mut b, 2)?;

    assert!(Library::open(dir.join("a")).is_err());
    Ok(())
}

mod added_field {
    #[dbsync::entity]
    pub struct Doc {
        #[index]
        key: String,
        title: String,
        body: String,
        extra: u32,
    }
    #[dbsync::container(schema = "library", version = 1)]
    pub struct Library {
        docs: Set<Doc>,
    }
}

#[test]
fn old_schema_merge_must_not_break_new_schema_row_alignment() -> Result<()> {
    let dir = directory("unknown-field");
    let mut a = added_field::Library::create(dir.join("a"), writer(1))?;
    a.docs.push("first", "", "", 42)?;
    commit(&mut a, 1)?;
    std::fs::copy(dir.join("a"), dir.join("b"))?;
    let mut b = added_field::Library::open(dir.join("b"))?;
    b.docs.push("second", "", "", 99)?;
    commit(&mut b, 2)?;
    assert!(Library::open(dir.join("a")).is_err());
    Ok(())
}

#[test]
fn a_same_device_stale_commit_preserves_both_sets_of_edits() -> Result<()> {
    let dir = directory("stale_commit");
    let mut a = Library::create(dir.join("db"), writer(1))?;
    let first = a.docs.push("first", "old first", "")?;
    let second = a.docs.push("second", "old second", "")?;
    commit(&mut a, 1)?;
    let mut b = Library::open(dir.join("db"))?;
    a.docs.set_title(first, "new first")?;
    commit(&mut a, 1)?;
    b.docs.set_title(second, "new second")?;
    commit(&mut b, 1)?;
    assert_eq!(b.docs.title(first)?, "new first");
    assert_eq!(b.docs.title(second)?, "new second");
    let reloaded = Library::open(dir.join("db"))?;
    assert_eq!(reloaded.docs.title(first)?, "new first");
    assert_eq!(reloaded.docs.title(second)?, "new second");
    Ok(())
}

#[test]
fn control_plain_row_updates_converge_for_every_merge_order() -> Result<()> {
    let dir = directory("plain-control");
    let mut base = Library::create(dir.join("base"), writer(1))?;
    let id = base.docs.push("key", "base", "")?;
    commit(&mut base, 1)?;
    let mut replicas = Vec::new();
    for n in 0..3 {
        let mut replica = fork(&dir, "base", &format!("replica-{n}"))?;
        replica.docs.set_title(id, &format!("edit-{n}"))?;
        commit(&mut replica, n + 2)?;
        replicas.push(replica);
    }
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let mut merged = replicas[order[0]].clone();
        merged.merge_snapshot(&replicas[order[1]])?;
        merged.merge_snapshot(&replicas[order[2]])?;
        assert_eq!(merged.docs.title(id)?, "edit-2");
        merged.audit()?;
        let count = merged.docs.rows();
        for replica in &replicas {
            assert!(!merged.merge_snapshot(replica)?.changed_anything());
        }
        assert_eq!(merged.docs.rows(), count);
    }
    Ok(())
}

#[test]
fn control_explicit_deletion_propagates_and_survives_stale_sync() -> Result<()> {
    let dir = directory("delete-control");
    let mut a = Library::create(dir.join("a"), writer(1))?;
    let id = a.docs.push("key", "base", "")?;
    commit(&mut a, 1)?;
    let stale = a.clone();
    let mut b = fork(&dir, "a", "b")?;
    b.remove(id)?;
    commit(&mut b, 2)?;
    a.merge_snapshot(&b)?;
    a.merge_snapshot(&stale)?;
    commit(&mut a, 1)?;
    let reopened = Library::open(dir.join("a"))?;
    assert!(reopened.docs.live_row_of(id)?.is_none());
    reopened.audit()?;
    Ok(())
}

mod relationships {
    #[dbsync::entity]
    pub struct Parent {
        #[index]
        name: String,
    }
    #[dbsync::entity]
    pub struct Child {
        parent: One<Parent>,
        text: String,
    }
    #[dbsync::entity]
    pub struct Grandchild {
        child: One<Child>,
        text: String,
    }
    #[dbsync::container(schema = "library", version = 1)]
    pub struct Library {
        parents: Set<Parent>,
        children: Set<Child>,
        grandchildren: Set<Grandchild>,
    }
}

#[test]
fn repeated_parent_names_do_not_hide_parents_or_children() -> Result<()> {
    let dir = directory("parent-name");
    let mut a = relationships::Library::create(dir.join("a"), writer(1))?;
    std::fs::copy(dir.join("a"), dir.join("b"))?;
    let mut b = relationships::Library::open(dir.join("b"))?;
    let first = a.parents.push("same")?;
    let child = a.children.push(first, "child")?;
    let second = b.parents.push("same")?;
    commit(&mut b, 2)?;
    a.merge_from(dir.join("b"), writer(1))?;
    assert!(a.at(first).is_ok());
    assert!(a.at(second).is_ok());
    assert!(a.at(child).is_ok());
    assert_eq!(a.parents.find_by_name("same")?.len(), 2);
    a.remove(second)?;
    assert_eq!(a.parents.find_by_name("same")?, [first]);
    assert_eq!(a.at(child)?.text()?, "child");
    Ok(())
}

#[test]
fn deletion_wins_concurrent_edits_and_restore_excludes_delayed_old_edits() -> Result<()> {
    let dir = directory("restore");
    let mut a = Library::create(dir.join("a"), writer(1))?;
    let id = a.docs.push("key", "base", "body")?;
    commit(&mut a, 1)?;
    let base = a.at(id)?.version()?.dot();
    let mut b = fork(&dir, "a", "b")?;
    let mut delayed = fork(&dir, "a", "delayed")?;
    a.remove(id)?;
    commit(&mut a, 1)?;
    b.docs.set_title(id, "concurrent edit")?;
    commit(&mut b, 2)?;
    a.merge_snapshot(&b)?;
    assert_eq!(a.status(id)?, dbsync::schema::RowStatus::Deleted);
    assert!(a.docs.set_title(id, "implicit resurrection").is_err());
    a.restore(id, base)?;
    commit(&mut a, 1)?;
    delayed.docs.set_title(id, "late old edit")?;
    commit(&mut delayed, 3)?;
    a.merge_snapshot(&delayed)?;
    assert_eq!(a.docs.title(id)?, "base");
    assert_eq!(a.history(id)?.len(), 5);
    assert_eq!(a.at_version(id, base)?.title()?, "base");
    commit(&mut a, 1)?;
    let reopened = Library::open(dir.join("a"))?;
    assert_eq!(reopened.docs.title(id)?, "base");
    assert_eq!(reopened.history(id)?.len(), 5);
    Ok(())
}

#[test]
fn a_restore_cannot_undo_an_unseen_concurrent_deletion() -> Result<()> {
    let dir = directory("restore-delete");
    let mut a = Library::create(dir.join("a"), writer(1))?;
    let id = a.docs.push("key", "base", "")?;
    commit(&mut a, 1)?;
    let dot = a.at(id)?.version()?.dot();
    let mut b = fork(&dir, "a", "b")?;
    a.remove(id)?;
    commit(&mut a, 1)?;
    a.restore(id, dot)?;
    commit(&mut a, 1)?;
    b.remove(id)?;
    commit(&mut b, 2)?;
    a.merge_snapshot(&b)?;
    assert_eq!(a.status(id)?, dbsync::schema::RowStatus::Deleted);
    a.restore(id, dot)?;
    commit(&mut a, 1)?;
    b.merge_snapshot(&a)?;
    assert_eq!(b.docs.title(id)?, "base");
    Ok(())
}

#[test]
fn parent_restoration_reveals_a_transitively_hidden_subtree() -> Result<()> {
    let dir = directory("subtree");
    let mut store = relationships::Library::create(dir.join("db"), writer(1))?;
    let parent = store.parents.push("parent")?;
    let child = store.children.push(parent, "child")?;
    let grandchild = store.grandchildren.push(child, "grandchild")?;
    commit(&mut store, 1)?;
    let parent_dot = store.at(parent)?.version()?.dot();
    store.remove(parent)?;
    assert!(store.at(child).is_err());
    assert!(store.children.text(child).is_err());
    assert_eq!(store.all::<relationships::Grandchild>().count(), 0);
    assert_eq!(store.at_hidden(grandchild)?.text()?, "grandchild");
    assert_eq!(store.conflicts()?.len(), 2);
    commit(&mut store, 1)?;
    let mut store = relationships::Library::open(dir.join("db"))?;
    store.restore(parent, parent_dot)?;
    assert_eq!(store.at(child)?.text()?, "child");
    assert_eq!(store.at(grandchild)?.text()?, "grandchild");
    assert!(store.conflicts()?.is_empty());
    store.audit()?;
    Ok(())
}

#[test]
fn a_historical_row_can_be_selected_as_an_explicit_new_edit() -> Result<()> {
    let dir = directory("history_edit");
    let mut db = Library::create(dir.join("db"), writer(1))?;
    let id = db.docs.push("same", "A", "")?;
    db.docs.push("same", "B", "")?;
    commit(&mut db, 1)?;
    let chosen = db.at(id)?.version()?.dot();
    db.docs.set_title(id, "changed")?;
    commit(&mut db, 1)?;
    db.resolve(id, chosen)?;
    commit(&mut db, 1)?;
    assert_eq!(db.docs.title(id)?, "A");
    assert_ne!(db.at(id)?.version()?.dot(), chosen);
    assert_eq!(db.at_version(id, chosen)?.title()?, "A");
    assert_eq!(db.all::<Doc>().count(), 2);
    Ok(())
}

mod multiple_keys {
    #[dbsync::entity]
    pub struct Item {
        #[index]
        first: String,
        #[index]
        second: Option<String>,
    }
    #[dbsync::container(schema = "database", version = 1)]
    pub struct Database {
        items: Set<Item>,
    }
}

#[test]
fn repeated_values_across_multiple_indexes_remain_independent() -> Result<()> {
    let mut db = multiple_keys::Database::default();
    let first = db.items.push("same", Some("same"))?;
    let second = db.items.push("same", Some("same"))?;
    assert_eq!(db.items.find_by_first("same")?, [first, second]);
    assert_eq!(db.items.find_by_second(Some("same"))?, [first, second]);
    db.audit()?;
    Ok(())
}

mod cycle {
    #[dbsync::entity]
    pub struct Link {
        target: One<Link>,
        name: String,
    }
    #[dbsync::container(schema = "database", version = 1)]
    pub struct Database {
        links: Set<Link>,
    }
}

#[test]
fn reference_cycles_are_visible_until_an_external_deletion_blocks_them() -> Result<()> {
    let dir = directory("cycle");
    let mut db = cycle::Database::create(dir.join("db"), writer(1))?;
    let a = db.links.push(dbsync::Id::NIL, "a")?;
    let b = db.links.push(a, "b")?;
    db.links.set_target(a, b)?;
    commit(&mut db, 1)?;
    assert_eq!(db.all::<cycle::Link>().count(), 2);
    db.remove(a)?;
    assert!(db.at(b).is_err());
    commit(&mut db, 1)?;
    assert_eq!(
        cycle::Database::open(dir.join("db"))?
            .all::<cycle::Link>()
            .count(),
        0
    );
    Ok(())
}

#[test]
fn parent_deletion_hides_concurrent_children() -> Result<()> {
    let dir = directory("rollback-control");
    let mut a = relationships::Library::create(dir.join("a"), writer(1))?;
    let id = a.parents.push("parent")?;
    commit(&mut a, 1)?;
    std::fs::copy(dir.join("a"), dir.join("b"))?;
    let mut b = relationships::Library::open(dir.join("b"))?;
    b.children.push(id, "child")?;
    commit(&mut b, 2)?;
    a.remove(id)?;
    commit(&mut a, 1)?;
    let old_clock = a.clock().clone();
    let old_parents = a.parents.rows();
    a.merge_snapshot(&b)?;
    assert_ne!(*a.clock(), old_clock);
    assert_eq!(a.parents.rows(), old_parents);
    assert_eq!(a.children.rows(), 1);
    assert_eq!(a.all::<relationships::Child>().count(), 0);
    a.audit()?;
    Ok(())
}
