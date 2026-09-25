use crate::error::Result;
use crate::id::Id;
use crate::schema::{Store, Table};
use crate::storage::{ColumnFile, WriterId};
use crate::test_utils::TempDir;
use crate::test_utils::props;
use proptest::prelude::*;

#[crate::entity]
#[merge(history)]
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

#[crate::container(schema = "library", version = 1)]
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
            "book.version",
            "book.version.parent_ends",
            "book.version.parents",
            "book.title",
            "book.title.bytes",
            "book.title.buckets",
            "book.title.links",
            "book.author",
            "book.author.bytes",
            "chapter.id",
            "chapter.id.buckets",
            "chapter.id.links",
            "chapter.version",
            "chapter.version.parent_ends",
            "chapter.version.parents",
            "chapter.book",
            "chapter.book.buckets",
            "chapter.book.links",
            "chapter.title",
            "chapter.title.bytes",
            "paragraph.id",
            "paragraph.id.buckets",
            "paragraph.id.links",
            "paragraph.version",
            "paragraph.version.parent_ends",
            "paragraph.version.parents",
            "paragraph.chapter",
            "paragraph.chapter.buckets",
            "paragraph.chapter.links",
            "paragraph.text",
            "paragraph.text.bytes",
            "editor.id",
            "editor.id.buckets",
            "editor.id.links",
            "editor.version",
            "editor.version.parent_ends",
            "editor.version.parents",
            "editor.name",
            "editor.name.bytes",
            "editor.name.buckets",
            "editor.name.links",
            "credit.id",
            "credit.id.buckets",
            "credit.id.links",
            "credit.version",
            "credit.version.parent_ends",
            "credit.version.parents",
            "credit.book",
            "credit.book.buckets",
            "credit.book.links",
            "credit.editor",
            "credit.editor.buckets",
            "credit.editor.links",
            "dbsync.clock",
            "dbsync.schema",
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
        if crate::test_utils::is_merge_metadata(&reopened, &column.name) {
            continue;
        }
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
        cold("chapter.book").chunks,
        "parent references are resident for visibility resolution"
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

fn texts_of(library: &Library, chapter: Id<Chapter>) -> Result<Vec<String>> {
    library
        .at(chapter)?
        .paragraphs()?
        .map(|paragraph| Ok(paragraph.text()?.into_owned()))
        .collect()
}

#[test]
fn editing_a_paragraph_keeps_its_place_among_its_siblings() -> Result<()> {
    let mut library = Library::default();
    let on = stock(&mut library)?;
    let first = library.children_of::<Chapter, Paragraph>(on.rabbit)?[0];

    library
        .paragraphs
        .set_text(first, "Alice was very tired indeed.")?;

    assert_eq!(
        texts_of(&library, on.rabbit)?,
        [
            "Alice was very tired indeed.",
            "So she was considering in her own mind.",
        ]
    );
    Ok(())
}

#[test]
fn a_child_moved_to_another_parent_takes_its_creation_order_with_it() -> Result<()> {
    let mut library = Library::default();
    let on = stock(&mut library)?;
    let first = library.children_of::<Chapter, Paragraph>(on.rabbit)?[0];

    library.paragraphs.set_chapter(first, on.sensei)?;

    assert_eq!(
        texts_of(&library, on.sensei)?,
        [
            "Alice was beginning to get very tired.",
            "I always called him Sensei.",
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
    assert_eq!(
        moved,
        ["Down the Rabbit-Hole", "Sensei and I"],
        "a chapter written over keeps the place its creation gave it"
    );
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
        opened
            .residency()
            .iter()
            .filter(|held| !crate::test_utils::is_merge_metadata(&opened, &held.name))
            .all(|held| held.resident == 0),
        "opening reads merge metadata but leaves ordinary payload cold"
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
    let reconciled = stale.commit(this_device())?;
    assert!(reconciled > published);
    assert_eq!(stale.at(alice)?.title()?, "Alice in Wonderland");
    assert_eq!(stale.all::<Book>().count(), 4);
    let token = crate::storage::ColumnFile::read_commit_token(&path)?;
    assert_eq!(token.generation, reconciled);
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

fn minted_in<T: Table>(table: &T) -> Vec<Id<T::Of>> {
    use crate::schema::Row;

    let mut seen: Vec<Id<T::Of>> = Vec::new();
    for index in 0..Table::rows(table) {
        let id = Table::id_at(table, Row::trusted(index)).unwrap();
        if !seen.contains(&id) {
            seen.push(id);
        }
    }
    seen
}

fn book_at(library: &Library, row: usize) -> Id<Book> {
    minted_in(&library.books)[row]
}

fn chapter_at(library: &Library, row: usize) -> Id<Chapter> {
    minted_in(&library.chapters)[row]
}

fn paragraph_at(library: &Library, row: usize) -> Id<Paragraph> {
    minted_in(&library.paragraphs)[row]
}

fn book_row(library: &Library, of: Id<Book>) -> usize {
    minted_in(&library.books)
        .iter()
        .position(|held| *held == of)
        .expect("an id this library minted")
}

fn chapter_row(library: &Library, of: Id<Chapter>) -> usize {
    minted_in(&library.chapters)
        .iter()
        .position(|held| *held == of)
        .expect("an id this library minted")
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
        let books = minted_in(&library.books).len();
        let chapters = minted_in(&library.chapters).len();
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
            let mut found: Vec<usize> = book
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
            found.sort_unstable();
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
        let row = at % minted_in(&reopened.books).len();
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
            if elsewhere && !crate::test_utils::is_merge_metadata(&reopened, &column.name) {
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
    let mut named: Vec<usize> = library
        .books
        .find_by_title(title)
        .unwrap()
        .into_iter()
        .map(|of| book_row(library, of))
        .collect();
    named.sort_unstable();
    named
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
            reopened
                .residency()
                .iter()
                .filter(|held| !crate::test_utils::is_merge_metadata(&reopened, &held.name))
                .all(|held| held.resident == 0),
            "opening reads merge metadata but leaves ordinary payload cold"
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
    assert!(
        reopened
            .residency()
            .iter()
            .filter(|held| !crate::test_utils::is_merge_metadata(&reopened, &held.name))
            .all(|held| held.resident == 0),
        "the clock is the one column an open reads, because a store cannot mint without it"
    );

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
fn repeated_relationship_pairs_survive_a_reopen() {
    let dir = TempDir::new("declared_pair");
    let path = dir.join("library.dbs");
    let mut library = Library::create(&path, this_device()).unwrap();
    let (alice, _, ada, _) = credited(&mut library);
    library.commit(this_device()).unwrap();
    let mut reopened = Library::open(&path).unwrap();
    reopened.credits.push(alice, ada).unwrap();
    reopened.commit(this_device()).unwrap();
    assert_eq!(Table::rows(&Library::open(&path).unwrap().credits), 4);
    reopened.verify().unwrap();
}

#[test]
fn a_relationship_index_is_rebuilt_without_enforcing_pair_uniqueness() {
    let dir = TempDir::new("declared_reference_rebuild");
    let path = dir.join("library.dbs");
    let mut library = Library::create(&path, this_device()).unwrap();
    let (alice, _, ada, _) = credited(&mut library);
    library.commit(this_device()).unwrap();
    without(&path, &["credit.book.buckets", "credit.book.links"]);
    let mut reopened = Library::open(&path).unwrap();
    assert_eq!(
        reopened.children_of::<Book, Credit>(alice).unwrap().len(),
        2
    );
    reopened.credits.push(alice, ada).unwrap();
    assert_eq!(
        reopened.children_of::<Book, Credit>(alice).unwrap().len(),
        3
    );
    reopened.verify().unwrap();
}

#[test]
fn deleting_a_book_hides_its_children_without_deleting_them() -> Result<()> {
    let mut library = Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll")?;
    let rabbit = library.chapters.push(alice, "Down the Rabbit-Hole")?;
    library.remove(alice)?;
    assert_eq!(library.all::<Book>().count(), 0);
    assert_eq!(library.all::<Chapter>().count(), 0);
    assert_eq!(library.at_hidden(rabbit)?.title()?, "Down the Rabbit-Hole");
    library.remove(rabbit)?;
    assert_eq!(library.status(rabbit)?, crate::schema::RowStatus::Deleted);
    library.audit()?;
    Ok(())
}

#[test]
fn a_declared_store_keeps_a_book_it_took_out_across_a_commit_and_a_reopen() -> Result<()> {
    let dir = TempDir::new("declared_remove");
    let path = dir.join("library.dbs");

    let mut library = Library::create(&path, this_device())?;
    let alice = library.books.push("Alice", "Lewis Carroll")?;
    library.books.push("Kokoro", "Natsume Soseki")?;
    library.remove(alice)?;
    library.commit(this_device())?;
    drop(library);

    let reopened = Library::open(&path)?;

    assert_eq!(reopened.all::<Book>().count(), 1);
    assert!(reopened.books.find_by_title("Alice")?.is_empty());
    assert!(reopened.at(alice).is_err());
    assert_eq!(
        Table::rows(&reopened.books),
        3,
        "a removal is written down like anything else, and it says which row it ended"
    );
    reopened.verify()?;
    Ok(())
}

#[test]
fn a_declared_entity_carries_the_retention_it_was_marked_with() {
    use crate::schema::{Entity, Retention};

    assert_eq!(Book::KEEPS, Retention::History);
    assert!(Book::KEEPS.keeps_what_was_written_over());
    assert_eq!(
        Chapter::KEEPS,
        Retention::Newest,
        "an entity that says nothing keeps only what it needs to"
    );
    assert!(!Chapter::KEEPS.keeps_what_was_written_over());
}

fn device(named: &str) -> WriterId {
    WriterId::from_device_id(named)
}

fn titles(library: &Library) -> Vec<String> {
    let mut held: Vec<String> = library
        .all::<Book>()
        .map(|book| book.title().unwrap().into_owned())
        .collect();
    held.sort();
    held
}

fn copied(dir: &TempDir, from: &str, to: &str) -> Result<Library> {
    std::fs::copy(dir.join(from), dir.join(to))?;
    Library::open(dir.join(to))
}

#[test]
fn two_copies_edited_apart_end_up_holding_what_both_of_them_wrote() -> Result<()> {
    let dir = TempDir::new("merge_apart");
    let mut mine = Library::create(dir.join("here.dbs"), device("here"))?;
    mine.books.push("Alice", "Lewis Carroll")?;
    mine.commit(device("here"))?;

    let mut theirs = copied(&dir, "here.dbs", "there.dbs")?;
    theirs.books.push("Kokoro", "Natsume Soseki")?;
    theirs.commit(device("there"))?;
    mine.books.push("An Atlas", "many hands")?;
    mine.commit(device("here"))?;

    let report = mine.merge_snapshot(&theirs)?;

    assert_eq!(titles(&mine), ["Alice", "An Atlas", "Kokoro"]);
    assert_eq!(report.taken, 1, "only their new book was news here");
    assert!(report.is_quiet(), "{report:?}");
    mine.audit()?;
    Ok(())
}

#[test]
fn two_copies_that_wrote_into_one_chapter_apart_walk_it_in_the_same_order() -> Result<()> {
    let dir = TempDir::new("merge_order");
    let mut mine = Library::create(dir.join("here.dbs"), device("here"))?;
    let alice = mine.books.push("Alice", "Lewis Carroll")?;
    let rabbit = mine.chapters.push(alice, "Down the Rabbit-Hole")?;
    let tired = mine.paragraphs.push(rabbit, "Alice was tired.")?;
    mine.commit(device("here"))?;

    let mut theirs = copied(&dir, "here.dbs", "there.dbs")?;
    mine.paragraphs.push(rabbit, "Written here first.")?;
    theirs.paragraphs.push(rabbit, "Written there second.")?;
    mine.paragraphs.set_text(tired, "Alice was very tired.")?;
    theirs.paragraphs.push(rabbit, "Written there third.")?;
    mine.commit(device("here"))?;
    theirs.commit(device("there"))?;

    let mut other = copied(&dir, "there.dbs", "other.dbs")?;
    let mut again = copied(&dir, "here.dbs", "again.dbs")?;
    mine.merge_snapshot(&theirs)?;
    other.merge_snapshot(&again)?;
    again.merge_snapshot(&other)?;

    let walked = texts_of(&mine, rabbit)?;
    assert_eq!(
        walked,
        [
            "Alice was very tired.",
            "Written here first.",
            "Written there second.",
            "Written there third.",
        ]
    );
    assert_eq!(texts_of(&other, rabbit)?, walked);
    assert_eq!(texts_of(&again, rabbit)?, walked);
    Ok(())
}

#[test]
fn merging_the_same_copy_twice_says_the_second_time_that_it_knew_it_all() -> Result<()> {
    let dir = TempDir::new("merge_twice");
    let mut mine = Library::create(dir.join("here.dbs"), device("here"))?;
    mine.books.push("Alice", "Lewis Carroll")?;
    mine.commit(device("here"))?;

    let mut theirs = copied(&dir, "here.dbs", "there.dbs")?;
    theirs.books.push("Kokoro", "Natsume Soseki")?;
    theirs.commit(device("there"))?;

    let once = mine.merge_snapshot(&theirs)?;
    let rows = Table::rows(&mine.books);
    let again = mine.merge_snapshot(&theirs)?;

    assert_eq!(once.taken, 1);
    assert_eq!(again.taken, 0, "{again:?}");
    assert!(again.known > 0);
    assert_eq!(
        Table::rows(&mine.books),
        rows,
        "a second merge wrote nothing"
    );
    assert_eq!(titles(&mine), ["Alice", "Kokoro"]);
    Ok(())
}

#[test]
fn two_copies_merged_in_either_order_hold_the_same_books() -> Result<()> {
    let dir = TempDir::new("merge_either");
    let mut mine = Library::create(dir.join("here.dbs"), device("here"))?;
    mine.books.push("Alice", "Lewis Carroll")?;
    mine.commit(device("here"))?;

    let mut theirs = copied(&dir, "here.dbs", "there.dbs")?;
    theirs.books.push("Kokoro", "Natsume Soseki")?;
    theirs.commit(device("there"))?;
    mine.books.push("An Atlas", "many hands")?;
    mine.commit(device("here"))?;

    let mut ours = mine.clone();
    ours.merge_snapshot(&theirs)?;
    let mut backwards = theirs.clone();
    backwards.merge_snapshot(&mine)?;

    assert_eq!(titles(&ours), titles(&backwards));
    assert_eq!(titles(&ours), ["Alice", "An Atlas", "Kokoro"]);
    Ok(())
}

#[test]
fn a_row_two_copies_wrote_apart_is_kept_by_the_later_write_and_the_other_is_reported() -> Result<()>
{
    let dir = TempDir::new("merge_concurrent");
    let mut mine = Library::create(dir.join("here.dbs"), device("here"))?;
    let alice = mine.books.push("Alice", "Lewis Carroll")?;
    mine.commit(device("here"))?;

    let mut theirs = copied(&dir, "here.dbs", "there.dbs")?;
    theirs.at_mut(alice)?.set_title("Alice's Adventures")?;
    theirs.commit(device("there"))?;
    std::thread::sleep(std::time::Duration::from_millis(2));
    mine.at_mut(alice)?.set_title("Alice in Wonderland")?;
    mine.commit(device("here"))?;

    let ours = Table::versions(&mine.books)
        .at(mine.books.row_of(alice)?)?
        .dot();
    let theirs_dot = Table::versions(&theirs.books)
        .at(theirs.books.row_of(alice)?)?
        .dot();

    let report = mine.merge_snapshot(&theirs)?;

    assert_eq!(report.concurrent.len(), 1, "{report:?}");
    let said = report.concurrent[0];
    assert_eq!(said.entity, "book");
    assert_eq!(said.id, alice.uuid());
    assert!(
        ours > theirs_dot,
        "this copy wrote a clear two milliseconds later: {ours:?} against {theirs_dot:?}"
    );
    assert_eq!(said.kept, ours);
    assert_eq!(said.dropped, theirs_dot);
    assert_eq!(
        mine.books.title(alice)?,
        "Alice in Wonderland",
        "the row that counted further is the one the id names"
    );
    assert_eq!(titles(&mine).len(), 1, "one book, however many wrote it");
    mine.audit()?;
    Ok(())
}

#[test]
fn three_copies_that_declared_the_same_pair_keep_three_credits() -> Result<()> {
    let dir = TempDir::new("merge_three_pairs");
    let mut first = Library::create(dir.join("first.dbs"), device("first"))?;
    let alice = first.books.push("Alice", "Lewis Carroll")?;
    let carroll = first.editors.push("Lewis Carroll")?;
    first.commit(device("first"))?;

    let mut second = copied(&dir, "first.dbs", "second.dbs")?;
    let mut third = copied(&dir, "first.dbs", "third.dbs")?;
    for (shelf, who) in [
        (&mut second, "second"),
        (&mut third, "third"),
        (&mut first, "first"),
    ] {
        shelf.credits.push(alice, carroll)?;
        shelf.commit(device(who))?;
    }

    first.merge_snapshot(&second)?;
    let report = first.merge_snapshot(&third)?;

    let live = first.children_of::<Book, Credit>(alice)?;
    assert_eq!(
        live.len(),
        3,
        "three different UUIDs remain independent: {report:?}"
    );
    assert_eq!(
        Table::rows(&first.credits),
        3,
        "the two that gave it up are still rows that say what they held"
    );

    first.audit()?;
    Ok(())
}

#[test]
fn equal_parent_pairs_on_different_ids_are_kept_without_conflict() -> Result<()> {
    let dir = TempDir::new("merge_pair");
    let mut mine = Library::create(dir.join("here.dbs"), device("here"))?;
    let alice = mine.books.push("Alice", "Lewis Carroll")?;
    let carroll = mine.editors.push("Lewis Carroll")?;
    mine.commit(device("here"))?;

    let mut theirs = copied(&dir, "here.dbs", "there.dbs")?;
    theirs.credits.push(alice, carroll)?;
    theirs.commit(device("there"))?;
    mine.credits.push(alice, carroll)?;
    mine.commit(device("here"))?;

    let report = mine.merge_snapshot(&theirs)?;

    assert!(report.is_quiet());
    assert_eq!(
        mine.children_of::<Book, Credit>(alice)?.len(),
        2,
        "different UUIDs remain independent relationship rows"
    );
    mine.audit()?;
    Ok(())
}

#[test]
fn a_copy_with_a_write_nobody_published_is_refused_before_anything_is_merged() -> Result<()> {
    let dir = TempDir::new("merge_unpublished");
    let mut mine = Library::create(dir.join("here.dbs"), device("here"))?;
    mine.books.push("Alice", "Lewis Carroll")?;
    mine.commit(device("here"))?;
    let theirs = copied(&dir, "here.dbs", "there.dbs")?;

    mine.books.push("An Atlas", "many hands")?;
    let err = mine
        .merge_snapshot(&theirs)
        .expect_err("a write with no name of its own cannot be merged");

    assert!(err.to_string().contains("nobody has published"), "{err}");
    Ok(())
}

#[test]
fn three_copies_merged_in_either_grouping_hold_the_same_books() -> Result<()> {
    let dir = TempDir::new("merge_three");
    let mut first = Library::create(dir.join("first.dbs"), device("first"))?;
    first.books.push("Alice", "Lewis Carroll")?;
    first.commit(device("first"))?;

    let mut second = copied(&dir, "first.dbs", "second.dbs")?;
    let mut third = copied(&dir, "first.dbs", "third.dbs")?;
    second.books.push("Kokoro", "Natsume Soseki")?;
    second.commit(device("second"))?;
    third.books.push("An Atlas", "many hands")?;
    third.commit(device("third"))?;

    let mut leftwards = first.clone();
    leftwards.merge_snapshot(&second)?;
    leftwards.merge_snapshot(&third)?;

    let mut gathered = second.clone();
    gathered.merge_snapshot(&third)?;
    let mut rightwards = first.clone();
    rightwards.merge_snapshot(&gathered)?;

    assert_eq!(titles(&leftwards), titles(&rightwards));
    assert_eq!(titles(&leftwards), ["Alice", "An Atlas", "Kokoro"]);
    leftwards.audit()?;
    rightwards.audit()?;
    Ok(())
}

#[test]
fn a_row_only_the_other_copy_wrote_is_taken_without_calling_it_a_disagreement() -> Result<()> {
    let dir = TempDir::new("merge_later");
    let mut mine = Library::create(dir.join("here.dbs"), device("here"))?;
    let alice = mine.books.push("Alice", "Lewis Carroll")?;
    mine.commit(device("here"))?;

    let mut theirs = copied(&dir, "here.dbs", "there.dbs")?;
    theirs.at_mut(alice)?.set_title("Alice's Adventures")?;
    theirs.commit(device("there"))?;

    let report = mine.merge_snapshot(&theirs)?;

    assert_eq!(mine.books.title(alice)?, "Alice's Adventures");
    assert_eq!(report.taken, 1);
    assert!(
        report.concurrent.is_empty(),
        "they wrote after seeing what I had, so nobody disagreed: {report:?}"
    );
    mine.audit()?;
    Ok(())
}

#[test]
fn a_merge_retains_and_hides_a_child_whose_parent_was_deleted() -> Result<()> {
    let dir = TempDir::new("merge_orphan");
    let mut mine = Library::create(dir.join("here.dbs"), device("here"))?;
    let alice = mine.books.push("Alice", "Lewis Carroll")?;
    mine.commit(device("here"))?;
    let mut theirs = copied(&dir, "here.dbs", "there.dbs")?;
    let chapter = theirs.chapters.push(alice, "Down the Rabbit-Hole")?;
    theirs.commit(device("there"))?;
    mine.remove(alice)?;
    mine.commit(device("here"))?;
    let report = mine.merge_snapshot(&theirs)?;
    assert_eq!(report.references.len(), 1);
    assert_eq!(mine.all::<Book>().count(), 0);
    assert_eq!(mine.all::<Chapter>().count(), 0);
    assert_eq!(mine.at_hidden(chapter)?.title()?, "Down the Rabbit-Hole");
    mine.audit()?;
    Ok(())
}
