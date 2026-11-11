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
            Path::new("chapter/0000000000000011.v1.dbs"),
            Path::new("dbsync.meta/0000000000000011.dbs")
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
fn a_refresh_names_the_rows_it_took_a_revision_of() -> Result<()> {
    let dir = TempDir::new("sync_adopted");
    let (mut ours, our_dir, mut theirs, their_dir) = two_shelves(&dir)?;
    let emma = ours.books.push("Emma")?;
    ours.commit(here())?;
    exchange(&our_dir, &their_dir);
    assert_eq!(theirs.refresh_peers()?.adopted, [("book", emma.uuid())]);

    // An edit changes no row's visibility, and is named all the same.
    ours.books.set_title(emma, "Emma, revised")?;
    let chapter = ours.chapters.push(emma, 1, "text")?;
    ours.commit(here())?;
    exchange(&our_dir, &their_dir);
    let told = theirs.refresh_peers()?;
    let mut expected = vec![("book", emma.uuid()), ("chapter", chapter.uuid())];
    expected.sort_unstable();
    assert_eq!(told.adopted, expected);

    exchange(&our_dir, &their_dir);
    assert!(theirs.refresh_peers()?.adopted.is_empty());
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
    assert_eq!(
        told.taken, 0,
        "the commit whose directory is cut off is not read"
    );
    assert!(titles(&ours)?.is_empty());

    std::fs::write(&arriving, &bytes[..4000])?;
    assert_eq!(
        ours.refresh_peers()?.unreadable,
        std::slice::from_ref(&arriving)
    );
    assert!(titles(&ours)?.is_empty());

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

#[test]
fn a_writer_says_what_it_has_seen_when_that_is_news_to_the_others() -> Result<()> {
    let dir = TempDir::new("sync_seen");
    let (mut ours, our_dir, mut theirs, their_dir) = two_shelves(&dir)?;
    let emma = ours.books.push("Emma")?;
    ours.commit(here())?;
    let said = ours.peers()?;
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].writer, here());
    assert_eq!(&said[0].seen, ours.clock());

    let meta = Path::new("dbsync.meta/0000000000000011.dbs");
    let first = shared_files(&our_dir)[meta].clone();
    ours.books.set_title(emma, "Emma, again")?;
    ours.commit(here())?;
    assert_eq!(
        shared_files(&our_dir)[meta],
        first,
        "a commit that learned nothing from anyone has nothing new to tell them"
    );

    theirs.books.push("Sanditon")?;
    theirs.commit(there())?;
    exchange(&our_dir, &their_dir);
    ours.refresh_peers()?;
    assert_eq!(
        shared_files(&our_dir)[meta],
        first,
        "reading alone says nothing"
    );
    ours.acknowledge(here())?;
    let said = ours.peers()?;
    assert_eq!(said.len(), 1, "their own word has not been delivered here");
    assert!(said[0].seen.counter_of(there()) > 0);
    assert_eq!(&said[0].seen, ours.clock());
    Ok(())
}

#[test]
fn compacting_a_writers_shards_shrinks_them_without_anyone_reading_a_row_again() -> Result<()> {
    let dir = TempDir::new("sync_compact_shards");
    let (mut ours, our_dir, mut theirs, their_dir) = two_shelves(&dir)?;
    let emma = ours.books.push("Emma")?;
    ours.commit(here())?;
    for draft in 0..30 {
        ours.books
            .set_title(emma, &format!("Emma, draft {draft}"))?;
        ours.commit(here())?;
    }
    exchange(&our_dir, &their_dir);
    assert_eq!(theirs.refresh_peers()?.taken, 31);
    let shard = our_dir.join("book/0000000000000011.v1.dbs");
    let before = std::fs::metadata(&shard)?.len();

    let report = ours.compact_shards(here())?;

    let after = std::fs::metadata(&shard)?.len();
    assert!(after < before, "{after} < {before}");
    assert_eq!((report.before_bytes, report.after_bytes), (before, after));
    assert_eq!(ours.refresh_peers()?.taken + ours.refresh_peers()?.known, 0);
    assert_eq!(Table::versions_of(&ours.books, emma)?.len(), 31);
    ours.verify()?;

    exchange(&our_dir, &their_dir);
    let told = theirs.refresh_peers()?;
    assert_eq!(
        (told.taken, told.known),
        (0, 31),
        "the same rows in a new file"
    );
    assert!(!told.changed_anything());

    ours.books.set_title(emma, "Emma")?;
    ours.commit(here())?;
    let merged = theirs.merge_from(&our_dir, there())?;
    assert_eq!(merged.taken, 1);
    assert_eq!(theirs.books.title(emma)?, "Emma");
    Ok(())
}

#[test]
fn opening_a_store_reads_no_row_of_any_table_and_a_snapshot_copies_none() -> Result<()> {
    let dir = TempDir::new("sync_cold_open");
    let (mut ours, our_dir, _, _) = two_shelves(&dir)?;
    let mut last = None;
    for book in 0..40 {
        let made = ours.books.push(&format!("Book {book}"))?;
        for chapter in 0..60 {
            ours.chapters
                .push(made, chapter, "a chapter of some length")?;
        }
        last = Some(made);
    }
    ours.commit(here())?;
    drop(ours);

    let opened = Shelf::open(&our_dir)?;
    let read: Vec<_> = opened
        .residency()
        .into_iter()
        .filter(|column| column.resident > 0)
        .map(|column| column.name)
        .collect();
    assert_eq!(
        read,
        [
            "dbsync.clock",
            "dbsync.shards",
            "dbsync.durable",
            "dbsync.schema"
        ]
    );

    let copy = opened.snapshot();
    assert!(
        copy.residency()
            .into_iter()
            .all(|column| column.resident == 0 || column.name.starts_with("dbsync.")),
        "a snapshot shares what it has not read"
    );

    let book = opened.at(last.unwrap())?;
    assert_eq!(book.title()?, "Book 39");
    assert_eq!(book.chapters()?.count(), 60);
    let touched = |name: &str| {
        opened
            .residency()
            .into_iter()
            .find(|column| column.name == name)
            .map(|column| (column.resident, column.chunks))
            .unwrap()
    };
    assert_eq!(touched("chapter.text.bytes").0, 0);
    assert_eq!(touched("chapter.book").0, 0);
    assert_eq!(touched("book.title.bytes"), (1, 1));
    opened.verify()?;
    Ok(())
}

#[test]
fn a_cache_whose_unflushed_tail_did_not_land_is_rebuilt_from_the_shards() -> Result<()> {
    let dir = TempDir::new("sync_torn_cache");
    let (mut ours, our_dir, _, _) = two_shelves(&dir)?;
    let emma = ours.books.push("Emma")?;
    ours.commit(here())?;
    ours.books.set_title(emma, "Emma, second edition")?;
    ours.commit(here())?;
    drop(ours);

    let file = ColumnFile::open(cache(&our_dir))?;
    let newest = file
        .columns()
        .iter()
        .flat_map(|column| column.chunks.iter())
        .max_by_key(|chunk| chunk.offset)
        .copied()
        .unwrap();
    drop(file);
    crate::test_utils::flip_byte(&cache(&our_dir), newest.offset + newest.physical_len() - 1);

    let reopened = Shelf::open(&our_dir)?;
    assert_eq!(reopened.books.title(emma)?, "Emma, second edition");
    assert_eq!(Table::versions_of(&reopened.books, emma)?.len(), 2);
    reopened.verify()?;
    Ok(())
}

#[test]
fn a_store_commits_and_reopens_the_same_under_every_durability() -> Result<()> {
    use crate::storage::Durability;

    let dir = TempDir::new("sync_durability");
    assert_eq!(Shelf::default().durability(), Durability::Relaxed);
    for (name, durability) in [
        ("none", Durability::None),
        ("relaxed", Durability::Relaxed),
        ("full", Durability::Full),
    ] {
        let path = dir.join(name);
        let mut shelf = Shelf::create(&path, here())?;
        shelf.set_durability(durability);
        let emma = shelf.books.push("Emma")?;
        shelf.commit(here())?;
        shelf.books.set_title(emma, "Emma, again")?;
        shelf.commit(here())?;
        assert_eq!(shelf.snapshot().durability(), durability);
        assert!(shelf.compact_shards(here())?.before_bytes > 0);
        drop(shelf);

        let reopened = Shelf::open(&path)?;
        assert_eq!(reopened.books.title(emma)?, "Emma, again");
        reopened.verify()?;
        std::fs::remove_dir_all(path.join("local"))?;
        assert_eq!(Shelf::open(&path)?.books.title(emma)?, "Emma, again");
    }
    Ok(())
}
