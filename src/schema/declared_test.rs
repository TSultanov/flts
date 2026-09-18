use crate::error::Result;
use crate::id::Id;
use crate::schema::{Store, Table};
use crate::storage::{ColumnFile, WriterId};
use crate::test_utils::TempDir;
use crate::test_utils::props;
use proptest::prelude::*;

#[crate::entity]
struct Book {
    #[index]
    title: String,
    author: String,
    chapters: Many<Chapter>,
    credits: Many<Credit>,
    #[through(Credit)]
    editors: Many<Editor>,
}

#[crate::entity]
struct Editor {
    #[index]
    name: String,
    #[through(Credit)]
    books: Many<Book>,
}

#[crate::entity]
#[distinct(book, editor)]
struct Credit {
    book: One<Book>,
    editor: One<Editor>,
}

#[crate::entity]
struct Chapter {
    book: One<Book>,
    title: String,
    paragraphs: Many<Paragraph>,
}

#[crate::entity]
struct Paragraph {
    chapter: One<Chapter>,
    text: String,
}

#[crate::container]
struct Library {
    books: Set<Book>,
    chapters: Set<Chapter>,
    paragraphs: Set<Paragraph>,
    editors: Set<Editor>,
    credits: Set<Credit>,
}

fn this_device() -> WriterId {
    WriterId::from_device_id("KRZH3XY-7PH4NJK-2WQ5LMC")
}

struct Shelved {
    alice: Id<Book>,
    kokoro: Id<Book>,
    rabbit: Id<Chapter>,
    sensei: Id<Chapter>,
}

fn stock(library: &mut Library) -> Result<Shelved> {
    let alice = library.books.push("Alice", "Lewis Carroll")?;
    let kokoro = library.books.push("Kokoro", "Natsume Soseki")?;

    let rabbit = library.chapters.push(alice, "Down the Rabbit-Hole")?;
    let pool = library.chapters.push(alice, "The Pool of Tears")?;
    let sensei = library.chapters.push(kokoro, "Sensei and I")?;

    library
        .paragraphs
        .push(rabbit, "Alice was beginning to get very tired.")?;
    library
        .paragraphs
        .push(sensei, "I always called him Sensei.")?;
    library
        .paragraphs
        .push(rabbit, "So she was considering in her own mind.")?;
    library.paragraphs.push(pool, "Curiouser and curiouser!")?;
    Ok(Shelved {
        alice,
        kokoro,
        rabbit,
        sensei,
    })
}

#[test]
fn the_declared_schema_names_every_column_the_file_will_hold() {
    let names: Vec<String> = Library::default()
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect();

    assert_eq!(
        names,
        [
            "book.id",
            "book.id.buckets",
            "book.id.links",
            "book.title",
            "book.title.bytes",
            "book.title.buckets",
            "book.title.links",
            "book.author",
            "book.author.bytes",
            "chapter.id",
            "chapter.id.buckets",
            "chapter.id.links",
            "chapter.book",
            "chapter.book.buckets",
            "chapter.book.links",
            "chapter.title",
            "chapter.title.bytes",
            "paragraph.id",
            "paragraph.id.buckets",
            "paragraph.id.links",
            "paragraph.chapter",
            "paragraph.chapter.buckets",
            "paragraph.chapter.links",
            "paragraph.text",
            "paragraph.text.bytes",
            "editor.id",
            "editor.id.buckets",
            "editor.id.links",
            "editor.name",
            "editor.name.bytes",
            "editor.name.buckets",
            "editor.name.links",
            "credit.id",
            "credit.id.buckets",
            "credit.id.links",
            "credit.book",
            "credit.book.buckets",
            "credit.book.links",
            "credit.editor",
            "credit.editor.buckets",
            "credit.editor.links",
            "credit.book_editor",
            "credit.book_editor.buckets",
            "credit.book_editor.links",
            "dbsync.clock",
        ]
    );
}

#[test]
fn a_declared_library_joins_two_levels_down() -> Result<()> {
    let mut library = Library::default();
    let on = stock(&mut library)?;
    let alice = on.alice;

    let chapters: Vec<Id<Chapter>> = library.children_of(alice)?;
    assert_eq!(chapters.len(), 2);
    assert_eq!(library.chapters.title(chapters[0])?, "Down the Rabbit-Hole");

    let paragraphs: Vec<Id<Paragraph>> = library.children_of(chapters[0])?;
    assert_eq!(paragraphs.len(), 2);
    assert_eq!(
        library.paragraphs.text(paragraphs[1])?,
        "So she was considering in her own mind."
    );
    Ok(())
}

#[test]
fn a_declared_library_round_trips_through_a_file_it_owns() -> Result<()> {
    let dir = TempDir::new("declared");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    let on = stock(&mut library)?;
    let alice = on.alice;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;
    reopened.verify()?;

    assert_eq!(Table::rows(&reopened.books), 2);
    assert_eq!(reopened.books.title(alice)?, "Alice");
    assert_eq!(reopened.books.author(alice)?, "Lewis Carroll");

    let chapters: Vec<Id<Chapter>> = reopened.children_of(alice)?;
    assert_eq!(chapters.len(), 2);
    let paragraphs: Vec<Id<Paragraph>> = reopened.children_of(chapters[1])?;
    assert_eq!(
        reopened.paragraphs.text(paragraphs[0])?,
        "Curiouser and curiouser!"
    );
    Ok(())
}

#[test]
fn a_declared_library_appends_only_what_changed() -> Result<()> {
    let dir = TempDir::new("declared_append");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    let _ = stock(&mut library)?;
    library.commit(this_device())?;

    let settled = crate::storage::ColumnFile::open(&path)?;
    let untouched = settled.column_meta("paragraph.text").unwrap().chunks[0];
    drop(settled);

    library.books.push("Il nome della rosa", "Umberto Eco")?;
    library.commit(this_device())?;

    let grown = crate::storage::ColumnFile::open(&path)?;
    assert_eq!(
        grown.column_meta("paragraph.text").unwrap().chunks[0],
        untouched,
        "a table nobody touched must not be rewritten"
    );
    assert_eq!(Table::rows(&library.books), 3);
    Ok(())
}

#[test]
fn a_snapshot_of_a_declared_library_keeps_what_it_saw() -> Result<()> {
    let dir = TempDir::new("declared_snapshot");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    let _ = stock(&mut library)?;
    library.commit(this_device())?;

    let pinned = library.snapshot();
    library.books.push("Added later", "Nobody")?;
    library.commit(this_device())?;

    assert_eq!(Table::rows(&pinned.books), 2);
    assert_eq!(Table::rows(&library.books), 3);
    Ok(())
}

#[test]
fn an_edited_title_survives_a_commit_and_leaves_its_neighbours_alone() -> Result<()> {
    let dir = TempDir::new("declared_edit");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    let on = stock(&mut library)?;
    let alice = on.alice;
    library.commit(this_device())?;

    library.books.set_title(alice, "Alice in Wonderland")?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;
    assert_eq!(reopened.books.title(alice)?, "Alice in Wonderland");
    assert_eq!(reopened.books.author(alice)?, "Lewis Carroll");
    assert_eq!(
        reopened.books.title(on.kokoro)?,
        "Kokoro",
        "the row beside the edited one is untouched"
    );
    Ok(())
}

#[test]
fn opening_a_declared_library_reads_no_payload_at_all() -> Result<()> {
    let dir = TempDir::new("declared_lazy");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    let _ = stock(&mut library)?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;
    for column in reopened.residency() {
        assert_eq!(
            column.resident, 0,
            "{} was read before anyone asked for a row",
            column.name
        );
    }
    Ok(())
}

#[test]
fn listing_the_catalogue_leaves_every_paragraph_cold() -> Result<()> {
    let dir = TempDir::new("declared_cold");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    let _ = stock(&mut library)?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;
    for book in reopened.all::<Book>() {
        book.title()?;
        book.author()?;
    }

    let cold = |name: &str| {
        reopened
            .residency()
            .into_iter()
            .find(|column| column.name == name)
            .expect("the schema names this column")
    };

    assert!(cold("book.title").resident > 0);
    assert!(cold("book.title.bytes").resident > 0);
    assert_eq!(
        cold("paragraph.text").resident,
        0,
        "reading the catalogue must not read the books"
    );
    assert_eq!(
        cold("paragraph.text.bytes").resident,
        0,
        "a per-field heap is what keeps a title from faulting in a paragraph"
    );
    Ok(())
}

#[test]
fn walking_a_books_chapters_reads_the_index_and_never_the_column_it_indexes() -> Result<()> {
    let dir = TempDir::new("declared_walk");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    let on = stock(&mut library)?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;
    let walked = reopened.at(on.alice)?.chapters()?.count();

    let cold = |name: &str| {
        reopened
            .residency()
            .into_iter()
            .find(|column| column.name == name)
            .expect("the schema names this column")
    };

    assert_eq!(walked, 2);
    assert!(cold("chapter.book.buckets").resident > 0);
    assert_eq!(
        cold("chapter.book").resident,
        0,
        "a link carries the parent it files a child under, so the walk never reads the column"
    );
    assert_eq!(
        cold("chapter.title").resident,
        0,
        "walking to a chapter is not reading it"
    );
    Ok(())
}

#[test]
fn a_declared_row_answers_with_its_own_fields() -> Result<()> {
    let mut library = Library::default();
    let on = stock(&mut library)?;
    let alice = on.alice;

    let book = library.at(alice)?;

    assert_eq!(book.title()?, "Alice");
    assert_eq!(book.author()?, "Lewis Carroll");
    assert_eq!(book.id()?, alice);
    Ok(())
}

#[test]
fn every_declared_row_of_a_table_can_be_walked_in_turn() -> Result<()> {
    let mut library = Library::default();
    let _ = stock(&mut library)?;

    let titles: Vec<String> = library
        .all::<Chapter>()
        .map(|chapter| chapter.title().map(|title| title.into_owned()))
        .collect::<Result<_>>()?;

    assert_eq!(
        titles,
        ["Down the Rabbit-Hole", "The Pool of Tears", "Sensei and I"]
    );
    Ok(())
}

#[test]
fn a_declared_row_read_from_a_file_faults_in_only_the_column_it_was_asked_for() -> Result<()> {
    let dir = TempDir::new("declared_row_cold");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    let _ = stock(&mut library)?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;
    for book in reopened.all::<Book>() {
        book.title()?;
    }

    let resident = |name: &str| {
        reopened
            .residency()
            .into_iter()
            .find(|column| column.name == name)
            .expect("the schema names this column")
            .resident
    };

    assert!(resident("book.title.bytes") > 0);
    assert_eq!(resident("book.author.bytes"), 0);
    assert_eq!(resident("paragraph.text.bytes"), 0);
    Ok(())
}

#[test]
fn a_declared_row_walks_down_to_its_children_and_back_up_to_its_parent() -> Result<()> {
    let mut library = Library::default();
    let on = stock(&mut library)?;
    let alice = on.alice;

    let book = library.at(alice)?;
    let mut read = Vec::new();
    for chapter in book.chapters()? {
        read.push(chapter.title()?.into_owned());
        for paragraph in chapter.paragraphs()? {
            read.push(paragraph.text()?.into_owned());
            assert_eq!(paragraph.chapter()?.id()?, chapter.id()?);
            assert_eq!(paragraph.chapter()?.book()?.title()?, "Alice");
        }
    }

    assert_eq!(
        read,
        [
            "Down the Rabbit-Hole",
            "Alice was beginning to get very tired.",
            "So she was considering in her own mind.",
            "The Pool of Tears",
            "Curiouser and curiouser!",
        ]
    );
    Ok(())
}

#[test]
fn a_declared_row_with_no_children_walks_nowhere() -> Result<()> {
    let mut library = Library::default();
    let on = stock(&mut library)?;

    let sensei = library.at(on.sensei)?;
    assert_eq!(sensei.paragraphs()?.count(), 1);

    let quiet = library.books.push("Unread", "Nobody")?;
    assert_eq!(library.at(quiet)?.chapters()?.count(), 0);
    Ok(())
}

#[test]
fn walking_a_declared_book_reads_no_chapter_a_different_book_owns() -> Result<()> {
    let dir = TempDir::new("declared_walk_cold");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    let on = stock(&mut library)?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;
    let kokoro = reopened.at(on.kokoro)?;
    let titles: Vec<String> = kokoro
        .chapters()?
        .map(|chapter| chapter.title().map(|title| title.into_owned()))
        .collect::<Result<_>>()?;

    assert_eq!(titles, ["Sensei and I"]);
    assert_eq!(
        reopened
            .residency()
            .into_iter()
            .find(|column| column.name == "paragraph.text.bytes")
            .expect("the schema names this column")
            .resident,
        0,
        "listing a book's chapters must not read a single paragraph"
    );
    Ok(())
}

#[test]
fn a_title_edited_through_a_row_survives_the_commit_that_follows_it() -> Result<()> {
    let dir = TempDir::new("declared_row_edit");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    let on = stock(&mut library)?;
    let alice = on.alice;
    library.commit(this_device())?;

    library.at_mut(alice)?.set_title("Alice in Wonderland")?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;
    assert_eq!(reopened.at(alice)?.title()?, "Alice in Wonderland");
    assert_eq!(reopened.at(alice)?.author()?, "Lewis Carroll");
    assert_eq!(reopened.at(on.kokoro)?.title()?, "Kokoro");
    Ok(())
}

#[test]
fn a_chapter_moved_through_a_row_is_grouped_under_the_book_it_moved_to() -> Result<()> {
    let mut library = Library::default();
    let on = stock(&mut library)?;
    let alice = on.alice;
    let kokoro = on.kokoro;
    assert_eq!(library.at(alice)?.chapters()?.count(), 2);

    library.at_mut(on.rabbit)?.set_book(kokoro)?;

    assert_eq!(library.at(alice)?.chapters()?.count(), 1);
    let moved: Vec<String> = library
        .at(kokoro)?
        .chapters()?
        .map(|chapter| chapter.title().map(|title| title.into_owned()))
        .collect::<Result<_>>()?;
    assert_eq!(moved, ["Down the Rabbit-Hole", "Sensei and I"]);
    Ok(())
}

#[test]
fn a_library_lives_its_whole_life_through_the_rows_it_hands_out() -> Result<()> {
    let dir = TempDir::new("declared_life");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    let on = stock(&mut library)?;
    let alice = on.alice;
    let written_down = alice.to_string();
    let shelved = library.commit(this_device())?;

    let opened = Library::open(&path)?;
    assert!(
        opened.residency().iter().all(|held| held.resident == 0),
        "opening a library must read no payload"
    );

    let book = opened.at(alice)?;
    let mut read = Vec::new();
    for chapter in book.chapters()? {
        assert_eq!(chapter.book()?.id()?, book.id()?);
        for paragraph in chapter.paragraphs()? {
            assert_eq!(paragraph.chapter()?.id()?, chapter.id()?);
            read.push(paragraph.text()?.into_owned());
        }
    }
    assert_eq!(
        read,
        [
            "Alice was beginning to get very tired.",
            "So she was considering in her own mind.",
            "Curiouser and curiouser!",
        ]
    );

    library.at_mut(alice)?.set_title("Alice in Wonderland")?;
    library.books.push("Il nome della rosa", "Umberto Eco")?;
    let published = library.commit(this_device())?;
    assert!(published > shelved);

    let reopened = Library::open(&path)?;
    assert_eq!(reopened.at(alice)?.title()?, "Alice in Wonderland");
    assert_eq!(reopened.at(alice)?.author()?, "Lewis Carroll");
    assert_eq!(reopened.all::<Book>().count(), 3);
    assert_eq!(reopened.at(alice)?.chapters()?.count(), 2);
    assert_eq!(
        reopened.at(alice)?.id()?,
        alice,
        "two commits and two reopens later, the row still answers to the name it was minted with"
    );

    let carried: Id<Book> = written_down.parse()?;
    assert_eq!(carried, alice);
    assert_eq!(reopened.at(carried)?.title()?, "Alice in Wonderland");
    assert_eq!(
        reopened
            .at(alice)?
            .chapters()?
            .next()
            .unwrap()
            .book()?
            .id()?,
        alice,
        "and the chapters written before the edit still point at it"
    );

    let mut stale = opened;
    stale.books.push("Never committed", "Nobody")?;
    let refused = stale
        .commit(this_device())
        .expect_err("a handle opened at the older generation cannot commit over the newer one");
    assert!(
        refused.to_string().contains("moved to generation"),
        "{refused}"
    );

    let token = crate::storage::ColumnFile::read_commit_token(&path)?;
    assert_eq!(token.generation, published);
    assert!(!token.is_echo_of(this_device(), shelved));
    Ok(())
}

#[derive(Debug, Clone)]
enum Deed {
    Book(String, String),
    Chapter(usize, String),
    Paragraph(usize, String),
    Retitle(usize, String),
    Rebook(usize, usize),
    Settle,
}

fn deed() -> impl Strategy<Value = Deed> {
    prop_oneof![
        5 => (props::text(), props::text()).prop_map(|(title, author)| Deed::Book(title, author)),
        4 => (props::row_index(), props::text()).prop_map(|(at, title)| Deed::Chapter(at, title)),
        4 => (props::row_index(), props::text()).prop_map(|(at, text)| Deed::Paragraph(at, text)),
        2 => (props::row_index(), props::text()).prop_map(|(at, title)| Deed::Retitle(at, title)),
        2 => (props::row_index(), props::row_index()).prop_map(|(at, to)| Deed::Rebook(at, to)),
        1 => Just(Deed::Settle),
    ]
}

fn deeds() -> impl Strategy<Value = Vec<Deed>> {
    prop::collection::vec(deed(), 0..12)
}

type Shelves = (
    Vec<(String, String)>,
    Vec<(usize, String)>,
    Vec<(usize, String)>,
);

fn expected(deeds: &[Deed]) -> Shelves {
    let mut books: Vec<(String, String)> = Vec::new();
    let mut chapters: Vec<(usize, String)> = Vec::new();
    let mut paragraphs: Vec<(usize, String)> = Vec::new();

    for deed in deeds {
        match deed {
            Deed::Book(title, author) => books.push((title.clone(), author.clone())),
            Deed::Chapter(at, title) => {
                if !books.is_empty() {
                    chapters.push((at % books.len(), title.clone()));
                }
            }
            Deed::Paragraph(at, text) => {
                if !chapters.is_empty() {
                    paragraphs.push((at % chapters.len(), text.clone()));
                }
            }
            Deed::Retitle(at, title) => {
                if !books.is_empty() {
                    let row = at % books.len();
                    books[row].0 = title.clone();
                }
            }
            Deed::Rebook(at, to) => {
                if !chapters.is_empty() {
                    let row = at % chapters.len();
                    chapters[row].0 = to % books.len();
                }
            }
            Deed::Settle => {}
        }
    }
    (books, chapters, paragraphs)
}

fn book_at(library: &Library, row: usize) -> Id<Book> {
    use crate::schema::Row;

    Table::id_at(&library.books, Row::trusted(row)).unwrap()
}

fn chapter_at(library: &Library, row: usize) -> Id<Chapter> {
    use crate::schema::Row;

    Table::id_at(&library.chapters, Row::trusted(row)).unwrap()
}

fn paragraph_at(library: &Library, row: usize) -> Id<Paragraph> {
    use crate::schema::Row;

    Table::id_at(&library.paragraphs, Row::trusted(row)).unwrap()
}

fn book_row(library: &Library, of: Id<Book>) -> usize {
    Table::row_of(&library.books, of).unwrap().index()
}

fn chapter_row(library: &Library, of: Id<Chapter>) -> usize {
    Table::row_of(&library.chapters, of).unwrap().index()
}

#[derive(Default)]
struct Minted {
    books: Vec<Id<Book>>,
    chapters: Vec<Id<Chapter>>,
    paragraphs: Vec<Id<Paragraph>>,
}

fn kept(deeds: &[Deed], path: &std::path::Path) -> (Library, Minted) {
    let mut library = Library::create(path, this_device()).unwrap();
    let mut minted = Minted::default();

    for deed in deeds {
        let books = Table::rows(&library.books);
        let chapters = Table::rows(&library.chapters);
        match deed {
            Deed::Book(title, author) => {
                minted
                    .books
                    .push(library.books.push(title, author).unwrap());
            }
            Deed::Chapter(at, title) => {
                if books > 0 {
                    let held = book_at(&library, at % books);
                    minted
                        .chapters
                        .push(library.chapters.push(held, title).unwrap());
                }
            }
            Deed::Paragraph(at, text) => {
                if chapters > 0 {
                    let held = chapter_at(&library, at % chapters);
                    minted
                        .paragraphs
                        .push(library.paragraphs.push(held, text).unwrap());
                }
            }
            Deed::Retitle(at, title) => {
                if books > 0 {
                    let held = book_at(&library, at % books);
                    library.at_mut(held).unwrap().set_title(title).unwrap();
                }
            }
            Deed::Rebook(at, to) => {
                if chapters > 0 {
                    let moved = chapter_at(&library, at % chapters);
                    let home = book_at(&library, to % books);
                    library.at_mut(moved).unwrap().set_book(home).unwrap();
                }
            }
            Deed::Settle => {
                library.commit(this_device()).unwrap();
                library = Library::open(path).unwrap();
            }
        }
    }
    (library, minted)
}

proptest! {
    #[test]
    fn a_library_driven_only_through_its_rows_survives_every_commit_and_reopen(
        deeds in deeds(),
    ) {
        let dir = TempDir::new("declared_survives");
        let path = dir.join("library.dbs");
        let (books, chapters, paragraphs) = expected(&deeds);

        let (mut library, _) = kept(&deeds, &path);
        library.commit(this_device()).unwrap();
        let reopened = Library::open(&path).unwrap();

        prop_assert_eq!(reopened.all::<Book>().count(), books.len());
        for (row, (title, author)) in books.iter().enumerate() {
            let book = reopened.at(book_at(&reopened, row)).unwrap();
            prop_assert_eq!(book.title().unwrap().into_owned(), title.clone());
            prop_assert_eq!(book.author().unwrap().into_owned(), author.clone());
        }

        prop_assert_eq!(reopened.all::<Chapter>().count(), chapters.len());
        for (row, (book, title)) in chapters.iter().enumerate() {
            let chapter = reopened.at(chapter_at(&reopened, row)).unwrap();
            prop_assert_eq!(chapter.title().unwrap().into_owned(), title.clone());
            prop_assert_eq!(book_row(&reopened, chapter.book().unwrap().id().unwrap()), *book);
        }

        prop_assert_eq!(reopened.all::<Paragraph>().count(), paragraphs.len());
        for (row, (chapter, text)) in paragraphs.iter().enumerate() {
            let paragraph = reopened.at(paragraph_at(&reopened, row)).unwrap();
            prop_assert_eq!(paragraph.text().unwrap().into_owned(), text.clone());
            prop_assert_eq!(chapter_row(&reopened, paragraph.chapter().unwrap().id().unwrap()), *chapter);
        }

        let mut walked = Vec::new();
        for book in reopened.all::<Book>() {
            let at = book_row(&reopened, book.id().unwrap());
            let found: Vec<usize> = book
                .chapters()
                .unwrap()
                .map(|chapter| chapter_row(&reopened, chapter.id().unwrap()))
                .collect();
            let scanned: Vec<usize> = chapters
                .iter()
                .enumerate()
                .filter(|(_, (owner, _))| *owner == at)
                .map(|(row, _)| row)
                .collect();
            prop_assert_eq!(&found, &scanned);
            walked.extend(found);
        }
        walked.sort_unstable();
        prop_assert_eq!(walked, (0..chapters.len()).collect::<Vec<_>>());
    }

    #[test]
    fn every_id_a_row_was_given_still_names_that_row_after_every_commit_and_reopen(
        deeds in deeds(),
    ) {
        let dir = TempDir::new("declared_identity");
        let path = dir.join("library.dbs");
        let (books, chapters, paragraphs) = expected(&deeds);

        let (mut library, minted) = kept(&deeds, &path);
        library.commit(this_device()).unwrap();
        let reopened = Library::open(&path).unwrap();

        prop_assert_eq!(minted.books.len(), books.len());
        prop_assert_eq!(minted.chapters.len(), chapters.len());
        prop_assert_eq!(minted.paragraphs.len(), paragraphs.len());

        for (row, id) in minted.books.iter().enumerate() {
            let book = reopened.at(*id).unwrap();
            prop_assert_eq!(book.id().unwrap(), *id);
            prop_assert_eq!(book_row(&reopened, *id), row);
            prop_assert_eq!(book.title().unwrap().into_owned(), books[row].0.clone());
            prop_assert_eq!(id.to_string().parse::<Id<Book>>().unwrap(), *id);
        }

        for (row, id) in minted.chapters.iter().enumerate() {
            let chapter = reopened.at(*id).unwrap();
            prop_assert_eq!(chapter.id().unwrap(), *id);
            prop_assert_eq!(chapter_row(&reopened, *id), row);
            prop_assert_eq!(chapter.title().unwrap().into_owned(), chapters[row].1.clone());
        }

        for (row, id) in minted.paragraphs.iter().enumerate() {
            let paragraph = reopened.at(*id).unwrap();
            prop_assert_eq!(paragraph.id().unwrap(), *id);
            prop_assert_eq!(paragraph.text().unwrap().into_owned(), paragraphs[row].1.clone());
        }

        let mut distinct = minted.books.clone();
        distinct.sort_unstable();
        distinct.dedup();
        prop_assert_eq!(distinct.len(), books.len());

        let stranger = Id::<Book>::mint();
        prop_assert!(!minted.books.contains(&stranger));
        prop_assert!(reopened.at(stranger).is_err());
    }

    #[test]
    fn reading_one_field_through_a_row_faults_in_no_column_it_did_not_name(
        deeds in deeds(),
        at in props::row_index(),
        by_author in any::<bool>(),
    ) {
        let dir = TempDir::new("declared_cold");
        let path = dir.join("library.dbs");

        let (mut library, _) = kept(&deeds, &path);
        library.books.push("Anchor", "Nobody").unwrap();
        library.commit(this_device()).unwrap();

        let reopened = Library::open(&path).unwrap();
        let row = at % Table::rows(&reopened.books);
        let book = reopened.at(book_at(&reopened, row)).unwrap();
        if by_author {
            book.author().unwrap();
        } else {
            book.title().unwrap();
        }

        let unasked = if by_author { "book.title" } else { "book.author" };
        for column in reopened.residency() {
            let elsewhere = column.name.starts_with("chapter.")
                || column.name.starts_with("paragraph.")
                || column.name.starts_with(unasked);
            if elsewhere {
                prop_assert_eq!(
                    column.resident,
                    0,
                    "{} was read though nobody asked for it",
                    column.name
                );
            }
        }
    }
}

fn found_by_title(library: &Library, title: &str) -> Vec<usize> {
    library
        .books
        .find_by_title(title)
        .unwrap()
        .into_iter()
        .map(|of| book_row(library, of))
        .collect()
}

fn shelving(books: &[(String, String)], title: &str) -> Vec<usize> {
    books
        .iter()
        .enumerate()
        .filter(|(_, (held, _))| held == title)
        .map(|(row, _)| row)
        .collect()
}

fn without(path: &std::path::Path, dropped: &[&str]) {
    let file = ColumnFile::open(path).unwrap();
    let kept: Vec<String> = file
        .columns()
        .iter()
        .map(|column| column.name.clone())
        .filter(|name| !dropped.contains(&name.as_str()))
        .collect();

    let mut updater = file.update(this_device()).unwrap();
    for name in &kept {
        updater.keep_column(name).unwrap();
    }
    for name in dropped {
        updater.remove_column(name);
    }
    updater.commit().unwrap();
}

proptest! {
    #![proptest_config(props::scaled(4))]

    #[test]
    fn every_value_a_commit_wrote_is_found_after_a_reopen_that_read_no_payload(
        deeds in deeds(),
        wanted in props::indexable_text(),
    ) {
        let dir = TempDir::new("declared_find");
        let path = dir.join("library.dbs");

        let (books, _, _) = expected(&deeds);
        let (mut library, _) = kept(&deeds, &path);
        library.commit(this_device()).unwrap();
        drop(library);

        let reopened = Library::open(&path).unwrap();
        prop_assert!(
            reopened.residency().iter().all(|held| held.resident == 0),
            "opening a library must read no payload"
        );

        for title in books.iter().map(|(held, _)| held.clone()).chain([wanted]) {
            prop_assert_eq!(
                found_by_title(&reopened, &title),
                shelving(&books, &title),
                "find_by_title and a scan disagree about {:?}",
                title
            );
        }
    }

    #[test]
    fn an_index_stripped_from_a_file_is_rebuilt_on_open_and_answers_as_it_did(
        deeds in deeds(),
        strip in prop::collection::vec(any::<bool>(), 4),
    ) {
        let dir = TempDir::new("declared_strip");
        let path = dir.join("library.dbs");

        let (mut books, _, _) = expected(&deeds);
        let (mut library, _) = kept(&deeds, &path);
        library.books.push("Anchor", "Nobody").unwrap();
        books.push(("Anchor".to_owned(), "Nobody".to_owned()));
        library.commit(this_device()).unwrap();
        drop(library);

        let derived = [
            "book.title.buckets",
            "book.title.links",
            "book.id.buckets",
            "book.id.links",
        ];
        let dropped: Vec<&str> = derived
            .iter()
            .zip(&strip)
            .filter(|(_, strip)| **strip)
            .map(|(name, _)| *name)
            .collect();
        without(&path, &dropped);
        let after = ColumnFile::open(&path).unwrap();
        for name in &dropped {
            prop_assert!(
                after.column_meta(name).is_none(),
                "{} was still in the file after it was dropped",
                name
            );
        }
        drop(after);

        let reopened = Library::open(&path).unwrap();
        Table::audit(&reopened.books).unwrap();

        for title in books.iter().map(|(held, _)| held.clone()) {
            prop_assert_eq!(
                found_by_title(&reopened, &title),
                shelving(&books, &title),
                "an index rebuilt without {:?} disagrees about {:?}",
                dropped,
                title
            );
        }
    }
}

fn credited(library: &mut Library) -> (Id<Book>, Id<Book>, Id<Editor>, Id<Editor>) {
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    let kokoro = library.books.push("Kokoro", "Natsume Soseki").unwrap();
    let ada = library.editors.push("Ada").unwrap();
    let bram = library.editors.push("Bram").unwrap();

    library.credits.push(alice, ada).unwrap();
    library.credits.push(alice, bram).unwrap();
    library.credits.push(kokoro, ada).unwrap();
    (alice, kokoro, ada, bram)
}

fn edited(library: &Library, book: Id<Book>) -> Vec<String> {
    library
        .at(book)
        .unwrap()
        .editors()
        .unwrap()
        .map(|editor| editor.name().unwrap().into_owned())
        .collect()
}

#[test]
fn a_declared_join_crosses_the_same_way_after_a_reopen_that_read_no_payload() {
    let dir = TempDir::new("declared_join");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    let (alice, kokoro, ada, _) = credited(&mut library);
    library.commit(this_device()).unwrap();
    drop(library);

    let reopened = Library::open(&path).unwrap();
    assert!(reopened.residency().iter().all(|held| held.resident == 0));

    assert_eq!(edited(&reopened, alice), ["Ada", "Bram"]);
    assert_eq!(edited(&reopened, kokoro), ["Ada"]);
    assert_eq!(
        reopened
            .at(ada)
            .unwrap()
            .books()
            .unwrap()
            .map(|book| book.title().unwrap().into_owned())
            .collect::<Vec<_>>(),
        ["Alice", "Kokoro"]
    );
    reopened.verify().unwrap();
}

#[test]
fn a_pair_a_file_holds_is_still_refused_after_the_file_is_reopened() {
    let dir = TempDir::new("declared_pair");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    let (alice, _, ada, _) = credited(&mut library);
    library.commit(this_device()).unwrap();
    drop(library);

    let mut reopened = Library::open(&path).unwrap();
    let err = reopened.credits.push(alice, ada).unwrap_err();

    assert!(err.to_string().contains("credit.book_editor"), "{err}");
    assert_eq!(Table::rows(&reopened.credits), 3);
    reopened.verify().unwrap();
}

#[test]
fn a_pair_index_stripped_from_a_file_is_rebuilt_on_open_and_refuses_as_it_did() {
    let dir = TempDir::new("declared_pair_rebuild");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device()).unwrap();
    let (alice, _, ada, _) = credited(&mut library);
    library.commit(this_device()).unwrap();
    drop(library);

    without(
        &path,
        &["credit.book_editor.buckets", "credit.book_editor.links"],
    );
    let stripped = ColumnFile::open(&path).unwrap();
    assert!(stripped.column_meta("credit.book_editor.buckets").is_none());
    assert!(stripped.column_meta("credit.book_editor").is_some());
    drop(stripped);

    let mut reopened = Library::open(&path).unwrap();

    let err = reopened.credits.push(alice, ada).unwrap_err();
    assert!(err.to_string().contains("credit.book_editor"), "{err}");
    assert_eq!(edited(&reopened, alice), ["Ada", "Bram"]);
    reopened.verify().unwrap();
}
