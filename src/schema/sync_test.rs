use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::id::Id;
use crate::schema::{Layout, RowStatus, Store, Table};
use crate::storage::column_file::{CommitFault, fail_commit_at, fail_rewrite_at};
use crate::storage::{ColumnFile, WriterId};
use crate::test_utils::{TempDir, cache, shared_files};

#[crate::entity]
struct Book {
    #[index]
    title: String,
    chapters: Many<Chapter>,
}

#[crate::entity]
struct Chapter {
    book: One<Book>,
    #[order(book)]
    number: u32,
    text: String,
}

#[crate::container(schema = "shelf", version = 1)]
struct Shelf {
    books: Set<Book>,
    chapters: Set<Chapter>,
}

fn here() -> WriterId {
    WriterId::from_raw(0x11)
}

fn there() -> WriterId {
    WriterId::from_raw(0x22)
}

fn deliver(from: &Path, to: &Path, writer: WriterId, entities: &[&str]) {
    for entity in entities {
        let name = format!("{:016x}.v1.dbs", writer.raw());
        let source = from.join(entity).join(&name);
        if source.exists() {
            std::fs::create_dir_all(to.join(entity)).unwrap();
            std::fs::copy(source, to.join(entity).join(&name)).unwrap();
        }
    }
}

fn exchange(ours: &Path, theirs: &Path) {
    deliver(ours, theirs, here(), &["book", "chapter"]);
    deliver(theirs, ours, there(), &["book", "chapter"]);
}

fn titles(shelf: &Shelf) -> Result<Vec<String>> {
    let mut titles = shelf
        .all::<Book>()
        .map(|book| Ok(book.title()?.into_owned()))
        .collect::<Result<Vec<_>>>()?;
    titles.sort();
    Ok(titles)
}

fn two_shelves(dir: &TempDir) -> Result<(Shelf, PathBuf, Shelf, PathBuf)> {
    let (ours, theirs) = (dir.join("ours"), dir.join("theirs"));
    Ok((
        Shelf::create(&ours, here())?,
        ours,
        Shelf::create(&theirs, there())?,
        theirs,
    ))
}

#[test]
fn a_commit_appends_only_to_the_shards_named_for_its_writer() -> Result<()> {
    let dir = TempDir::new("sync_own");
    let (mut ours, our_dir, mut theirs, their_dir) = two_shelves(&dir)?;
    let emma = theirs.books.push("Emma")?;
    theirs
        .chapters
        .push(emma, 1, "Emma Woodhouse, handsome, clever, and rich")?;
    theirs.commit(there())?;
    exchange(&our_dir, &their_dir);
    ours.refresh_peers()?;
    let delivered = shared_files(&our_dir);

    let persuasion = ours.books.push("Persuasion")?;
    ours.chapters.set_text(
        ours.children_of::<Book, Chapter>(emma)?[0],
        "Emma Woodhouse",
    )?;
    ours.chapters.push(persuasion, 1, "Sir Walter Elliot")?;
    ours.commit(here())?;

    let after = shared_files(&our_dir);
    for (path, bytes) in &delivered {
        assert_eq!(
            after.get(path),
            Some(bytes),
            "{} was rewritten",
            path.display()
        );
    }
    let added: Vec<_> = after
        .keys()
        .filter(|path| !delivered.contains_key(*path))
        .collect();
    assert_eq!(
        added,
        [
            Path::new("book/0000000000000011.v1.dbs"),
            Path::new("chapter/0000000000000011.v1.dbs")
        ]
    );
    Ok(())
}

#[test]
fn two_writers_converge_by_exchanging_files_and_then_fall_silent() -> Result<()> {
    let dir = TempDir::new("sync_converge");
    let (mut ours, our_dir, mut theirs, their_dir) = two_shelves(&dir)?;
    let emma = ours.books.push("Emma")?;
    ours.commit(here())?;
    exchange(&our_dir, &their_dir);
    assert_eq!(theirs.refresh_peers()?.taken, 1);

    ours.books.set_title(emma, "Emma, revised here")?;
    ours.commit(here())?;
    theirs.books.set_title(emma, "Emma, revised there")?;
    theirs.books.push("Sanditon")?;
    theirs.commit(there())?;
    exchange(&our_dir, &their_dir);
    let told = ours.refresh_peers()?;
    let heard = theirs.refresh_peers()?;

    assert_eq!((told.taken, told.known), (2, 0));
    assert_eq!((heard.taken, heard.known), (1, 0));
    assert_eq!(told.concurrent.len(), 1);
    assert_eq!(heard.concurrent, told.concurrent);
    assert_eq!(titles(&ours)?, titles(&theirs)?);
    assert_eq!(ours.clock(), theirs.clock());
    ours.verify()?;
    theirs.verify()?;

    let (shared, cached) = (shared_files(&our_dir), ours.generation());
    exchange(&our_dir, &their_dir);
    let quiet = ours.refresh_peers()?;
    assert!(!quiet.changed_anything() && quiet.is_quiet(), "{quiet:?}");
    assert_eq!(quiet.taken + quiet.known, 0, "nothing was read again");
    assert_eq!(shared_files(&our_dir), shared);
    assert_eq!(
        ours.generation(),
        cached,
        "a refresh that took nothing wrote nothing"
    );
    Ok(())
}

#[test]
fn a_lost_cache_is_rebuilt_from_every_writers_shards() -> Result<()> {
    let dir = TempDir::new("sync_rebuild");
    let (mut ours, our_dir, mut theirs, their_dir) = two_shelves(&dir)?;
    let emma = ours.books.push("Emma")?;
    ours.chapters.push(emma, 1, "one")?;
    ours.commit(here())?;
    theirs.books.push("Sanditon")?;
    theirs.commit(there())?;
    exchange(&our_dir, &their_dir);
    ours.refresh_peers()?;
    ours.books.remove(emma)?;
    ours.commit(here())?;
    let (held, clock, statuses) = (titles(&ours)?, ours.clock().clone(), ours.statuses()?);
    drop(ours);

    std::fs::remove_dir_all(our_dir.join("local"))?;
    let rebuilt = Shelf::open(&our_dir)?;

    assert_eq!(titles(&rebuilt)?, held);
    assert_eq!(rebuilt.clock(), &clock);
    assert_eq!(rebuilt.statuses()?, statuses);
    assert_eq!(Table::versions_of(&rebuilt.books, emma)?.len(), 2);
    rebuilt.verify()?;

    std::fs::write(cache(&our_dir), b"not a column file")?;
    assert_eq!(titles(&Shelf::open(&our_dir)?)?, held);
    Ok(())
}

#[test]
fn a_half_delivered_shard_is_reported_and_taken_once_it_is_whole() -> Result<()> {
    let dir = TempDir::new("sync_partial");
    let (mut ours, our_dir, mut theirs, their_dir) = two_shelves(&dir)?;
    theirs.books.push("Sanditon")?;
    theirs.commit(there())?;
    let whole = their_dir.join("book/0000000000000022.v1.dbs");
    let arriving = our_dir.join("book/0000000000000022.v1.dbs");
    std::fs::create_dir_all(arriving.parent().unwrap())?;
    let bytes = std::fs::read(&whole)?;
    std::fs::write(&arriving, &bytes[..bytes.len() - 7])?;

    let told = ours.refresh_peers()?;
    assert_eq!(told.unreadable, std::slice::from_ref(&arriving));
    assert_eq!(told.taken, 0);
    assert!(titles(&ours)?.is_empty());

    std::fs::write(&arriving, &bytes[..4000])?;
    assert_eq!(
        ours.refresh_peers()?.unreadable,
        std::slice::from_ref(&arriving)
    );

    std::fs::write(&arriving, &bytes)?;
    let told = ours.refresh_peers()?;
    assert!(told.unreadable.is_empty());
    assert_eq!(titles(&ours)?, ["Sanditon"]);
    Ok(())
}

#[test]
fn a_file_that_is_not_a_shard_is_named_and_left_alone() -> Result<()> {
    let dir = TempDir::new("sync_stranger");
    let (mut ours, our_dir, _, _) = two_shelves(&dir)?;
    ours.books.push("Emma")?;
    ours.commit(here())?;
    let conflict = our_dir.join("book/0000000000000011.v1.sync-conflict-20261111-AAAAAAA.dbs");
    std::fs::copy(our_dir.join("book/0000000000000011.v1.dbs"), &conflict)?;

    let told = ours.refresh_peers()?;

    assert_eq!(told.strangers, std::slice::from_ref(&conflict));
    assert_eq!(told.taken, 0);
    assert!(conflict.exists());
    assert_eq!(titles(&Shelf::open(&our_dir)?)?, ["Emma"]);
    Ok(())
}

#[test]
fn a_child_delivered_before_its_parent_waits_hidden_until_the_parent_arrives() -> Result<()> {
    let dir = TempDir::new("sync_order");
    let (mut ours, our_dir, mut theirs, their_dir) = two_shelves(&dir)?;
    let emma = theirs.books.push("Emma")?;
    let first = theirs.chapters.push(emma, 1, "one")?;
    theirs.commit(there())?;

    deliver(&their_dir, &our_dir, there(), &["chapter"]);
    let early = ours.refresh_peers()?;
    assert_eq!(early.taken, 1);
    assert_eq!(early.references.len(), 1);
    assert!(matches!(ours.status(first)?, RowStatus::Suppressed(_)));
    assert!(ours.at(first).is_err());
    assert_eq!(ours.dangling()?.len(), 1);
    ours.verify()?;
    let reopened = Shelf::open(&our_dir)?;
    assert!(matches!(reopened.status(first)?, RowStatus::Suppressed(_)));
    drop(reopened);

    deliver(&their_dir, &our_dir, there(), &["book"]);
    let late = ours.refresh_peers()?;
    assert_eq!(late.taken, 1);
    assert!(
        late.visibility
            .iter()
            .any(|change| change.id == first.uuid() && change.after == RowStatus::Visible)
    );
    assert_eq!(ours.at(first)?.book()?.id()?, emma);
    assert!(ours.dangling()?.is_empty());
    Ok(())
}

#[test]
fn a_commit_that_failed_part_way_is_finished_by_the_next_one_without_a_second_revision()
-> Result<()> {
    let dir = TempDir::new("sync_retry");
    let (mut ours, our_dir, _, _) = two_shelves(&dir)?;
    let emma = ours.books.push("Emma")?;
    ours.chapters.push(emma, 1, "one")?;

    fail_commit_at(CommitFault::Payload);
    assert!(ours.commit(here()).is_err());
    assert!(
        ours.shard_marks().is_empty(),
        "nothing was recorded as written"
    );
    assert_eq!(ours.books.title(emma)?, "Emma");

    ours.commit(here())?;

    assert_eq!(Table::versions_of(&ours.books, emma)?.len(), 1);
    std::fs::remove_dir_all(our_dir.join("local"))?;
    let rebuilt = Shelf::open(&our_dir)?;
    assert_eq!(titles(&rebuilt)?, ["Emma"]);
    assert_eq!(rebuilt.children_of::<Book, Chapter>(emma)?.len(), 1);
    assert_eq!(Table::versions_of(&rebuilt.books, emma)?.len(), 1);
    Ok(())
}

#[test]
fn a_failed_compaction_leaves_the_store_as_it_was() -> Result<()> {
    let dir = TempDir::new("sync_compact");
    let (mut ours, our_dir, _, _) = two_shelves(&dir)?;
    let emma = ours.books.push("Emma")?;
    ours.commit(here())?;
    for round in 0..4 {
        ours.books
            .set_title(emma, &format!("Emma, draft {round}"))?;
        ours.commit(here())?;
    }
    let before = ColumnFile::read_commit_token(cache(&our_dir))?;

    fail_rewrite_at("before_replace");
    assert!(ours.compact().is_err());
    assert_eq!(ColumnFile::read_commit_token(cache(&our_dir))?, before);
    assert_eq!(ours.books.title(emma)?, "Emma, draft 3");

    ours.compact()?;
    ours.verify()?;
    assert_eq!(ours.books.title(emma)?, "Emma, draft 3");
    ours.books.set_title(emma, "Emma")?;
    ours.commit(here())?;
    assert_eq!(Shelf::open(&our_dir)?.books.title(emma)?, "Emma");
    Ok(())
}

#[test]
fn the_shared_and_local_halves_can_live_in_different_places() -> Result<()> {
    let dir = TempDir::new("sync_layout");
    let layout = Layout::split(dir.join("synced"), dir.join("device/cache"));
    let mut shelf = Shelf::create(layout.clone(), here())?;
    let emma: Id<Book> = shelf.books.push("Emma")?;
    shelf.commit(here())?;

    assert!(layout.shard("book", here(), 1).exists());
    assert!(layout.cache(1).exists());
    assert!(!dir.join("synced/local").exists());
    assert_eq!(Shelf::open(layout)?.books.title(emma)?, "Emma");
    Ok(())
}
