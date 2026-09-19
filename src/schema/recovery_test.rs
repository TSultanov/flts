use super::{Store, Table};
use crate::storage::column_file::{CommitFault, fail_commit_at};
use crate::test_utils::TempDir;
use crate::{Error, Result, WriterId};

#[crate::entity]
struct Item {
    value: String,
}

#[crate::container]
struct Database {
    items: Set<Item>,
}

#[test]
fn prepublication_failures_preserve_unpublished_edits_and_clock() -> Result<()> {
    for fault in [
        CommitFault::Payload,
        CommitFault::Directory,
        CommitFault::BeforeSlot,
    ] {
        let dir = TempDir::new("commit_fault");
        let writer = WriterId::from_raw(1);
        let mut store = Database::create(dir.join("db"), writer)?;
        let id = store.items.push("pending")?;
        let clock = store.clock().clone();
        let generation = store.generation();
        fail_commit_at(fault);
        assert!(
            matches!(store.commit(writer), Err(Error::Io(_))),
            "{fault:?}"
        );
        assert_eq!(store.items.value(id)?, "pending");
        assert_eq!(*store.clock(), clock);
        assert_eq!(store.generation(), generation);
        assert!(!store.items.is_published());
        assert!(store.recovery().is_none());
        store.commit(writer)?;
        assert_eq!(Database::open(dir.join("db"))?.items.value(id)?, "pending");
    }
    Ok(())
}

#[test]
fn a_partial_slot_recovers_pending_edits_without_reusing_prepared_dots() -> Result<()> {
    let dir = TempDir::new("partial_slot");
    let writer = WriterId::from_raw(1);
    let mut store = Database::create(dir.join("db"), writer)?;
    let id = store.items.push("pending")?;
    let clock = store.clock().clone();
    fail_commit_at(CommitFault::PartialSlot);
    assert!(matches!(
        store.commit(writer),
        Err(Error::CommitUncertain { .. })
    ));
    assert!(store.items.set_value(id, "blocked").is_err());
    assert!(store.items.push("blocked").is_err());
    assert!(store.commit(writer).is_err());
    let peer = Database::open(dir.join("db"))?;
    assert!(store.merge(&peer).is_err());
    assert_eq!(store.items.value(id)?, "pending");
    store.recover_commit()?;
    assert_eq!(*store.clock(), clock);
    assert!(!store.items.is_published());
    assert_eq!(store.items.value(id)?, "pending");
    store.commit(writer)?;
    assert_eq!(Database::open(dir.join("db"))?.items.value(id)?, "pending");
    Ok(())
}

#[test]
fn recovery_of_a_published_slot_keeps_the_attempted_revision_identity() -> Result<()> {
    let dir = TempDir::new("published_slot");
    let writer = WriterId::from_raw(1);
    let mut store = Database::create(dir.join("db"), writer)?;
    let id = store.items.push("published")?;
    fail_commit_at(CommitFault::SlotSync);
    assert!(matches!(
        store.commit(writer),
        Err(Error::CommitUncertain { .. })
    ));
    let prepared = &store.recovery().unwrap().prepared;
    let dot = prepared.at(id)?.version()?.dot();
    store.recover_commit()?;
    assert_eq!(store.at(id)?.version()?.dot(), dot);
    assert!(store.items.is_published());
    assert!(store.recovery().is_none());
    assert_eq!(
        Database::open(dir.join("db"))?.at(id)?.version()?.dot(),
        dot
    );
    store.items.set_value(id, "next")?;
    store.commit(writer)?;
    Ok(())
}

#[test]
fn append_mark_failure_does_not_report_a_durable_commit_as_failed() -> Result<()> {
    let dir = TempDir::new("mark_fault");
    let writer = WriterId::from_raw(1);
    let mut store = Database::create(dir.join("db"), writer)?;
    let id = store.items.push("published")?;
    fail_commit_at(CommitFault::AppendMark);
    store.commit(writer)?;
    assert!(store.recovery().is_none());
    assert_eq!(
        Database::open(dir.join("db"))?.items.value(id)?,
        "published"
    );
    store.items.set_value(id, "next")?;
    store.commit(writer)?;
    assert_eq!(Database::open(dir.join("db"))?.items.value(id)?, "next");
    Ok(())
}

#[test]
fn recovery_recognizes_the_attempted_revisions_in_a_later_generation() -> Result<()> {
    let dir = TempDir::new("later_generation");
    let writer = WriterId::from_raw(1);
    let mut store = Database::create(dir.join("db"), writer)?;
    let id = store.items.push("uncertain")?;
    fail_commit_at(CommitFault::SlotSync);
    assert!(matches!(
        store.commit(writer),
        Err(Error::CommitUncertain { .. })
    ));
    let mut prepared = store.recovery().unwrap().prepared.clone();
    assert!(
        prepared
            .items
            .set_value(id, "cannot escape recovery")
            .is_err()
    );
    assert!(prepared.commit(writer).is_err());
    let mut another = Database::open(dir.join("db"))?;
    let added = another.items.push("later")?;
    another.commit(WriterId::from_raw(2))?;
    store.recover_commit()?;
    assert!(store.recovery().is_none());
    assert_eq!(store.items.value(id)?, "uncertain");
    assert_eq!(store.items.value(added)?, "later");
    store.items.set_value(id, "next")?;
    store.commit(writer)?;
    Ok(())
}
