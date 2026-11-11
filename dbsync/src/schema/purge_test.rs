use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::id::Id;
use crate::schema::{AnyTable, PurgeReport, RowStatus, Store, Table};
use crate::storage::{ColumnFile, WriterId};
use crate::test_utils::{TempDir, cache, files_holding};

#[crate::field_enum]
#[derive(PartialEq, Eq, Debug)]
enum Binding {
    Cloth = 3,
    Paper = 5,
}

#[crate::entity]
struct Book {
    #[index]
    title: String,
    subtitle: Option<String>,
    cover: Vec<u8>,
    #[sorted]
    year: u32,
    binding: Binding,
    chapters: Many<Chapter>,
    notes: Many<Note>,
    #[through(Credit)]
    authors: Many<Author>,
}

#[crate::entity]
struct Author {
    #[index]
    name: String,
    #[through(Credit)]
    books: Many<Book>,
}

#[crate::entity]
struct Credit {
    book: One<Book>,
    author: One<Author>,
    role: String,
}

#[crate::entity]
struct Chapter {
    book: One<Book>,
    #[order(book)]
    number: u32,
    #[index]
    heading: String,
    paragraphs: Many<Paragraph>,
}

#[crate::entity]
#[merge(history)]
struct Paragraph {
    chapter: One<Chapter>,
    text: String,
}

#[crate::entity]
struct Note {
    book: Option<One<Book>>,
    body: String,
    rank: Option<u32>,
    scan: Option<Vec<u8>>,
    binding: Option<Binding>,
}

#[crate::entity]
struct Link {
    target: One<Link>,
    label: String,
}

#[crate::container(schema = "library", version = 1)]
struct Library {
    books: Set<Book>,
    authors: Set<Author>,
    credits: Set<Credit>,
    chapters: Set<Chapter>,
    paragraphs: Set<Paragraph>,
    notes: Set<Note>,
    links: Set<Link>,
}

const PHONE: WriterId = WriterId::from_raw(0x51);
const DESK: WriterId = WriterId::from_raw(0x52);
const TABLET: WriterId = WriterId::from_raw(0x53);

const FOLDERS: [&str; 7] = [
    "book",
    "author",
    "credit",
    "chapter",
    "paragraph",
    "note",
    "link",
];

const TITLE: &str = "Zuleika Dobson";
const SUBTITLE: &str = "or, an Oxford love story, told at some length";
const COVER: &[u8] = b"\x89cover-of-zuleika\x00\x01";
const HEADING: &str = "The Duke arrives";
const TEXT: &str = "That old bell, presage of a train, had just sounded through Oxford station.";
const ROLE: &str = "wrote-zuleika";
const KEPT: &str = "Seven Men";

fn carry_some(from: &Path, to: &Path, writer: WriterId, folders: &[&str]) {
    let shard = format!("{:016x}.v1.dbs", writer.raw());
    let said = format!("{:016x}.dbs", writer.raw());
    for folder in folders {
        let name = match *folder {
            "dbsync.meta" => &said,
            _ => &shard,
        };
        let source = from.join(folder).join(name);
        if source.exists() {
            std::fs::create_dir_all(to.join(folder)).unwrap();
            let arriving = to.join(folder).join(format!(".{name}.tmp"));
            std::fs::copy(source, &arriving).unwrap();
            std::fs::rename(arriving, to.join(folder).join(name)).unwrap();
        }
    }
}

fn carry(from: &Path, to: &Path, writer: WriterId) {
    carry_some(from, to, writer, &FOLDERS);
    carry_some(from, to, writer, &["dbsync.meta"]);
}

struct Stocked {
    zuleika: Id<Book>,
    seven: Id<Book>,
    beerbohm: Id<Author>,
    credit: Id<Credit>,
    chapters: [Id<Chapter>; 2],
    paragraphs: [Id<Paragraph>; 3],
    pinned: Id<Note>,
    loose: Id<Note>,
}

fn stock(library: &mut Library) -> Result<Stocked> {
    let beerbohm = library.authors.push("Max Beerbohm")?;
    let zuleika = library
        .books
        .push(TITLE, Some(SUBTITLE), COVER, 1911, Binding::Paper)?;
    let credit = library.credits.push(zuleika, beerbohm, ROLE)?;
    let first = library.chapters.push(zuleika, 1, HEADING)?;
    let second = library.chapters.push(zuleika, 2, "Zuleika dines")?;
    let paragraphs = [
        library.paragraphs.push(first, TEXT)?,
        library.paragraphs.push(first, "second of the first")?,
        library.paragraphs.push(second, "first of the second")?,
    ];
    let seven = library.books.push(KEPT, None, &[], 1919, Binding::Cloth)?;
    library.credits.push(seven, beerbohm, "wrote")?;
    let enoch = library.chapters.push(seven, 1, "Enoch Soames")?;
    library.paragraphs.push(enoch, "When a book about the literature of the eighteen-nineties was given by Mr. Holbrook Jackson to the world")?;
    let pinned = library.notes.push(
        Some(zuleika),
        "a note pinned to zuleika",
        Some(7),
        Some(&[1, 2, 3]),
        Some(Binding::Paper),
    )?;
    let loose = library.notes.push(None, "a loose note", None, None, None)?;
    Ok(Stocked {
        zuleika,
        seven,
        beerbohm,
        credit,
        chapters: [first, second],
        paragraphs,
        pinned,
        loose,
    })
}

fn refused<T>(said: Result<T>) {
    let Err(err) = said else {
        panic!("a purged row took a write");
    };
    assert!(err.to_string().contains("was purged"), "{err}");
}

fn needles() -> [&'static [u8]; 6] {
    [
        TITLE.as_bytes(),
        SUBTITLE.as_bytes(),
        COVER,
        HEADING.as_bytes(),
        TEXT.as_bytes(),
        ROLE.as_bytes(),
    ]
}

fn traces(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for needle in needles() {
        found.extend(files_holding(dir, needle));
    }
    found.sort();
    found.dedup();
    found
}

#[test]
fn purging_a_book_purges_every_row_that_needs_it_and_nothing_else() -> Result<()> {
    let dir = TempDir::new("purge_cascade");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;

    assert_eq!(library.purge(held.zuleika)?, 1 + 1 + 2 + 3);

    assert!(library.is_purged(held.zuleika)?);
    assert!(library.is_purged(held.credit)?);
    for chapter in held.chapters {
        assert!(library.is_purged(chapter)?);
        assert_eq!(library.status(chapter)?, RowStatus::Deleted);
    }
    for paragraph in held.paragraphs {
        assert!(library.is_purged(paragraph)?);
    }
    assert!(!library.is_purged(held.beerbohm)?);
    assert!(!library.is_purged(held.seven)?);
    assert!(!library.is_purged(held.pinned)?);
    assert!(!library.is_purged(Id::<Book>::mint())?);
    assert_eq!(library.status(held.zuleika)?, RowStatus::Deleted);

    assert!(library.books.find_by_title(TITLE)?.is_empty());
    assert_eq!(library.books.by_year(..)?, [held.seven]);
    assert!(library.chapters.find_by_heading(HEADING)?.is_empty());
    assert!(
        library
            .children_of::<Book, Chapter>(held.zuleika)?
            .is_empty()
    );
    assert!(
        library
            .children_of::<Chapter, Paragraph>(held.chapters[0])?
            .is_empty()
    );
    assert_eq!(library.all::<Book>().count(), 1);
    assert_eq!(library.all::<Chapter>().count(), 1);
    assert_eq!(library.all::<Paragraph>().count(), 1);
    assert_eq!(library.all::<Credit>().count(), 1);
    let written: Vec<_> = library
        .at(held.beerbohm)?
        .books()?
        .map(|book| book.id())
        .collect::<Result<_>>()?;
    assert_eq!(written, [held.seven]);
    assert_eq!(library.at(held.seven)?.chapters()?.count(), 1);
    assert!(library.at(held.zuleika).is_err());
    assert!(library.conflicts()?.iter().all(|conflict| matches!(
        conflict,
        crate::schema::Conflict::Reference(reason) if reason.id == held.pinned.uuid()
    )));
    library.audit()?;

    library.commit(PHONE)?;
    library.verify()?;
    let reopened = Library::open(dir.join("phone"))?;
    assert_eq!(reopened.statuses()?, library.statuses()?);
    assert!(reopened.is_purged(held.paragraphs[2])?);
    assert_eq!(reopened.all::<Book>().count(), 1);
    Ok(())
}

#[test]
fn a_purge_leaves_a_revision_that_carries_nothing_of_the_row() -> Result<()> {
    let dir = TempDir::new("purge_blank");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;
    let written = library.at(held.zuleika)?.version()?.dot();
    library.purge(held.zuleika)?;
    library.purge(held.pinned)?;
    library.commit(PHONE)?;

    let history = library.history(held.zuleika)?;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].title()?, TITLE);
    let left = &history[1];
    let version = left.version()?;
    assert!(version.is_purged() && version.is_removed() && !version.is_live());
    assert_eq!(left.parents()?, [crate::schema::Parent::Published(written)]);
    assert_eq!(left.title()?, "");
    assert_eq!(left.subtitle()?, None);
    assert!(left.cover()?.is_empty());
    assert_eq!(left.year()?, 0);
    assert_eq!(left.binding()?, Binding::Cloth, "the first variant");

    let chapter = library.history(held.chapters[0])?;
    let left = chapter.last().unwrap();
    assert!(left.parent_id::<Book>()?.is_nil());
    assert_eq!((left.number()?, left.heading()?.as_ref()), (0, ""));

    let note = library.history(held.pinned)?;
    let left = note.last().unwrap();
    assert!(left.book()?.is_none());
    assert_eq!(left.body()?, "");
    assert_eq!(left.rank()?, None);
    assert_eq!(left.scan()?, None);
    assert_eq!(left.binding()?, None);

    for table in library.tables() {
        let blanks = match table.entity() {
            "book" => library.books.blank_values()?,
            "chapter" => library.chapters.blank_values()?,
            "paragraph" => library.paragraphs.blank_values()?,
            "credit" => library.credits.blank_values()?,
            "note" => library.notes.blank_values()?,
            _ => continue,
        };
        let mut purges = 0;
        for row in table.export_rows()? {
            if row.version.is_purged() {
                let values: Vec<Vec<u8>> = row.values.into_iter().map(|(_, bytes)| bytes).collect();
                assert_eq!(values, blanks, "{}", table.entity());
                purges += 1;
            }
        }
        assert!(purges > 0, "{}", table.entity());
    }
    Ok(())
}

#[test]
fn a_purged_row_refuses_every_write_and_cannot_be_made_again() -> Result<()> {
    let dir = TempDir::new("purge_final");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;
    let written = library.at(held.zuleika)?.version()?.dot();
    library.purge(held.zuleika)?;
    let rows = library.books.rows();

    refused(library.restore(held.zuleika, written));
    refused(library.resolve(held.zuleika, written));
    refused(library.remove(held.zuleika));
    refused(library.books.remove(held.zuleika));
    refused(library.books.set_title(held.zuleika, "again"));
    refused(library.books.set_year(held.zuleika, 1912));
    refused(library.at_hidden_mut(held.zuleika).map(|_| ()));
    refused(library.at_hidden(held.zuleika).map(|_| ()));
    refused(library.books.check_unpurged(held.zuleika));
    refused(Table::revise(&mut library.books, held.zuleika));
    refused(Table::begin_recovery(
        &mut library.books,
        held.zuleika,
        true,
    ));
    refused(Table::begin_recovery(
        &mut library.books,
        held.zuleika,
        false,
    ));
    assert!(library.at_mut(held.zuleika).is_err());
    let again = library
        .books
        .insert(held.zuleika, TITLE, None, &[], 1911, Binding::Paper);
    assert!(again.unwrap_err().to_string().contains("already has a row"));
    assert_eq!(library.books.rows(), rows, "nothing was written");

    assert_eq!(library.purge(held.zuleika)?, 0, "purging twice says so");
    assert!(!library.books.purge(held.zuleika)?);
    assert_eq!(library.books.rows(), rows);
    assert_eq!(library.at_version(held.zuleika, written)?.title()?, TITLE);
    assert_eq!(library.history(held.zuleika)?.len(), 2);
    library.books.check_unpurged(held.seven)?;
    library.audit()?;

    library.commit(PHONE)?;
    refused(library.restore(held.zuleika, written));
    refused(library.books.set_title(held.zuleika, "again"));
    library.reclaim_purged(PHONE)?;
    refused(library.restore(held.zuleika, written));
    assert!(library.at_version(held.zuleika, written).is_err());
    assert_eq!(library.history(held.zuleika)?.len(), 1);
    assert_eq!(library.history(held.zuleika)?[0].title()?, "");
    Ok(())
}

#[test]
fn a_row_that_may_stand_without_the_purged_one_is_hidden_until_it_lets_go() -> Result<()> {
    let dir = TempDir::new("purge_optional");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;

    library.purge(held.zuleika)?;

    assert!(matches!(
        library.status(held.pinned)?,
        RowStatus::Suppressed(_)
    ));
    assert_eq!(library.status(held.loose)?, RowStatus::Visible);
    library.at_hidden_mut(held.pinned)?.set_book(None)?;
    assert_eq!(library.status(held.pinned)?, RowStatus::Visible);
    assert_eq!(library.at(held.pinned)?.body()?, "a note pinned to zuleika");
    library.commit(PHONE)?;
    assert_eq!(library.reclaim_purged(PHONE)?.purged, 0);
    assert_eq!(library.at(held.pinned)?.rank()?, Some(7));
    Ok(())
}

#[test]
fn a_row_already_taken_out_is_purged_with_the_one_it_named() -> Result<()> {
    let dir = TempDir::new("purge_removed");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let held = stock(&mut library)?;
    library.remove(held.chapters[0])?;
    library.commit(PHONE)?;
    assert!(!library.is_purged(held.chapters[0])?);

    assert_eq!(library.purge(held.zuleika)?, 7);

    assert!(library.is_purged(held.chapters[0])?);
    assert!(library.is_purged(held.paragraphs[0])?);
    Ok(())
}

#[test]
fn rows_that_name_each_other_are_purged_once_each() -> Result<()> {
    let dir = TempDir::new("purge_cycle");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let a = library.links.push(Id::NIL, "a")?;
    let b = library.links.push(a, "b")?;
    let apart = library.links.push(Id::NIL, "apart")?;
    library.links.set_target(a, b)?;
    library.commit(PHONE)?;

    assert_eq!(library.purge(a)?, 2);

    assert!(library.is_purged(a)? && library.is_purged(b)?);
    assert_eq!(library.status(apart)?, RowStatus::Visible);
    assert_eq!(library.links.rows(), 6);
    library.audit()?;
    Ok(())
}

#[test]
fn purging_what_was_never_there_changes_nothing() -> Result<()> {
    let dir = TempDir::new("purge_unknown");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    stock(&mut library)?;
    let before = library.statuses()?;

    let said = library.purge(Id::<Book>::mint());

    assert!(said.unwrap_err().to_string().contains("to purge"));
    assert!(
        library
            .purge_cascading("shelf", crate::Uuid::now_v7())
            .is_err()
    );
    assert_eq!(library.statuses()?, before);
    assert!(library.books.purged_ids()?.is_empty());
    Ok(())
}

#[test]
fn a_row_purged_before_it_was_ever_published_is_published_as_purged() -> Result<()> {
    let dir = TempDir::new("purge_pending");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let held = stock(&mut library)?;

    assert_eq!(library.purge(held.zuleika)?, 7);
    library.commit(PHONE)?;

    let report = library.reclaim_purged(PHONE)?;
    assert_eq!(
        report,
        PurgeReport {
            purged: 0,
            dropped: 7
        }
    );
    assert!(traces(&dir.join("phone")).is_empty());
    let reopened = Library::open(dir.join("phone"))?;
    assert!(reopened.is_purged(held.zuleika)?);
    assert_eq!(reopened.all::<Book>().count(), 1);
    reopened.verify()?;
    Ok(())
}

#[test]
fn a_purge_through_one_table_reaches_the_rest_when_the_store_reclaims() -> Result<()> {
    let dir = TempDir::new("purge_table");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;

    assert!(library.books.purge(held.zuleika)?);
    assert!(library.books.is_purged(held.zuleika)?);
    assert!(!library.chapters.is_purged(held.chapters[0])?);
    assert!(matches!(
        library.status(held.chapters[0])?,
        RowStatus::Suppressed(_)
    ));
    library.commit(PHONE)?;

    let report = library.reclaim_purged(PHONE)?;

    assert_eq!(
        report,
        PurgeReport {
            purged: 6,
            dropped: 7
        }
    );
    assert!(library.is_purged(held.paragraphs[1])?);
    assert!(traces(&dir.join("phone")).is_empty());
    library.verify()?;
    Ok(())
}

#[test]
fn reclaiming_after_a_purge_leaves_none_of_the_row_in_any_file() -> Result<()> {
    let dir = TempDir::new("purge_reclaim");
    let phone_dir = dir.join("phone");
    let mut library = Library::create(&phone_dir, PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;
    let first = library.clock().counter_of(PHONE);
    library.books.set_year(held.zuleika, 1912)?;
    library
        .paragraphs
        .set_text(held.paragraphs[0], &format!("{TEXT} Revised."))?;
    library.commit(PHONE)?;
    for needle in needles() {
        let found = files_holding(&phone_dir, needle);
        assert!(found.contains(&PathBuf::from("local/cache.v1.r2.dbs")));
        assert_eq!(found.len(), 2, "the cache and one shard: {found:?}");
    }

    library.purge(held.zuleika)?;
    library.commit(PHONE)?;
    assert!(!traces(&phone_dir).is_empty(), "a tombstone hides, no more");
    let shard = library.layout().unwrap().shard("paragraph", PHONE, 1);
    let before = std::fs::metadata(&shard)?.len();

    let report = library.reclaim_purged(PHONE)?;

    assert_eq!(
        report,
        PurgeReport {
            purged: 0,
            dropped: 9
        }
    );
    assert_eq!(traces(&phone_dir), Vec::<PathBuf>::new());
    assert!(std::fs::metadata(&shard)?.len() < before);
    let dropped = library.clock().counter_of(PHONE) - 1;
    for mark in library.shard_marks() {
        let rewritten = [
            library.layout().unwrap().shard("book", PHONE, 1),
            library.layout().unwrap().shard("paragraph", PHONE, 1),
        ]
        .iter()
        .any(|path| {
            ColumnFile::read_commit_token(path).is_ok_and(|token| token.file_uuid == mark.file_uuid)
        });
        if rewritten {
            assert!(
                (first..dropped).contains(&mark.floor),
                "what was dropped is gone for good, and readers are told: {mark:?}"
            );
        }
    }
    assert_eq!(
        library
            .shard_marks()
            .iter()
            .filter(|mark| mark.floor > 0)
            .count(),
        4,
        "book, credit, chapter and paragraph"
    );
    assert_eq!(files_holding(&phone_dir, KEPT.as_bytes()).len(), 2);
    assert_eq!(Table::versions_of(&library.books, held.zuleika)?.len(), 1);
    assert_eq!(
        Table::versions_of(&library.paragraphs, held.paragraphs[0])?.len(),
        1,
        "an entity that keeps its history gives up a purged row all the same"
    );
    assert!(library.is_purged(held.zuleika)?);
    assert_eq!(library.at(held.seven)?.title()?, KEPT);
    assert_eq!(library.at(held.seven)?.chapters()?.count(), 1);
    assert!(library.books.purged_payload()?.is_empty());
    library.verify()?;

    let generation = library.generation();
    assert_eq!(library.reclaim_purged(PHONE)?, PurgeReport::default());
    assert_eq!(library.generation(), generation, "nothing was written");
    drop(library);

    std::fs::remove_dir_all(phone_dir.join("local"))?;
    let rebuilt = Library::open(&phone_dir)?;
    assert!(rebuilt.is_purged(held.chapters[1])?);
    assert_eq!(rebuilt.all::<Paragraph>().count(), 1);
    assert!(traces(&phone_dir).is_empty());
    rebuilt.verify()?;
    Ok(())
}

#[test]
fn reclaiming_where_nothing_was_purged_reads_and_writes_nothing() -> Result<()> {
    let dir = TempDir::new("purge_quiet");
    let phone_dir = dir.join("phone");
    let mut library = Library::create(&phone_dir, PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;
    let shards = crate::test_utils::shared_files(&phone_dir);
    let generation = library.generation();

    assert_eq!(library.reclaim_purged(PHONE)?, PurgeReport::default());
    library.books.set_year(held.seven, 1920)?;
    library.commit(PHONE)?;
    let committed = library.generation();
    assert_eq!(library.reclaim_purged(PHONE)?, PurgeReport::default());

    assert_ne!(committed, generation);
    assert_eq!(library.generation(), committed);
    let after = crate::test_utils::shared_files(&phone_dir);
    assert_eq!(
        after.get(Path::new("chapter/0000000000000051.v1.dbs")),
        shards.get(Path::new("chapter/0000000000000051.v1.dbs"))
    );
    assert_eq!(
        Table::versions_of(&library.books, held.seven)?.len(),
        2,
        "what was merely written over is pruning's to drop"
    );
    Ok(())
}

#[test]
fn only_new_rows_are_looked_at_once_a_store_has_reclaimed() -> Result<()> {
    let dir = TempDir::new("purge_news");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;
    assert!(!library.books.purge_news(0)?);
    assert!(!library.notes.purge_news(0)?);

    library.purge(held.zuleika)?;
    library.commit(PHONE)?;

    let rows = library.books.rows();
    assert!(library.books.purge_news(0)?);
    assert!(library.books.purge_news(rows - 1)?, "the tombstone itself");
    assert!(!library.books.purge_news(rows)?);
    assert!(
        library.notes.purge_news(0)?,
        "a hidden row may need purging"
    );
    assert!(!library.authors.purge_news(0)?);
    library.reclaim_purged(PHONE)?;
    assert_eq!(
        library.state().purges_settled,
        library
            .tables()
            .iter()
            .map(|table| table.row_count())
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn reclaiming_needs_a_named_writer_and_nothing_left_unpublished() -> Result<()> {
    let dir = TempDir::new("purge_guard");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;
    library.purge(held.zuleika)?;

    assert!(library.reclaim_purged(PHONE).is_err(), "nothing published");
    library.commit(PHONE)?;
    let said = library.reclaim_purged(WriterId::ANONYMOUS);
    assert!(said.unwrap_err().to_string().contains("named writer"));
    assert!(!traces(&dir.join("phone")).is_empty());
    assert_eq!(library.reclaim_purged(PHONE)?.dropped, 7);
    Ok(())
}

#[test]
fn pruning_drops_what_a_purge_outranked_without_waiting_for_anyone() -> Result<()> {
    let dir = TempDir::new("purge_prune");
    let phone_dir = dir.join("phone");
    let mut library = Library::create(&phone_dir, PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;
    library.books.set_year(held.seven, 1920)?;
    library.commit(PHONE)?;
    library.purge(held.zuleika)?;
    library.commit(PHONE)?;

    let report = library.prune(PHONE, &[TABLET])?;

    assert_eq!(report.waiting_for, [TABLET]);
    assert_eq!(report.dropped, 7, "paragraphs keep history and went too");
    assert!(traces(&phone_dir).is_empty());
    assert_eq!(Table::versions_of(&library.books, held.seven)?.len(), 2);
    library.verify()?;

    assert_eq!(library.prune(PHONE, &[])?.dropped, 1);
    assert_eq!(Table::versions_of(&library.books, held.seven)?.len(), 1);
    assert!(library.is_purged(held.zuleika)?);
    assert_eq!(
        Table::versions_of(&library.books, held.zuleika)?.len(),
        1,
        "the tombstone is never pruned"
    );
    assert_eq!(library.prune(PHONE, &[])?.dropped, 0);
    Ok(())
}

struct Three {
    phone: Library,
    phone_dir: PathBuf,
    desk: Library,
    desk_dir: PathBuf,
    tablet: Library,
    tablet_dir: PathBuf,
    held: Stocked,
}

// The phone wrote the library; the desk and the tablet have read it.
fn three(dir: &TempDir) -> Result<Three> {
    let (phone_dir, desk_dir, tablet_dir) =
        (dir.join("phone"), dir.join("desk"), dir.join("tablet"));
    let mut phone = Library::create(&phone_dir, PHONE)?;
    let mut desk = Library::create(&desk_dir, DESK)?;
    let mut tablet = Library::create(&tablet_dir, TABLET)?;
    let held = stock(&mut phone)?;
    phone.commit(PHONE)?;
    carry(&phone_dir, &desk_dir, PHONE);
    carry(&phone_dir, &tablet_dir, PHONE);
    desk.refresh_peers()?;
    tablet.refresh_peers()?;
    Ok(Three {
        phone,
        phone_dir,
        desk,
        desk_dir,
        tablet,
        tablet_dir,
        held,
    })
}

#[test]
fn a_writer_that_never_held_the_row_takes_the_purge_and_waits_for_nothing() -> Result<()> {
    let dir = TempDir::new("purge_never_held");
    let phone_dir = dir.join("phone");
    let mut phone = Library::create(&phone_dir, PHONE)?;
    let held = stock(&mut phone)?;
    phone.commit(PHONE)?;
    phone.purge(held.zuleika)?;
    phone.commit(PHONE)?;
    phone.reclaim_purged(PHONE)?;

    let tablet_dir = dir.join("tablet");
    let mut tablet = Library::create(&tablet_dir, TABLET)?;
    carry(&phone_dir, &tablet_dir, PHONE);
    let told = tablet.refresh_peers()?;

    assert_eq!(told.waiting, 0);
    assert_eq!(told.taken, 7 + 1 + 2 + 1 + 1 + 2);
    assert!(told.concurrent.is_empty() && told.combined.is_empty());
    assert!(tablet.is_purged(held.zuleika)?);
    assert!(tablet.is_purged(held.paragraphs[0])?);
    assert_eq!(tablet.status(held.zuleika)?, RowStatus::Deleted);
    assert_eq!(tablet.all::<Book>().count(), 1);
    assert_eq!(tablet.statuses()?, phone.statuses()?);
    assert_eq!(tablet.reclaim_purged(TABLET)?, PurgeReport::default());
    tablet.verify()?;
    Ok(())
}

#[test]
fn a_purge_delivered_without_the_rows_it_follows_is_taken_at_once() -> Result<()> {
    let dir = TempDir::new("purge_alone");
    let mut three = three(&dir)?;
    three.desk.purge(three.held.zuleika)?;
    three.desk.commit(DESK)?;

    let fresh_dir = dir.join("fresh");
    let mut fresh = Library::create(&fresh_dir, WriterId::from_raw(0x54))?;
    carry(&three.desk_dir, &fresh_dir, DESK);
    let told = fresh.refresh_peers()?;

    assert_eq!((told.taken, told.waiting), (7, 0));
    assert!(fresh.is_purged(three.held.zuleika)?);
    assert_eq!(fresh.history(three.held.zuleika)?.len(), 1);

    carry(&three.phone_dir, &fresh_dir, PHONE);
    let late = fresh.refresh_peers()?;
    assert_eq!(late.waiting, 0);
    assert!(late.concurrent.is_empty(), "{late:?}");
    assert!(
        late.visibility
            .iter()
            .all(|change| change.id != three.held.zuleika.uuid()),
        "what arrives late does not bring the row back"
    );
    assert!(fresh.is_purged(three.held.zuleika)?);
    assert_eq!(fresh.all::<Book>().count(), 1);
    assert_eq!(fresh.statuses()?, three.desk.statuses()?);
    fresh.verify()?;
    Ok(())
}

#[test]
fn another_writer_scrubs_the_shards_of_one_that_is_gone_for_good() -> Result<()> {
    let dir = TempDir::new("purge_scrub");
    let mut three = three(&dir)?;
    drop(three.phone);
    three.desk.purge(three.held.zuleika)?;
    three.desk.commit(DESK)?;
    three.desk.reclaim_purged(DESK)?;
    assert!(
        !traces(&three.desk_dir).is_empty(),
        "the phone's shards hold it"
    );

    assert_eq!(three.desk.scrub_purged(DESK, PHONE)?, 7);
    assert!(traces(&three.desk_dir).is_empty());
    assert_eq!(
        three.desk.scrub_purged(DESK, PHONE)?,
        0,
        "nothing is left to drop"
    );
    assert_eq!(three.desk.all::<Book>().count(), 1);
    assert_eq!(three.desk.books.title(three.held.seven)?, KEPT);
    three.desk.verify()?;
    assert!(three.desk.scrub_purged(DESK, DESK).is_err(), "not its own");

    // The tablet had read the phone's shards whole; now they arrive scrubbed.
    carry(&three.desk_dir, &three.tablet_dir, DESK);
    carry(&three.desk_dir, &three.tablet_dir, PHONE);
    three.tablet.refresh_peers()?;
    three.tablet.reclaim_purged(TABLET)?;
    assert!(traces(&three.tablet_dir).is_empty());
    assert_eq!(three.tablet.statuses()?, three.desk.statuses()?);
    three.tablet.verify()?;

    // And one that never saw them whole reads the same library.
    let fresh_dir = dir.join("fresh");
    let mut fresh = Library::create(&fresh_dir, WriterId::from_raw(0x54))?;
    carry(&three.desk_dir, &fresh_dir, DESK);
    carry(&three.desk_dir, &fresh_dir, PHONE);
    let told = fresh.refresh_peers()?;
    assert_eq!(told.waiting, 0);
    assert_eq!(fresh.statuses()?, three.desk.statuses()?);
    assert_eq!(fresh.books.title(three.held.seven)?, KEPT);
    fresh.verify()?;

    drop(three.desk);
    let reopened = Library::open(&three.desk_dir)?;
    assert_eq!(reopened.all::<Book>().count(), 1);
    reopened.verify()?;
    Ok(())
}

fn purged_by_the_desk_and_dropped_by_the_phone(three: &mut Three) -> Result<()> {
    three.desk.purge(three.held.zuleika)?;
    three.desk.commit(DESK)?;
    let own = crate::test_utils::shared_files(&three.desk_dir);
    assert_eq!(
        three.desk.reclaim_purged(DESK)?,
        PurgeReport {
            purged: 0,
            dropped: 0
        },
        "the desk wrote none of it"
    );
    assert_eq!(
        crate::test_utils::shared_files(&three.desk_dir),
        own,
        "and rewrote no shard, its own included"
    );
    assert!(three.desk.books.purged_payload()?.is_empty());
    assert_eq!(
        files_holding(&three.desk_dir, TEXT.as_bytes()),
        [PathBuf::from("paragraph/0000000000000051.v1.dbs")]
    );
    carry(&three.desk_dir, &three.phone_dir, DESK);
    let told = three.phone.refresh_peers()?;
    assert!(told.visibility.iter().any(|change| {
        change.id == three.held.zuleika.uuid() && change.after == RowStatus::Deleted
    }));
    let report = three.phone.reclaim_purged(PHONE)?;
    assert_eq!(
        report,
        PurgeReport {
            purged: 0,
            dropped: 7
        }
    );
    assert!(traces(&three.phone_dir).is_empty());
    Ok(())
}

#[test]
fn a_reader_given_the_rewritten_shard_and_then_the_purge_ends_up_without_the_row() -> Result<()> {
    let dir = TempDir::new("purge_shard_first");
    let mut three = three(&dir)?;
    purged_by_the_desk_and_dropped_by_the_phone(&mut three)?;

    carry(&three.phone_dir, &three.tablet_dir, PHONE);
    let early = three.tablet.refresh_peers()?;
    assert_eq!(early.waiting, 0);
    assert_eq!(
        three.tablet.at(three.held.zuleika)?.title()?,
        TITLE,
        "nobody has told the tablet of the purge"
    );
    assert_eq!(three.tablet.reclaim_purged(TABLET)?, PurgeReport::default());
    assert!(!traces(&three.tablet_dir).is_empty());

    carry(&three.desk_dir, &three.tablet_dir, DESK);
    let told = three.tablet.refresh_peers()?;
    assert_eq!((told.taken, told.waiting), (7, 0));
    assert!(three.tablet.is_purged(three.held.zuleika)?);
    assert_eq!(
        traces(&three.tablet_dir),
        [PathBuf::from("local/cache.v1.r2.dbs")]
    );

    assert_eq!(three.tablet.reclaim_purged(TABLET)?, PurgeReport::default());
    assert!(traces(&three.tablet_dir).is_empty());
    assert_eq!(three.tablet.statuses()?, three.desk.statuses()?);
    assert_eq!(three.tablet.at(three.held.seven)?.title()?, KEPT);
    three.tablet.verify()?;
    Ok(())
}

#[test]
fn a_reader_given_the_purge_and_then_the_rewritten_shard_ends_up_without_the_row() -> Result<()> {
    let dir = TempDir::new("purge_tombstone_first");
    let mut three = three(&dir)?;
    purged_by_the_desk_and_dropped_by_the_phone(&mut three)?;

    carry(&three.desk_dir, &three.tablet_dir, DESK);
    let told = three.tablet.refresh_peers()?;
    assert_eq!((told.taken, told.waiting), (7, 0));
    assert!(three.tablet.is_purged(three.held.zuleika)?);
    assert_eq!(three.tablet.reclaim_purged(TABLET)?, PurgeReport::default());
    let left = traces(&three.tablet_dir);
    assert!(!left.is_empty());
    assert!(
        left.iter()
            .all(|path| path.ends_with("0000000000000051.v1.dbs")),
        "only the phone can rewrite the phone's shards: {left:?}"
    );
    three.tablet.verify()?;

    carry(&three.phone_dir, &three.tablet_dir, PHONE);
    let late = three.tablet.refresh_peers()?;
    assert_eq!((late.taken, late.waiting), (0, 0));
    assert!(traces(&three.tablet_dir).is_empty());
    assert_eq!(three.tablet.statuses()?, three.desk.statuses()?);
    assert_eq!(three.tablet.all::<Chapter>().count(), 1);
    three.tablet.verify()?;

    std::fs::remove_dir_all(three.tablet_dir.join("local"))?;
    let rebuilt = Library::open(&three.tablet_dir)?;
    assert_eq!(rebuilt.statuses()?, three.desk.statuses()?);
    assert!(rebuilt.books.purged_payload()?.is_empty());
    Ok(())
}

#[test]
fn an_edit_made_without_knowing_of_the_purge_does_not_bring_the_row_back() -> Result<()> {
    const LATE: &str = "Zuleika, retitled far from home";
    let dir = TempDir::new("purge_late_edit");
    let mut three = three(&dir)?;
    three.tablet.books.set_title(three.held.zuleika, LATE)?;
    three.tablet.remove(three.held.chapters[0])?;
    three.tablet.commit(TABLET)?;
    purged_by_the_desk_and_dropped_by_the_phone(&mut three)?;

    carry(&three.tablet_dir, &three.phone_dir, TABLET);
    let told = three.phone.refresh_peers()?;

    assert_eq!((told.taken, told.waiting), (2, 0));
    assert!(told.is_quiet(), "{told:?}");
    assert!(told.visibility.is_empty(), "{told:?}");
    assert!(three.phone.is_purged(three.held.zuleika)?);
    assert!(three.phone.conflicts()?.iter().all(|conflict| matches!(
        conflict,
        crate::schema::Conflict::Reference(reason) if reason.id == three.held.pinned.uuid()
    )));
    refused(three.phone.books.set_title(three.held.zuleika, "again"));
    three.phone.verify()?;
    assert_eq!(three.phone.reclaim_purged(PHONE)?, PurgeReport::default());
    assert_eq!(
        files_holding(&three.phone_dir, LATE.as_bytes()),
        [PathBuf::from("book/0000000000000053.v1.dbs")]
    );
    assert_eq!(
        three.phone.clock().counter_of(TABLET),
        three.tablet.clock().counter_of(TABLET),
        "a revision passed over is still a revision seen"
    );
    three.phone.verify()?;

    carry(&three.desk_dir, &three.tablet_dir, DESK);
    carry(&three.phone_dir, &three.tablet_dir, PHONE);
    three.tablet.refresh_peers()?;
    assert!(three.tablet.is_purged(three.held.zuleika)?);
    let report = three.tablet.reclaim_purged(TABLET)?;
    assert_eq!(
        report,
        PurgeReport {
            purged: 0,
            dropped: 2
        }
    );
    assert!(traces(&three.tablet_dir).is_empty());
    assert!(files_holding(&three.tablet_dir, LATE.as_bytes()).is_empty());

    carry(&three.tablet_dir, &three.phone_dir, TABLET);
    three.phone.refresh_peers()?;
    assert!(files_holding(&three.phone_dir, LATE.as_bytes()).is_empty());
    assert_eq!(three.phone.statuses()?, three.tablet.statuses()?);
    assert_eq!(three.phone.statuses()?, three.desk.statuses()?);
    Ok(())
}

#[test]
fn a_row_added_under_a_purged_one_is_purged_by_whoever_reclaims_next() -> Result<()> {
    const LATE: &str = "a chapter written on the train";
    let dir = TempDir::new("purge_late_child");
    let mut three = three(&dir)?;
    let chapter = three.tablet.chapters.push(three.held.zuleika, 3, LATE)?;
    let paragraph = three
        .tablet
        .paragraphs
        .push(chapter, "its only paragraph")?;
    three.tablet.commit(TABLET)?;
    purged_by_the_desk_and_dropped_by_the_phone(&mut three)?;

    carry(&three.tablet_dir, &three.phone_dir, TABLET);
    three.phone.refresh_peers()?;
    assert!(matches!(
        three.phone.status(chapter)?,
        RowStatus::Suppressed(_)
    ));
    let report = three.phone.reclaim_purged(PHONE)?;
    assert_eq!(
        report,
        PurgeReport {
            purged: 2,
            dropped: 0
        }
    );
    assert!(three.phone.is_purged(chapter)? && three.phone.is_purged(paragraph)?);
    assert_eq!(three.phone.status(chapter)?, RowStatus::Deleted);
    assert_eq!(
        files_holding(&three.phone_dir, LATE.as_bytes()),
        [PathBuf::from("chapter/0000000000000053.v1.dbs")]
    );

    carry(&three.desk_dir, &three.tablet_dir, DESK);
    three.tablet.refresh_peers()?;
    let report = three.tablet.reclaim_purged(TABLET)?;
    assert_eq!(
        report,
        PurgeReport {
            purged: 2,
            dropped: 2
        },
        "the tablet had not heard that the phone purged them too"
    );
    assert!(files_holding(&three.tablet_dir, LATE.as_bytes()).is_empty());

    carry(&three.tablet_dir, &three.phone_dir, TABLET);
    carry(&three.phone_dir, &three.tablet_dir, PHONE);
    let heard = three.phone.refresh_peers()?;
    assert!(heard.is_quiet(), "two purges of one row agree: {heard:?}");
    three.tablet.refresh_peers()?;
    assert!(files_holding(&three.phone_dir, LATE.as_bytes()).is_empty());
    assert_eq!(three.phone.statuses()?, three.tablet.statuses()?);
    assert_eq!(Table::versions_of(&three.phone.chapters, chapter)?.len(), 2);
    assert_eq!(three.phone.reclaim_purged(PHONE)?, PurgeReport::default());
    assert_eq!(three.tablet.reclaim_purged(TABLET)?, PurgeReport::default());
    three.phone.verify()?;
    three.tablet.verify()?;
    Ok(())
}

#[test]
fn two_copies_merged_either_way_let_a_purge_beat_what_the_other_wrote() -> Result<()> {
    let dir = TempDir::new("purge_merge");
    let mut three = three(&dir)?;
    let written = three.desk.at(three.held.zuleika)?.version()?.dot();
    three.desk.books.set_title(three.held.zuleika, "edited")?;
    three.desk.commit(DESK)?;
    three.tablet.remove(three.held.zuleika)?;
    three.tablet.commit(TABLET)?;
    three.tablet.restore(three.held.zuleika, written)?;
    three.tablet.commit(TABLET)?;
    three.phone.purge(three.held.zuleika)?;
    three.phone.commit(PHONE)?;

    let mut one = three.phone.clone();
    let told = one.merge_snapshot(&three.desk)?;
    assert!(told.is_quiet() && told.visibility.is_empty(), "{told:?}");
    one.merge_snapshot(&three.tablet)?;
    let mut other = three.tablet.clone();
    other.merge_snapshot(&three.desk)?;
    assert_eq!(other.books.title(three.held.zuleika)?, TITLE);
    let told = other.merge_snapshot(&three.phone)?;
    assert!(told.concurrent.is_empty() && told.combined.is_empty());
    assert_eq!(told.references.len(), 1, "the note that named the book");

    for merged in [&one, &other] {
        assert!(merged.is_purged(three.held.zuleika)?);
        assert_eq!(merged.status(three.held.zuleika)?, RowStatus::Deleted);
        assert_eq!(
            Table::versions_of(&merged.books, three.held.zuleika)?.len(),
            5
        );
        assert!(merged.books.conflicts()?.is_empty());
        assert!(merged.books.eligible_heads(three.held.zuleika)?.is_empty());
        merged.audit()?;
    }
    assert_eq!(one.statuses()?, other.statuses()?);
    assert!(!one.merge_snapshot(&other)?.changed_anything());
    Ok(())
}

#[test]
fn two_purges_of_one_row_are_both_kept_and_the_later_speaks_for_it() -> Result<()> {
    let dir = TempDir::new("purge_twice");
    let mut three = three(&dir)?;
    three.desk.books.purge(three.held.seven)?;
    three.desk.commit(DESK)?;
    three.tablet.books.purge(three.held.seven)?;
    three.tablet.commit(TABLET)?;

    let mut one = three.desk.clone();
    one.merge_snapshot(&three.tablet)?;
    let mut other = three.tablet.clone();
    other.merge_snapshot(&three.desk)?;

    let later = Table::versions_of(&three.tablet.books, three.held.seven)?[1];
    let later = three.tablet.books.versions().raw_at(later)?.dot();
    for merged in [&one, &other] {
        let rows = Table::versions_of(&merged.books, three.held.seven)?;
        assert_eq!(rows.len(), 3);
        let winner = merged.books.winner(three.held.seven)?.unwrap();
        assert_eq!(merged.books.versions().raw_at(winner)?.dot(), later);
        assert_eq!(merged.books.purge_of(three.held.seven)?, Some(winner));
        assert!(
            merged
                .conflicts()?
                .iter()
                .all(|conflict| matches!(conflict, crate::schema::Conflict::Reference(_)))
        );
        merged.audit()?;
    }
    Ok(())
}

#[test]
fn a_purge_marked_as_anything_but_a_removal_is_damage() -> Result<()> {
    use crate::record::Record;
    use crate::schema::{Dot, Version};

    let dir = TempDir::new("purge_flags");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;
    let mut raw = vec![0u8; <Version as Record>::SIZE];
    Version::of(Dot::new(DESK, u64::MAX - 1)).encode(&mut raw);
    raw[<Dot as Record>::SIZE] = 1 << 4;
    let version = Version::decode(&raw);
    assert!(version.is_purged() && !version.is_removed());

    library.books.adopt_purge(held.seven, version, Vec::new())?;

    let said = library.books.check_versions().unwrap_err();
    assert!(said.to_string().contains("operation flags"), "{said}");
    Ok(())
}

#[test]
fn a_purge_already_held_is_known_and_one_that_differs_is_damage() -> Result<()> {
    use crate::schema::{Adoption, Parent, ShardRow};

    let dir = TempDir::new("purge_known");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;
    library.purge(held.seven)?;
    library.commit(PHONE)?;
    let rows = AnyTable::export_rows(&library.books)?;
    let told = rows.last().unwrap().clone();
    assert!(told.version.is_purged());

    let mut bare = ShardRow {
        values: vec![("title".to_owned(), b"not even a value".to_vec())],
        ..told.clone()
    };
    assert_eq!(library.books.adopt_row(&bare, &|_| false)?, Adoption::Known);

    bare.id = held.zuleika.uuid();
    assert!(library.books.adopt_row(&bare, &|_| false).is_err());
    let parents: Vec<Parent> = Vec::new();
    assert!(
        library
            .books
            .adopt_purge(held.seven, told.version, parents)
            .is_err(),
        "the same dot with other parents"
    );
    let other = crate::schema::Version::of(told.version.dot()).purge();
    assert_eq!(other, told.version);
    assert!(
        library
            .books
            .adopt_purge(
                held.seven,
                crate::schema::Version::of(told.version.dot()),
                told.parents
                    .iter()
                    .copied()
                    .map(Parent::Published)
                    .collect()
            )
            .is_err(),
        "the same dot marked otherwise"
    );
    Ok(())
}

#[test]
fn a_purge_from_a_shard_is_taken_whatever_values_it_claims() -> Result<()> {
    use crate::schema::{Adoption, Dot, ShardRow, Version};

    let dir = TempDir::new("purge_claims");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;
    let told = ShardRow {
        id: held.seven.uuid(),
        version: Version::of(Dot::new(DESK, u64::MAX - 1)).purge(),
        parents: vec![Dot::new(TABLET, 9)],
        values: vec![("binding".to_owned(), vec![200])],
    };

    assert_eq!(library.books.adopt_row(&told, &|_| false)?, Adoption::Taken);
    Table::refresh_one(&library.books, held.seven)?;

    assert!(library.is_purged(held.seven)?);
    let left = library.history(held.seven)?;
    assert_eq!(left.last().unwrap().binding()?, Binding::Cloth);
    library.books.check_versions()?;
    Ok(())
}

#[test]
fn a_cache_rebuilt_while_shedding_takes_no_row_a_purge_outranked() -> Result<()> {
    let dir = TempDir::new("purge_shed");
    let mut three = three(&dir)?;
    three.desk.purge(three.held.zuleika)?;
    three.desk.commit(DESK)?;
    carry(&three.desk_dir, &three.tablet_dir, DESK);
    drop(three.tablet);
    std::fs::remove_dir_all(three.tablet_dir.join("local"))?;

    let plain = Library::open(&three.tablet_dir)?;
    assert_eq!(plain.books.purged_payload()?.len(), 1);
    assert_eq!(plain.paragraphs.purged_payload()?.len(), 3);
    assert!(
        files_holding(&three.tablet_dir, TEXT.as_bytes())
            .contains(&PathBuf::from("local/cache.v1.r2.dbs"))
    );
    drop(plain);

    std::fs::remove_file(cache(&three.tablet_dir))?;
    let mut shed = Library::with_fresh_cache(crate::schema::Layout::within(&three.tablet_dir))?;
    shed.state_mut().shedding = true;
    let mut report = crate::schema::MergeReport::default();
    let batches = shed.collect(|_| true, &mut report)?;
    shed.apply(batches, &mut report)?;

    assert_eq!(report.taken, 7 + 7);
    assert_eq!(report.known, 7, "shed rows are passed over, not kept");
    assert!(shed.books.purged_payload()?.is_empty());
    assert!(shed.is_purged(three.held.paragraphs[0])?);
    assert_eq!(shed.at(three.held.seven)?.title()?, KEPT);
    assert_eq!(shed.clock(), three.desk.clock());
    Ok(())
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

fn carry_pages(from: &Path, to: &Path, writer: WriterId) {
    carry_some(from, to, writer, &["page"]);
}

#[test]
fn a_purge_is_taken_from_a_shard_that_is_stalled_ahead_of_it() -> Result<()> {
    let dir = TempDir::new("purge_stalled");
    let (phone_dir, desk_dir, tablet_dir) =
        (dir.join("phone"), dir.join("desk"), dir.join("tablet"));
    let mut phone = newer::Journal::create(&phone_dir, PHONE)?;
    let mut desk = older::Journal::create(&desk_dir, DESK)?;
    let mut tablet = newer::Journal::create(&tablet_dir, TABLET)?;
    let page = phone.pages.push("written on the phone", Some(4))?;
    phone.commit(PHONE)?;
    carry_pages(&phone_dir, &desk_dir, PHONE);
    desk.refresh_peers()?;
    desk.pages.set_title(page.cast(), "retitled at the desk")?;
    desk.commit(DESK)?;
    let scrap = desk.pages.push("a scrap")?;
    desk.commit(DESK)?;
    desk.purge(scrap)?;
    desk.commit(DESK)?;

    carry_pages(&desk_dir, &tablet_dir, DESK);
    let told = tablet.refresh_peers()?;

    assert_eq!(told.taken, 1, "only the purge: {told:?}");
    assert_eq!(told.waiting, 2, "the retitling and what follows it");
    assert!(tablet.is_purged(scrap.cast::<newer::Page>())?);
    assert!(tablet.status(page).is_err());

    carry_pages(&phone_dir, &tablet_dir, PHONE);
    let late = tablet.refresh_peers()?;
    assert_eq!((late.taken, late.known, late.waiting), (3, 1, 0));
    assert_eq!(tablet.pages.title(page)?, "retitled at the desk");
    assert_eq!(tablet.pages.mark(page)?, Some(4));
    assert!(tablet.is_purged(scrap.cast::<newer::Page>())?);
    tablet.verify()?;
    Ok(())
}

#[test]
fn a_revision_of_a_purged_row_is_not_kept_waiting_for_what_it_followed() -> Result<()> {
    let dir = TempDir::new("purge_unblocked");
    let (phone_dir, desk_dir, tablet_dir) =
        (dir.join("phone"), dir.join("desk"), dir.join("tablet"));
    let laptop_dir = dir.join("laptop");
    let laptop_id = WriterId::from_raw(0x54);
    let mut phone = newer::Journal::create(&phone_dir, PHONE)?;
    let mut desk = older::Journal::create(&desk_dir, DESK)?;
    let mut laptop = newer::Journal::create(&laptop_dir, laptop_id)?;
    let page = phone.pages.push("written on the phone", Some(4))?;
    phone.commit(PHONE)?;
    carry_pages(&phone_dir, &desk_dir, PHONE);
    desk.refresh_peers()?;
    desk.pages.set_title(page.cast(), "retitled at the desk")?;
    let other = desk.pages.push("another page")?;
    desk.commit(DESK)?;
    carry_pages(&phone_dir, &laptop_dir, PHONE);
    carry_pages(&desk_dir, &laptop_dir, DESK);
    laptop.refresh_peers()?;
    laptop.purge(page)?;
    laptop.commit(laptop_id)?;

    let mut waits = newer::Journal::create(dir.join("waits"), TABLET)?;
    carry_pages(&desk_dir, &dir.join("waits"), DESK);
    assert_eq!(waits.refresh_peers()?.waiting, 2, "the phone is offline");

    let mut tablet = newer::Journal::create(&tablet_dir, TABLET)?;
    carry_pages(&desk_dir, &tablet_dir, DESK);
    carry_pages(&laptop_dir, &tablet_dir, laptop_id);
    let told = tablet.refresh_peers()?;

    assert_eq!((told.taken, told.waiting), (3, 0), "{told:?}");
    assert!(tablet.is_purged(page)?);
    assert_eq!(tablet.pages.title(other.cast())?, "another page");
    assert_eq!(Table::versions_of(&tablet.pages, page)?.len(), 2);
    tablet.verify()?;
    assert_eq!(tablet.reclaim_purged(TABLET)?, PurgeReport::default());
    assert_eq!(
        Table::versions_of(&tablet.pages, page)?.len(),
        1,
        "the cache gave up the desk's revision; only the desk can drop it from its shard"
    );
    Ok(())
}

#[test]
fn a_reclaim_that_cannot_publish_what_it_purged_leaves_the_purge_for_the_next_commit() -> Result<()>
{
    use crate::storage::column_file::{CommitFault, fail_commit_at};

    let dir = TempDir::new("purge_fault");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;
    library.books.purge(held.zuleika)?;
    library.commit(PHONE)?;

    fail_commit_at(CommitFault::Payload);
    assert!(library.reclaim_purged(PHONE).is_err());

    assert!(library.is_purged(held.chapters[0])?);
    let said = library.reclaim_purged(PHONE).unwrap_err();
    assert!(
        said.to_string().contains("commit pending changes"),
        "{said}"
    );
    library.commit(PHONE)?;
    assert_eq!(
        library.reclaim_purged(PHONE)?,
        PurgeReport {
            purged: 0,
            dropped: 7
        }
    );
    assert!(traces(&dir.join("phone")).is_empty());
    library.verify()?;
    Ok(())
}

#[test]
fn what_a_build_that_predates_purging_wrote_over_a_purge_does_not_unseat_it() -> Result<()> {
    use crate::schema::{Adoption, Dot, ShardRow, Version};

    let dir = TempDir::new("purge_older");
    let mut library = Library::create(dir.join("phone"), PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;
    library.books.purge(held.seven)?;
    library.commit(PHONE)?;
    let rows = AnyTable::export_rows(&library.books)?;
    let (written, purge) = (&rows[1], rows.last().unwrap());
    assert!(purge.version.is_purged() && written.id == purge.id);
    let restored = ShardRow {
        version: Version::of(Dot::new(DESK, u64::MAX - 1)).restoration(),
        parents: vec![purge.version.dot()],
        ..written.clone()
    };

    assert_eq!(
        library.books.adopt_row(&restored, &|_| false)?,
        Adoption::Taken
    );
    Table::refresh_one(&library.books, held.seven)?;

    assert!(library.is_purged(held.seven)?);
    assert_eq!(library.status(held.seven)?, RowStatus::Deleted);
    library.books.check_versions()?;
    assert_eq!(library.books.heads(held.seven)?.len(), 1);
    let pruned = library.books.prunable(PHONE, &|_| true, &|_| true)?;
    assert!(pruned.is_empty(), "the purge is not a revision to prune");
    assert_eq!(library.books.purged_payload()?.len(), 2);
    Ok(())
}

#[test]
fn a_purge_that_fails_part_way_down_leaves_every_row_as_it_was() -> Result<()> {
    let dir = TempDir::new("purge_rollback");
    let phone_dir = dir.join("phone");
    let mut library = Library::create(&phone_dir, PHONE)?;
    let held = stock(&mut library)?;
    library.commit(PHONE)?;
    drop(library);

    let mut library = Library::open(&phone_dir)?;
    assert!(library.clone().books.purge(held.zuleika)?);
    assert_eq!(library.at(held.zuleika)?.title()?, TITLE);
    let before = library.statuses()?;
    let file = std::fs::File::options()
        .write(true)
        .open(cache(&phone_dir))?;
    file.set_len(crate::test_utils::column_file_bytes::HEAD_LEN)?;
    assert!(
        library.clone().books.purge(held.zuleika)?,
        "the book is in reach without the file"
    );
    let rows = library.books.rows();

    assert!(library.purge(held.zuleika).is_err(), "its chapters are not");

    assert!(!library.is_purged(held.zuleika)?);
    assert_eq!(library.books.rows(), rows);
    assert_eq!(library.statuses()?, before);
    assert_eq!(library.at(held.zuleika)?.title()?, TITLE);
    Ok(())
}

#[test]
fn a_revision_written_after_a_purge_it_never_saw_is_no_head_of_the_row() -> Result<()> {
    let dir = TempDir::new("purge_later");
    let mut three = three(&dir)?;
    purged_by_the_desk_and_dropped_by_the_phone(&mut three)?;
    three.tablet.books.set_title(three.held.zuleika, "later")?;
    three.tablet.commit(TABLET)?;

    carry(&three.tablet_dir, &three.phone_dir, TABLET);
    let told = three.phone.refresh_peers()?;

    assert_eq!((told.taken, told.waiting), (1, 0));
    let books = &three.phone.books;
    let rows = Table::versions_of(books, three.held.zuleika)?;
    let [purge, later] = rows.as_slice() else {
        panic!("the purge and what the tablet wrote: {rows:?}");
    };
    assert!(books.versions().raw_at(*purge)?.dot() < books.versions().raw_at(*later)?.dot());
    assert!(books.versions().lacks_a_parent(*later)?);
    assert!(books.eligible_heads(three.held.zuleika)?.is_empty());
    assert_eq!(books.winner(three.held.zuleika)?, Some(*purge));
    assert!(told.is_quiet() && told.visibility.is_empty(), "{told:?}");
    three.phone.verify()?;
    Ok(())
}
