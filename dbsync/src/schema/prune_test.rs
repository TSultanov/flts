use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::id::Id;
use crate::schema::{RowStatus, Store, Table};
use crate::storage::WriterId;
use crate::test_utils::TempDir;

#[crate::entity]
struct Page {
    #[index]
    title: String,
    position: u32,
}

#[crate::entity]
#[merge(history)]
struct Entry {
    text: String,
}

#[crate::container(schema = "journal", version = 1)]
struct Journal {
    pages: Set<Page>,
    entries: Set<Entry>,
}

mod older {
    #[crate::entity]
    pub struct Page {
        #[index]
        title: String,
    }

    #[crate::container(schema = "journal", version = 1)]
    pub struct Journal {
        pub pages: Set<Page>,
    }
}

mod newer {
    #[crate::entity]
    pub struct Page {
        #[index]
        title: String,
        mark: Option<u32>,
    }

    #[crate::container(schema = "journal", version = 1)]
    pub struct Journal {
        pub pages: Set<Page>,
    }
}

const PHONE: WriterId = WriterId::from_raw(0x51);
const DESK: WriterId = WriterId::from_raw(0x52);
const TABLET: WriterId = WriterId::from_raw(0x53);

fn carry(from: &Path, to: &Path, writer: WriterId) {
    let shard = format!("{:016x}.v1.dbs", writer.raw());
    let said = format!("{:016x}.dbs", writer.raw());
    for (folder, name) in [("page", &shard), ("entry", &shard), ("dbsync.meta", &said)] {
        let source = from.join(folder).join(name);
        if source.exists() {
            std::fs::create_dir_all(to.join(folder)).unwrap();
            let arriving = to.join(folder).join(format!(".{name}.tmp"));
            std::fs::copy(source, &arriving).unwrap();
            std::fs::rename(arriving, to.join(folder).join(name)).unwrap();
        }
    }
}

fn exchange(a: (&Path, WriterId), b: (&Path, WriterId)) {
    carry(a.0, b.0, a.1);
    carry(b.0, a.0, b.1);
}

fn bytes_of(path: PathBuf) -> u64 {
    std::fs::metadata(path).unwrap().len()
}

struct Pair {
    phone: Journal,
    phone_dir: PathBuf,
    desk: Journal,
    desk_dir: PathBuf,
}

impl Pair {
    fn new(dir: &TempDir) -> Result<Self> {
        let (phone_dir, desk_dir) = (dir.join("phone"), dir.join("desk"));
        Ok(Self {
            phone: Journal::create(&phone_dir, PHONE)?,
            desk: Journal::create(&desk_dir, DESK)?,
            phone_dir,
            desk_dir,
        })
    }

    fn settle(&mut self) -> Result<()> {
        for _ in 0..2 {
            exchange((&self.phone_dir, PHONE), (&self.desk_dir, DESK));
            self.phone.refresh_peers()?;
            self.desk.refresh_peers()?;
            self.phone.acknowledge(PHONE)?;
            self.desk.acknowledge(DESK)?;
        }
        exchange((&self.phone_dir, PHONE), (&self.desk_dir, DESK));
        Ok(())
    }
}

fn turned(journal: &mut Journal, writer: WriterId, page: Id<Page>, times: u32) -> Result<()> {
    for position in 1..=times {
        journal.pages.set_position(page, position)?;
        journal.commit(writer)?;
    }
    Ok(())
}

#[test]
fn revisions_everyone_has_seen_past_are_dropped_from_the_shard_of_whoever_wrote_them() -> Result<()>
{
    let dir = TempDir::new("prune_settled");
    let mut pair = Pair::new(&dir)?;
    let page = pair.phone.pages.push("reading position", 0)?;
    pair.phone.commit(PHONE)?;
    turned(&mut pair.phone, PHONE, page, 40)?;
    let shard = pair.phone.layout().unwrap().shard("page", PHONE, 1);
    let before = bytes_of(shard.clone());
    pair.settle()?;

    let report = pair.phone.prune(PHONE, &[DESK])?;

    assert_eq!(report.dropped, 40);
    assert!(report.waiting_for.is_empty());
    assert!(bytes_of(shard) < before);
    assert_eq!(pair.phone.pages.position(page)?, 40);
    assert_eq!(Table::versions_of(&pair.phone.pages, page)?.len(), 1);
    assert!(pair.phone.conflicts()?.is_empty());
    pair.phone.verify()?;
    assert_eq!(pair.phone.prune(PHONE, &[DESK])?.dropped, 0);

    pair.phone.pages.set_position(page, 41)?;
    pair.phone.commit(PHONE)?;
    pair.settle()?;
    assert_eq!(pair.desk.pages.position(page)?, 41);
    assert!(pair.desk.conflicts()?.is_empty());

    std::fs::remove_dir_all(pair.desk_dir.join("local"))?;
    let fresh = Journal::open(&pair.desk_dir)?;
    assert_eq!(fresh.pages.position(page)?, 41);
    assert_eq!(Table::versions_of(&fresh.pages, page)?.len(), 2);
    assert!(fresh.conflicts()?.is_empty());
    fresh.verify()?;
    Ok(())
}

#[test]
fn a_writer_checking_its_shards_after_a_prune_brings_nothing_back() -> Result<()> {
    let dir = TempDir::new("prune_own");
    let mut pair = Pair::new(&dir)?;
    let page = pair.phone.pages.push("reading position", 0)?;
    let other = pair.phone.pages.push("bookmark", 3)?;
    pair.phone.entries.push("kept whole")?;
    pair.phone.commit(PHONE)?;
    turned(&mut pair.phone, PHONE, page, 5)?;
    pair.settle()?;
    assert_eq!(pair.phone.prune(PHONE, &[DESK])?.dropped, 5);
    let shard = pair.phone.layout().unwrap().shard("page", PHONE, 1);
    let pruned = std::fs::read(&shard)?;

    let found = pair.phone.check_own_shards(PHONE)?;

    assert_eq!(found, crate::schema::OwnShards::default());
    assert_eq!(std::fs::read(&shard)?, pruned);
    drop(pair.phone);

    // From the cache, and from the shards alone.
    for cold in [false, true] {
        if cold {
            std::fs::remove_dir_all(pair.phone_dir.join("local"))?;
        }
        let mut phone = Journal::open(&pair.phone_dir)?;
        assert_eq!(
            phone.check_own_shards(PHONE)?,
            crate::schema::OwnShards::default()
        );
        assert_eq!(std::fs::read(&shard)?, pruned);
        assert_eq!(Table::versions_of(&phone.pages, page)?.len(), 1);
        assert_eq!(phone.pages.position(page)?, 5);
        assert_eq!(phone.pages.position(other)?, 3);
        assert_eq!(phone.pages.find_by_title("bookmark")?, [other]);
        phone.verify()?;
    }
    Ok(())
}

#[test]
fn a_reader_given_a_pruned_shard_forgets_what_its_writer_dropped() -> Result<()> {
    let dir = TempDir::new("prune_reader");
    let mut pair = Pair::new(&dir)?;
    let early = pair.phone.pages.push("bookmark", 0)?;
    let page = pair.phone.pages.push("reading position", 0)?;
    pair.phone.commit(PHONE)?;
    turned(&mut pair.phone, PHONE, page, 5)?;
    pair.settle()?;
    assert_eq!(Table::versions_of(&pair.desk.pages, page)?.len(), 6);
    assert_eq!(pair.phone.prune(PHONE, &[DESK])?.dropped, 5);
    let floor = |journal: &Journal| journal.shard_marks().iter().map(|mark| mark.floor).max();

    pair.settle()?;

    assert_eq!(Table::versions_of(&pair.desk.pages, page)?.len(), 1);
    assert_eq!(pair.desk.pages.position(page)?, 5);
    assert_eq!(floor(&pair.desk), floor(&pair.phone));
    pair.desk.verify()?;

    // A drop under the floor that is already there.
    turned(&mut pair.phone, PHONE, early, 1)?;
    pair.settle()?;
    let reached = floor(&pair.phone);
    assert_eq!(pair.phone.prune(PHONE, &[DESK])?.dropped, 1);
    assert_eq!(floor(&pair.phone), reached);
    pair.settle()?;
    assert_eq!(Table::versions_of(&pair.desk.pages, early)?.len(), 1);
    assert_eq!(pair.desk.pages.position(early)?, 1);
    assert_eq!(pair.desk.pages.find_by_title("bookmark")?, [early]);
    assert!(pair.desk.conflicts()?.is_empty());
    pair.desk.verify()?;
    drop(pair.desk);

    let reopened = Journal::open(&pair.desk_dir)?;
    assert_eq!(Table::rows(&reopened.pages), 2);
    assert_eq!(reopened.pages.position(page)?, 5);
    reopened.verify()?;
    Ok(())
}

#[test]
fn nothing_is_dropped_while_a_writer_has_not_said_it_saw_the_newest_revision() -> Result<()> {
    let dir = TempDir::new("prune_unsettled");
    let mut pair = Pair::new(&dir)?;
    let page = pair.phone.pages.push("reading position", 0)?;
    pair.phone.commit(PHONE)?;
    pair.desk.entries.push("the desk exists")?;
    pair.desk.commit(DESK)?;
    pair.settle()?;
    turned(&mut pair.phone, PHONE, page, 5)?;

    let report = pair.phone.prune(PHONE, &[DESK, TABLET])?;
    assert_eq!(report.dropped, 0);
    assert_eq!(
        report.waiting_for,
        [TABLET],
        "nobody has heard from the tablet"
    );
    let report = pair.phone.prune(PHONE, &[DESK])?;
    assert_eq!(report.dropped, 0, "the desk has not seen the last five");
    assert!(report.waiting_for.is_empty());
    assert_eq!(Table::versions_of(&pair.phone.pages, page)?.len(), 6);

    std::fs::remove_file(pair.phone_dir.join("dbsync.meta/0000000000000052.dbs"))?;
    let report = pair.phone.prune(PHONE, &[DESK])?;
    assert_eq!((report.dropped, report.waiting_for), (0, vec![DESK]));

    pair.settle()?;
    assert_eq!(pair.phone.prune(PHONE, &[DESK])?.dropped, 5);
    Ok(())
}

#[test]
fn a_writer_left_out_of_the_roster_holds_nothing_back_and_is_still_read_if_it_returns() -> Result<()>
{
    let dir = TempDir::new("prune_retired");
    let mut pair = Pair::new(&dir)?;
    let page = pair.phone.pages.push("reading position", 0)?;
    pair.phone.commit(PHONE)?;
    pair.settle()?;
    turned(&mut pair.phone, PHONE, page, 3)?;
    assert_eq!(pair.phone.prune(PHONE, &[DESK])?.dropped, 0);

    assert_eq!(pair.phone.prune(PHONE, &[])?.dropped, 3);

    pair.desk
        .pages
        .set_title(page, "written by the one that was away")?;
    pair.desk.commit(DESK)?;
    carry(&pair.desk_dir, &pair.phone_dir, DESK);
    let told = pair.phone.refresh_peers()?;

    assert_eq!((told.taken, told.waiting), (1, 0));
    assert_eq!(Table::versions_of(&pair.phone.pages, page)?.len(), 2);
    assert_eq!(
        pair.phone.pages.title(page)?,
        "written by the one that was away",
        "the later write wins, as it would have had nothing been dropped"
    );
    assert_eq!(pair.phone.pages.position(page)?, 0);
    pair.phone.verify()?;
    Ok(())
}

#[test]
fn a_row_two_writers_left_in_disagreement_keeps_what_both_wrote() -> Result<()> {
    let dir = TempDir::new("prune_concurrent");
    let mut pair = Pair::new(&dir)?;
    let page = pair.phone.pages.push("shared", 0)?;
    pair.phone.commit(PHONE)?;
    pair.settle()?;
    pair.phone.pages.set_position(page, 1)?;
    pair.phone.commit(PHONE)?;
    pair.desk.pages.set_position(page, 2)?;
    pair.desk.commit(DESK)?;
    pair.settle()?;
    assert_eq!(pair.phone.conflicts()?.len(), 1);

    assert_eq!(pair.phone.prune(PHONE, &[DESK])?.dropped, 0);
    assert_eq!(pair.desk.prune(DESK, &[PHONE])?.dropped, 0);

    pair.phone.pages.set_position(page, 3)?;
    pair.phone.commit(PHONE)?;
    pair.settle()?;
    assert_eq!(pair.phone.prune(PHONE, &[DESK])?.dropped, 2);
    assert_eq!(pair.desk.prune(DESK, &[PHONE])?.dropped, 1);
    pair.settle()?;
    assert_eq!(pair.desk.pages.position(page)?, 3);
    assert!(pair.desk.conflicts()?.is_empty());
    assert!(pair.phone.conflicts()?.is_empty());
    Ok(())
}

#[test]
fn a_removed_row_stays_removed_when_everything_before_its_removal_is_dropped() -> Result<()> {
    let dir = TempDir::new("prune_removed");
    let mut pair = Pair::new(&dir)?;
    let page = pair.phone.pages.push("torn out", 0)?;
    pair.phone.commit(PHONE)?;
    turned(&mut pair.phone, PHONE, page, 3)?;
    pair.phone.remove(page)?;
    pair.phone.commit(PHONE)?;
    pair.settle()?;

    assert_eq!(pair.phone.prune(PHONE, &[DESK])?.dropped, 4);

    assert_eq!(pair.phone.status(page)?, RowStatus::Deleted);
    assert!(pair.phone.at(page).is_err());
    pair.settle()?;
    assert_eq!(pair.desk.status(page)?, RowStatus::Deleted);
    std::fs::remove_dir_all(pair.desk_dir.join("local"))?;
    let fresh = Journal::open(&pair.desk_dir)?;
    assert_eq!(fresh.status(page)?, RowStatus::Deleted);
    assert_eq!(fresh.all::<Page>().count(), 0);
    Ok(())
}

#[test]
fn an_entity_that_keeps_its_history_is_never_pruned() -> Result<()> {
    let dir = TempDir::new("prune_history");
    let mut pair = Pair::new(&dir)?;
    let entry = pair.phone.entries.push("first draft")?;
    pair.phone.commit(PHONE)?;
    for draft in ["second draft", "third draft"] {
        pair.phone.entries.set_text(entry, draft)?;
        pair.phone.commit(PHONE)?;
    }
    pair.settle()?;

    assert_eq!(pair.phone.prune(PHONE, &[DESK])?.dropped, 0);
    assert_eq!(pair.phone.history(entry)?.len(), 3);
    Ok(())
}

#[test]
fn a_third_writer_that_never_held_the_dropped_revisions_reads_the_same_row() -> Result<()> {
    let dir = TempDir::new("prune_third");
    let mut pair = Pair::new(&dir)?;
    let page = pair.phone.pages.push("reading position", 0)?;
    pair.phone.commit(PHONE)?;
    turned(&mut pair.phone, PHONE, page, 4)?;
    pair.desk.pages.push("another", 9)?;
    pair.desk.commit(DESK)?;
    pair.settle()?;
    assert_eq!(pair.phone.prune(PHONE, &[DESK])?.dropped, 4);
    pair.settle()?;

    let tablet_dir = dir.join("tablet");
    let mut tablet = Journal::create(&tablet_dir, TABLET)?;
    carry(&pair.phone_dir, &tablet_dir, PHONE);
    carry(&pair.desk_dir, &tablet_dir, DESK);
    let told = tablet.refresh_peers()?;

    assert_eq!((told.taken, told.waiting), (2, 0));
    assert!(told.is_quiet(), "{told:?}");
    assert_eq!(tablet.pages.position(page)?, 4);
    tablet.pages.set_position(page, 5)?;
    tablet.commit(TABLET)?;
    carry(&tablet_dir, &pair.phone_dir, TABLET);
    pair.phone.refresh_peers()?;
    assert_eq!(pair.phone.pages.position(page)?, 5);
    assert!(pair.phone.conflicts()?.is_empty());
    Ok(())
}

#[test]
fn a_revision_holding_what_an_older_build_could_not_write_is_kept_for_it() -> Result<()> {
    let dir = TempDir::new("prune_carrier");
    let (new_dir, old_dir) = (dir.join("desk"), dir.join("phone"));
    let mut desk = newer::Journal::create(&new_dir, DESK)?;
    let mut phone = older::Journal::create(&old_dir, PHONE)?;
    let page = desk.pages.push("chapter one", Some(7))?;
    desk.commit(DESK)?;
    carry(&new_dir, &old_dir, DESK);
    phone.refresh_peers()?;
    phone
        .pages
        .set_title(page.cast(), "chapter one, retitled")?;
    phone.commit(PHONE)?;
    for _ in 0..2 {
        carry(&old_dir, &new_dir, PHONE);
        carry(&new_dir, &old_dir, DESK);
        desk.refresh_peers()?;
        phone.refresh_peers()?;
        desk.acknowledge(DESK)?;
        phone.acknowledge(PHONE)?;
    }
    carry(&old_dir, &new_dir, PHONE);

    assert_eq!(
        desk.prune(DESK, &[PHONE])?.dropped,
        0,
        "the phone's revision has no mark of its own"
    );

    std::fs::remove_dir_all(new_dir.join("local"))?;
    let rebuilt = newer::Journal::open(&new_dir)?;
    assert_eq!(rebuilt.pages.title(page)?, "chapter one, retitled");
    assert_eq!(rebuilt.pages.mark(page)?, Some(7));
    Ok(())
}
