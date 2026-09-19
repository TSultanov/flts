use super::{Store, Table};
use crate::storage::column_file::{CommitFault, fail_commit_at, fail_commit_times};
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
fn transient_publication_failures_are_retried_without_caller_recovery() -> Result<()> {
    for fault in [
        CommitFault::Payload,
        CommitFault::Directory,
        CommitFault::BeforeSlot,
        CommitFault::PartialSlot,
        CommitFault::SlotSync,
        CommitFault::AppendMark,
    ] {
        let dir = TempDir::new("automatic_commit");
        let writer = WriterId::from_raw(1);
        let mut store = Database::create(dir.join("db"), writer)?;
        let id = store.items.push("pending")?;
        fail_commit_at(fault);
        store.commit(writer)?;
        assert!(store.recovery().is_none(), "{fault:?}");
        assert_eq!(store.items.rows(), 1);
        assert!(store.items.is_published());
        assert_eq!(Database::open(dir.join("db"))?.items.value(id)?, "pending");
        let dot = store.at(id)?.version()?.dot();
        let generation = store.generation();
        store.commit(writer)?;
        assert_eq!(store.generation(), generation);
        assert_eq!(store.at(id)?.version()?.dot(), dot);
    }
    Ok(())
}

#[test]
fn exhausting_retries_retains_edits_and_reuses_the_prepared_revision_on_resume() -> Result<()> {
    for fault in [
        CommitFault::Payload,
        CommitFault::Directory,
        CommitFault::BeforeSlot,
        CommitFault::PartialSlot,
        CommitFault::SlotSync,
    ] {
        let dir = TempDir::new("retry_exhausted");
        let writer = WriterId::from_raw(1);
        let mut store = Database::create(dir.join("db"), writer)?;
        let id = store.items.push("pending")?;
        fail_commit_times(fault, 3);
        let err = store.commit(writer).unwrap_err();
        assert!(
            matches!(err, Error::RetryExhausted { attempts: 3, .. }),
            "{err}"
        );
        assert_eq!(store.items.value(id)?, "pending");
        let dot = store.recovery().unwrap().prepared.at(id)?.version()?.dot();
        assert!(store.items.set_value(id, "blocked").is_err());
        let mut prepared = store.recovery().unwrap().prepared.clone();
        assert!(prepared.commit(writer).is_err());
        store.commit(writer)?;
        assert!(store.recovery().is_none());
        assert_eq!(store.at(id)?.version()?.dot(), dot);
        assert_eq!(store.history(id)?.len(), 1);
        assert_eq!(
            Database::open(dir.join("db"))?.at(id)?.version()?.dot(),
            dot
        );
    }
    Ok(())
}

#[test]
fn resuming_a_published_attempt_incorporates_a_later_generation() -> Result<()> {
    let dir = TempDir::new("later_generation");
    let writer = WriterId::from_raw(1);
    let mut store = Database::create(dir.join("db"), writer)?;
    let id = store.items.push("uncertain")?;
    fail_commit_times(CommitFault::SlotSync, 3);
    assert!(matches!(
        store.commit(writer),
        Err(Error::RetryExhausted { .. })
    ));
    let dot = store.recovery().unwrap().prepared.at(id)?.version()?.dot();
    let mut another = Database::open(dir.join("db"))?;
    let added = another.items.push("later")?;
    another.commit(WriterId::from_raw(2))?;
    store.commit(writer)?;
    assert_eq!(store.items.value(added)?, "later");
    assert_eq!(store.at(id)?.version()?.dot(), dot);
    assert_eq!(store.items.rows(), 2);
    Ok(())
}

#[test]
fn merge_from_resumes_a_failed_publication_and_reports_the_received_rows() -> Result<()> {
    let dir = TempDir::new("resume_merge");
    let writer = WriterId::from_raw(1);
    let mut store = Database::create(dir.join("db"), writer)?;
    std::fs::copy(dir.join("db"), dir.join("peer"))?;
    let mut peer = Database::open(dir.join("peer"))?;
    let incoming = peer.items.push("incoming")?;
    peer.commit(WriterId::from_raw(2))?;
    let local = store.items.push("local")?;
    fail_commit_times(CommitFault::PartialSlot, 3);
    assert!(matches!(
        store.merge_from(dir.join("peer"), writer),
        Err(Error::RetryExhausted { .. })
    ));
    let local_dot = store
        .recovery()
        .unwrap()
        .prepared
        .at(local)?
        .version()?
        .dot();
    let report = store.merge_from(dir.join("peer"), writer)?;
    assert_eq!(report.taken, 1);
    assert!(report.changed_anything());
    assert_eq!(store.items.value(incoming)?, "incoming");
    assert_eq!(store.at(local)?.version()?.dot(), local_dot);
    assert_eq!(Database::open(dir.join("db"))?.items.rows(), 2);
    Ok(())
}
