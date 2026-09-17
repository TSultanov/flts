use crate::error::Result;
use crate::id::Id;
use crate::schema::{Store, Table};
use crate::storage::WriterId;
use crate::test_utils::TempDir;
use crate::test_utils::props;
use proptest::prelude::*;

#[crate::entity]
struct Book {
    title: String,
    author: String,
    chapters: Many<Chapter>,
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
}

fn this_device() -> WriterId {
    WriterId::from_device_id("KRZH3XY-7PH4NJK-2WQ5LMC")
}

fn stock(library: &mut Library) -> Result<Id<Book>> {
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
    Ok(alice)
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
            "book.title",
            "book.title.bytes",
            "book.author",
            "book.author.bytes",
            "chapter.book",
            "chapter.title",
            "chapter.title.bytes",
            "paragraph.chapter",
            "paragraph.text",
            "paragraph.text.bytes",
        ]
    );
}

#[test]
fn a_declared_library_joins_two_levels_down() -> Result<()> {
    let mut library = Library::default();
    let alice = stock(&mut library)?;

    let chapters: &[Id<Chapter>] = library.children_of(alice)?;
    assert_eq!(chapters.len(), 2);
    assert_eq!(library.chapters.title(chapters[0])?, "Down the Rabbit-Hole");

    let paragraphs: &[Id<Paragraph>] = library.children_of(chapters[0])?;
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
    let alice = stock(&mut library)?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;
    reopened.verify()?;

    assert_eq!(Table::rows(&reopened.books), 2);
    assert_eq!(reopened.books.title(alice)?, "Alice");
    assert_eq!(reopened.books.author(alice)?, "Lewis Carroll");

    let chapters: &[Id<Chapter>] = reopened.children_of(alice)?;
    assert_eq!(chapters.len(), 2);
    let paragraphs: &[Id<Paragraph>] = reopened.children_of(chapters[1])?;
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
    stock(&mut library)?;
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
    stock(&mut library)?;
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
    let alice = stock(&mut library)?;
    library.commit(this_device())?;

    library.books.set_title(alice, "Alice in Wonderland")?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;
    assert_eq!(reopened.books.title(alice)?, "Alice in Wonderland");
    assert_eq!(reopened.books.author(alice)?, "Lewis Carroll");
    assert_eq!(
        reopened.books.title(Id::from_index(1))?,
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
    stock(&mut library)?;
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
    stock(&mut library)?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;
    for row in 0..Table::rows(&reopened.books) {
        reopened.books.title(Id::from_index(row))?;
        reopened.books.author(Id::from_index(row))?;
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
fn a_declared_row_answers_with_its_own_fields() -> Result<()> {
    let mut library = Library::default();
    let alice = stock(&mut library)?;

    let book = library.at(alice)?;

    assert_eq!(book.title()?, "Alice");
    assert_eq!(book.author()?, "Lewis Carroll");
    assert_eq!(book.id(), alice);
    Ok(())
}

#[test]
fn every_declared_row_of_a_table_can_be_walked_in_turn() -> Result<()> {
    let mut library = Library::default();
    stock(&mut library)?;

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
    stock(&mut library)?;
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
    let alice = stock(&mut library)?;

    let book = library.at(alice)?;
    let mut read = Vec::new();
    for chapter in book.chapters()? {
        read.push(chapter.title()?.into_owned());
        for paragraph in chapter.paragraphs()? {
            read.push(paragraph.text()?.into_owned());
            assert_eq!(paragraph.chapter()?.id(), chapter.id());
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
    stock(&mut library)?;

    let sensei = library.at(Id::<Chapter>::from_index(2))?;
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
    stock(&mut library)?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;
    let kokoro = reopened.at(Id::<Book>::from_index(1))?;
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
    let alice = stock(&mut library)?;
    library.commit(this_device())?;

    library.at_mut(alice)?.set_title("Alice in Wonderland")?;
    library.commit(this_device())?;

    let reopened = Library::open(&path)?;
    assert_eq!(reopened.at(alice)?.title()?, "Alice in Wonderland");
    assert_eq!(reopened.at(alice)?.author()?, "Lewis Carroll");
    assert_eq!(reopened.at(Id::<Book>::from_index(1))?.title()?, "Kokoro");
    Ok(())
}

#[test]
fn a_chapter_moved_through_a_row_is_grouped_under_the_book_it_moved_to() -> Result<()> {
    let mut library = Library::default();
    let alice = stock(&mut library)?;
    let kokoro = Id::<Book>::from_index(1);
    assert_eq!(library.at(alice)?.chapters()?.count(), 2);

    library
        .at_mut(Id::<Chapter>::from_index(0))?
        .set_book(kokoro)?;

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
    let alice = stock(&mut library)?;
    let shelved = library.commit(this_device())?;

    let opened = Library::open(&path)?;
    assert!(
        opened.residency().iter().all(|held| held.resident == 0),
        "opening a library must read no payload"
    );

    let book = opened.at(alice)?;
    let mut read = Vec::new();
    for chapter in book.chapters()? {
        assert_eq!(chapter.book()?.id(), book.id());
        for paragraph in chapter.paragraphs()? {
            assert_eq!(paragraph.chapter()?.id(), chapter.id());
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

fn kept(deeds: &[Deed], path: &std::path::Path) -> Library {
    let mut library = Library::create(path, this_device()).unwrap();

    for deed in deeds {
        let books = Table::rows(&library.books);
        let chapters = Table::rows(&library.chapters);
        match deed {
            Deed::Book(title, author) => {
                library.books.push(title, author).unwrap();
            }
            Deed::Chapter(at, title) => {
                if books > 0 {
                    library
                        .chapters
                        .push(Id::from_index(at % books), title)
                        .unwrap();
                }
            }
            Deed::Paragraph(at, text) => {
                if chapters > 0 {
                    library
                        .paragraphs
                        .push(Id::from_index(at % chapters), text)
                        .unwrap();
                }
            }
            Deed::Retitle(at, title) => {
                if books > 0 {
                    library
                        .at_mut(Id::<Book>::from_index(at % books))
                        .unwrap()
                        .set_title(title)
                        .unwrap();
                }
            }
            Deed::Rebook(at, to) => {
                if chapters > 0 {
                    library
                        .at_mut(Id::<Chapter>::from_index(at % chapters))
                        .unwrap()
                        .set_book(Id::from_index(to % books))
                        .unwrap();
                }
            }
            Deed::Settle => {
                library.commit(this_device()).unwrap();
                library = Library::open(path).unwrap();
            }
        }
    }
    library
}

proptest! {
    #[test]
    fn a_library_driven_only_through_its_rows_survives_every_commit_and_reopen(
        deeds in deeds(),
    ) {
        let dir = TempDir::new("declared_survives");
        let path = dir.join("library.dbs");
        let (books, chapters, paragraphs) = expected(&deeds);

        let mut library = kept(&deeds, &path);
        library.commit(this_device()).unwrap();
        let reopened = Library::open(&path).unwrap();

        prop_assert_eq!(reopened.all::<Book>().count(), books.len());
        for (row, (title, author)) in books.iter().enumerate() {
            let book = reopened.at(Id::<Book>::from_index(row)).unwrap();
            prop_assert_eq!(book.title().unwrap().into_owned(), title.clone());
            prop_assert_eq!(book.author().unwrap().into_owned(), author.clone());
        }

        prop_assert_eq!(reopened.all::<Chapter>().count(), chapters.len());
        for (row, (book, title)) in chapters.iter().enumerate() {
            let chapter = reopened.at(Id::<Chapter>::from_index(row)).unwrap();
            prop_assert_eq!(chapter.title().unwrap().into_owned(), title.clone());
            prop_assert_eq!(chapter.book().unwrap().id().index(), *book);
        }

        prop_assert_eq!(reopened.all::<Paragraph>().count(), paragraphs.len());
        for (row, (chapter, text)) in paragraphs.iter().enumerate() {
            let paragraph = reopened.at(Id::<Paragraph>::from_index(row)).unwrap();
            prop_assert_eq!(paragraph.text().unwrap().into_owned(), text.clone());
            prop_assert_eq!(paragraph.chapter().unwrap().id().index(), *chapter);
        }

        let mut walked = Vec::new();
        for book in reopened.all::<Book>() {
            let at = book.id().index();
            let found: Vec<usize> = book
                .chapters()
                .unwrap()
                .map(|chapter| chapter.id().index())
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
    fn reading_one_field_through_a_row_faults_in_no_column_it_did_not_name(
        deeds in deeds(),
        at in props::row_index(),
        by_author in any::<bool>(),
    ) {
        let dir = TempDir::new("declared_cold");
        let path = dir.join("library.dbs");

        let mut library = kept(&deeds, &path);
        library.books.push("Anchor", "Nobody").unwrap();
        library.commit(this_device()).unwrap();

        let reopened = Library::open(&path).unwrap();
        let row = at % Table::rows(&reopened.books);
        let book = reopened.at(Id::<Book>::from_index(row)).unwrap();
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
