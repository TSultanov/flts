use super::*;
use crate::error::Result;
use crate::id::Id;
use crate::test_utils::library::{Binding, Book, Chapter, Edition, Library, Paragraph};
use crate::test_utils::props;
use proptest::prelude::*;

fn refused<T>(result: Result<T>) -> String {
    match result {
        Ok(_) => panic!("this id must not have minted a handle"),
        Err(err) => err.to_string(),
    }
}

struct Shelved {
    alice: Id<Book>,
    kokoro: Id<Book>,
    rabbit: Id<Chapter>,
    tired: Id<Paragraph>,
}

fn stocked() -> (Library, Shelved) {
    let mut library = Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    let kokoro = library.books.push("Kokoro", "Natsume Soseki").unwrap();

    let rabbit = library
        .chapters
        .push(alice, "Down the Rabbit-Hole")
        .unwrap();
    let tired = library.paragraphs.push(rabbit, "Alice was tired.").unwrap();
    (
        library,
        Shelved {
            alice,
            kokoro,
            rabbit,
            tired,
        },
    )
}

#[test]
fn a_handle_reads_the_fields_of_the_row_it_was_minted_for() {
    let (library, on) = stocked();

    let kokoro = library.at(on.kokoro).unwrap();

    assert_eq!(kokoro.title().unwrap(), "Kokoro");
    assert_eq!(kokoro.author().unwrap(), "Natsume Soseki");
}

#[test]
fn a_handle_answers_with_the_id_it_was_minted_from() {
    let (library, on) = stocked();
    let asked = on.kokoro;

    assert_eq!(library.at(asked).unwrap().id().unwrap(), asked);
}

#[test]
fn a_handle_is_refused_for_an_id_no_row_holds() {
    let (library, _) = stocked();
    let stranger = Id::<Book>::mint();

    let err = refused(library.at(stranger));

    assert!(err.contains("book has no row with id"), "{err}");
    assert!(err.contains(&stranger.to_string()), "{err}");
}

#[test]
fn a_handle_is_refused_by_the_table_that_holds_the_entity_not_by_another() {
    let (library, on) = stocked();

    let err = refused(library.at(on.alice.cast::<Paragraph>()));

    assert!(err.contains("paragraph has no row with id"), "{err}");
}

#[test]
fn a_handle_reaches_the_store_and_the_table_it_was_minted_from() {
    let (library, on) = stocked();

    let alice = library.at(on.alice).unwrap();

    assert_eq!(Table::rows(alice.table()), 2);
    assert_eq!(Table::rows(&alice.store().paragraphs), 1);
    assert_eq!(alice.row().index(), 0);
}

#[test]
fn two_handles_into_the_same_store_are_alive_at_once() {
    let (library, on) = stocked();

    let alice = library.at(on.alice).unwrap();
    let kokoro = library.at(on.kokoro).unwrap();

    assert_eq!(alice.title().unwrap(), "Alice");
    assert_eq!(kokoro.title().unwrap(), "Kokoro");
}

#[test]
fn iterating_an_empty_table_yields_nothing() {
    let library = Library::default();

    assert_eq!(library.all::<Book>().count(), 0);
}

#[test]
fn iterating_a_table_yields_every_row_once_in_the_order_they_were_pushed() {
    let (library, _) = stocked();

    let titles: Vec<String> = library
        .all::<Book>()
        .map(|book| book.title().unwrap().into_owned())
        .collect();

    assert_eq!(titles, ["Alice", "Kokoro"]);
}

#[test]
fn iterating_a_table_of_one_row_yields_that_row_and_stops() {
    let (library, _) = stocked();

    let mut paragraphs = library.all::<Paragraph>();

    assert_eq!(
        paragraphs.next().unwrap().text().unwrap(),
        "Alice was tired."
    );
    assert!(paragraphs.next().is_none());
}

#[test]
fn an_iterated_handle_carries_the_id_of_the_row_it_stands_for() {
    let (library, on) = stocked();

    let ids: Vec<Id<Book>> = library
        .all::<Book>()
        .map(|book| book.id().unwrap())
        .collect();

    assert_eq!(ids, [on.alice, on.kokoro]);
}

#[test]
fn every_table_in_a_store_mints_handles_of_its_own_entity() {
    let (library, _) = stocked();

    let chapters: Vec<String> = library
        .all::<Chapter>()
        .map(|chapter| chapter.title().unwrap().into_owned())
        .collect();

    assert_eq!(chapters, ["Down the Rabbit-Hole"]);
    assert_eq!(library.all::<Book>().count(), 2);
}

#[test]
fn a_handle_follows_a_reference_into_the_row_it_points_at() {
    let (library, on) = stocked();

    let chapter = library.at(on.rabbit).unwrap();

    assert_eq!(chapter.book().unwrap().title().unwrap(), "Alice");
    assert_eq!(chapter.book().unwrap().id().unwrap(), on.alice);
}

#[test]
fn a_handle_walks_two_levels_down_and_one_back_up() {
    let (library, on) = stocked();

    let alice = library.at(on.alice).unwrap();
    let chapter = alice.chapters().unwrap().next().unwrap();
    let paragraph = chapter.paragraphs().unwrap().next().unwrap();

    assert_eq!(paragraph.text().unwrap(), "Alice was tired.");
    assert_eq!(
        paragraph.chapter().unwrap().id().unwrap(),
        chapter.id().unwrap()
    );
}

#[test]
fn a_handle_whose_row_has_no_children_walks_nowhere() {
    let (library, on) = stocked();

    let kokoro = library.at(on.kokoro).unwrap();

    assert_eq!(kokoro.chapters().unwrap().count(), 0);
}

#[test]
fn walking_the_children_of_a_row_hands_back_only_that_row_s_children() {
    let (mut library, on) = stocked();
    let kokoro = on.kokoro;
    library.chapters.push(kokoro, "Sensei and I").unwrap();
    library
        .chapters
        .push(on.alice, "The Pool of Tears")
        .unwrap();

    let titles: Vec<String> = library
        .at(kokoro)
        .unwrap()
        .chapters()
        .unwrap()
        .map(|chapter| chapter.title().unwrap().into_owned())
        .collect();

    assert_eq!(titles, ["Sensei and I"]);
    assert_eq!(library.at(on.alice).unwrap().chapters().unwrap().count(), 2);
}

#[test]
fn walking_the_children_of_a_row_refuses_a_reference_no_parent_answers_for() {
    let mut library = Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    library
        .chapters
        .push(Id::mint(), "Belongs to nobody")
        .unwrap();

    let err = refused(library.at(alice).unwrap().chapters());

    assert!(err.contains("chapter.book"), "{err}");
}

#[test]
fn a_mutable_handle_writes_the_field_it_names_and_leaves_its_neighbours_alone() {
    let (mut library, on) = stocked();
    let alice = on.alice;

    library
        .at_mut(alice)
        .unwrap()
        .set_title("Alice in Wonderland")
        .unwrap();

    assert_eq!(
        library.at(alice).unwrap().title().unwrap(),
        "Alice in Wonderland"
    );
    assert_eq!(
        library.at(alice).unwrap().author().unwrap(),
        "Lewis Carroll"
    );
    assert_eq!(library.at(on.kokoro).unwrap().title().unwrap(), "Kokoro");
}

#[test]
fn a_mutable_handle_answers_with_the_id_it_was_minted_from() {
    let (mut library, on) = stocked();
    let asked = on.tired;

    let mut paragraph = library.at_mut(asked).unwrap();
    paragraph.set_text("She was very tired indeed.").unwrap();

    assert_eq!(paragraph.id().unwrap(), asked);
    assert_eq!(paragraph.row().index(), 0);
    assert_eq!(
        library.at(asked).unwrap().text().unwrap(),
        "She was very tired indeed."
    );
}

#[test]
fn a_mutable_handle_is_refused_for_an_id_no_row_holds() {
    let (mut library, _) = stocked();

    let err = refused(library.at_mut(Id::<Book>::mint()));

    assert!(err.contains("book has no row with id"), "{err}");
}

#[test]
fn moving_a_child_through_a_handle_forgets_the_grouping_it_belonged_to() {
    let (mut library, on) = stocked();
    let alice = on.alice;
    let kokoro = on.kokoro;
    assert_eq!(library.at(alice).unwrap().chapters().unwrap().count(), 1);
    assert!(library.chapters.is_indexed());

    library.at_mut(on.rabbit).unwrap().set_book(kokoro).unwrap();

    assert!(
        !library.chapters.is_indexed(),
        "a grouping that outlives the column it grouped answers for rows that moved"
    );
    assert_eq!(library.at(alice).unwrap().chapters().unwrap().count(), 0);
    assert_eq!(library.at(kokoro).unwrap().chapters().unwrap().count(), 1);
}

fn shelved(library: &mut Library, book: Id<Book>) -> Id<Edition> {
    library
        .editions
        .push(book, Some("0-486-41658-5"), 1865, Some(192), Binding::Cloth)
        .unwrap()
}

#[test]
fn a_handle_reads_every_kind_of_field_the_table_beneath_it_holds() {
    let (mut library, on) = stocked();
    let alice = on.alice;
    let bound = shelved(&mut library, alice);
    let plain = library
        .editions
        .push(alice, None, 1946, None, Binding::Paper)
        .unwrap();

    let edition = library.at(bound).unwrap();
    assert_eq!(edition.isbn().unwrap().as_deref(), Some("0-486-41658-5"));
    assert_eq!(edition.year().unwrap(), 1865);
    assert_eq!(edition.pages().unwrap(), Some(192));
    assert_eq!(edition.binding().unwrap(), Binding::Cloth);
    assert_eq!(edition.book().unwrap().title().unwrap(), "Alice");

    let edition = library.at(plain).unwrap();
    assert_eq!(edition.isbn().unwrap(), None);
    assert_eq!(edition.pages().unwrap(), None);
    assert_eq!(edition.binding().unwrap(), Binding::Paper);
}

#[test]
fn a_handle_hands_back_what_the_table_accessor_hands_back_for_the_same_id() {
    let (mut library, on) = stocked();
    let bound = shelved(&mut library, on.kokoro);

    let edition = library.at(bound).unwrap();
    assert_eq!(
        edition.book().unwrap().id().unwrap(),
        library.editions.book(bound).unwrap()
    );
    assert_eq!(
        edition.isbn().unwrap(),
        library.editions.isbn(bound).unwrap()
    );
    assert_eq!(
        edition.year().unwrap(),
        library.editions.year(bound).unwrap()
    );
    assert_eq!(
        edition.pages().unwrap(),
        library.editions.pages(bound).unwrap()
    );
    assert_eq!(
        edition.binding().unwrap(),
        library.editions.binding(bound).unwrap()
    );
}

#[test]
fn a_mutable_handle_writes_every_kind_of_field_the_table_beneath_it_holds() {
    let (mut library, on) = stocked();
    let alice = on.alice;
    let kokoro = on.kokoro;
    let bound = shelved(&mut library, alice);
    let untouched = shelved(&mut library, alice);

    let mut edition = library.at_mut(bound).unwrap();
    edition.set_book(kokoro).unwrap();
    edition.set_isbn(None).unwrap();
    edition.set_year(1946).unwrap();
    edition.set_pages(None).unwrap();
    edition.set_binding(Binding::Leather).unwrap();

    let edition = library.at(bound).unwrap();
    assert_eq!(edition.book().unwrap().id().unwrap(), kokoro);
    assert_eq!(edition.isbn().unwrap(), None);
    assert_eq!(edition.year().unwrap(), 1946);
    assert_eq!(edition.pages().unwrap(), None);
    assert_eq!(edition.binding().unwrap(), Binding::Leather);

    let edition = library.at(untouched).unwrap();
    assert_eq!(edition.isbn().unwrap().as_deref(), Some("0-486-41658-5"));
    assert_eq!(edition.year().unwrap(), 1865);
    assert_eq!(edition.pages().unwrap(), Some(192));
    assert_eq!(edition.binding().unwrap(), Binding::Cloth);
}

#[test]
fn a_parent_of_two_kinds_of_child_walks_into_each_of_them_separately() {
    let (mut library, on) = stocked();
    let alice = on.alice;
    shelved(&mut library, alice);
    shelved(&mut library, alice);
    shelved(&mut library, on.kokoro);

    let book = library.at(alice).unwrap();

    assert_eq!(book.chapters().unwrap().count(), 1);
    assert_eq!(book.editions().unwrap().count(), 2);
    assert_eq!(
        library.at(on.kokoro).unwrap().editions().unwrap().count(),
        1
    );
}

struct Printing {
    book: usize,
    isbn: Option<String>,
    year: u32,
    pages: Option<u32>,
    binding: Binding,
}

#[derive(Default)]
struct Model {
    books: Vec<(String, String)>,
    chapters: Vec<(usize, String)>,
    paragraphs: Vec<(usize, String)>,
    printings: Vec<Printing>,
}

#[derive(Debug, Clone)]
enum Op {
    Book(String, String),
    Chapter(usize, String),
    Paragraph(usize, String),
    Print(usize, Option<String>, u32, Option<u32>, Binding),
    Retitle(usize, String),
    Rebook(usize, usize),
    Retext(usize, String),
    Reprint(usize, Option<String>, u32, Option<u32>, Binding),
}

fn binding() -> impl Strategy<Value = Binding> {
    prop_oneof![
        Just(Binding::Cloth),
        Just(Binding::Paper),
        Just(Binding::Leather),
    ]
}

fn printed() -> impl Strategy<Value = (usize, Option<String>, u32, Option<u32>, Binding)> {
    (
        props::row_index(),
        props::optional_text(),
        props::interesting_u32(),
        proptest::option::of(props::interesting_u32()),
        binding(),
    )
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (props::text(), props::text()).prop_map(|(title, author)| Op::Book(title, author)),
        4 => (props::row_index(), props::text()).prop_map(|(at, title)| Op::Chapter(at, title)),
        4 => (props::row_index(), props::text()).prop_map(|(at, text)| Op::Paragraph(at, text)),
        4 => printed().prop_map(|(at, isbn, year, pages, bound)| Op::Print(at, isbn, year, pages, bound)),
        2 => (props::row_index(), props::text()).prop_map(|(at, title)| Op::Retitle(at, title)),
        2 => (props::row_index(), props::row_index()).prop_map(|(at, to)| Op::Rebook(at, to)),
        2 => (props::row_index(), props::text()).prop_map(|(at, text)| Op::Retext(at, text)),
        2 => printed().prop_map(|(at, isbn, year, pages, bound)| Op::Reprint(at, isbn, year, pages, bound)),
    ]
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(op(), 0..24)
}

fn wrapped(at: usize, rows: usize) -> Option<usize> {
    (rows > 0).then(|| at % rows)
}

#[derive(Default)]
struct Minted {
    books: Vec<Id<Book>>,
    chapters: Vec<Id<Chapter>>,
    paragraphs: Vec<Id<Paragraph>>,
    printings: Vec<Id<Edition>>,
}

fn at<T>(minted: &[Id<T>], id: Id<T>) -> usize {
    minted
        .iter()
        .position(|held| *held == id)
        .expect("every id a row hands back was minted into this list")
}

fn played(ops: &[Op]) -> (Library, Model, Minted) {
    let mut library = Library::default();
    let mut model = Model::default();
    let mut minted = Minted::default();

    for op in ops {
        match op {
            Op::Book(title, author) => {
                minted
                    .books
                    .push(library.books.push(title, author).unwrap());
                model.books.push((title.clone(), author.clone()));
            }
            Op::Chapter(at, title) => {
                if let Some(book) = wrapped(*at, model.books.len()) {
                    minted
                        .chapters
                        .push(library.chapters.push(minted.books[book], title).unwrap());
                    model.chapters.push((book, title.clone()));
                }
            }
            Op::Paragraph(at, text) => {
                if let Some(chapter) = wrapped(*at, model.chapters.len()) {
                    let pushed = library
                        .paragraphs
                        .push(minted.chapters[chapter], text)
                        .unwrap();
                    minted.paragraphs.push(pushed);
                    model.paragraphs.push((chapter, text.clone()));
                }
            }
            Op::Print(at, isbn, year, pages, bound) => {
                if let Some(book) = wrapped(*at, model.books.len()) {
                    let pushed = library
                        .editions
                        .push(minted.books[book], isbn.as_deref(), *year, *pages, *bound)
                        .unwrap();
                    minted.printings.push(pushed);
                    model.printings.push(Printing {
                        book,
                        isbn: isbn.clone(),
                        year: *year,
                        pages: *pages,
                        binding: *bound,
                    });
                }
            }
            Op::Retitle(at, title) => {
                if let Some(book) = wrapped(*at, model.books.len()) {
                    library
                        .at_mut(minted.books[book])
                        .unwrap()
                        .set_title(title)
                        .unwrap();
                    model.books[book].0 = title.clone();
                }
            }
            Op::Rebook(at, to) => {
                let moved = wrapped(*at, model.chapters.len());
                let home = wrapped(*to, model.books.len());
                if let (Some(chapter), Some(book)) = (moved, home) {
                    library
                        .at_mut(minted.chapters[chapter])
                        .unwrap()
                        .set_book(minted.books[book])
                        .unwrap();
                    model.chapters[chapter].0 = book;
                }
            }
            Op::Retext(at, text) => {
                if let Some(paragraph) = wrapped(*at, model.paragraphs.len()) {
                    library
                        .at_mut(minted.paragraphs[paragraph])
                        .unwrap()
                        .set_text(text)
                        .unwrap();
                    model.paragraphs[paragraph].1 = text.clone();
                }
            }
            Op::Reprint(at, isbn, year, pages, bound) => {
                if let Some(printing) = wrapped(*at, model.printings.len()) {
                    let mut row = library.at_mut(minted.printings[printing]).unwrap();
                    row.set_isbn(isbn.as_deref()).unwrap();
                    row.set_year(*year).unwrap();
                    row.set_pages(*pages).unwrap();
                    row.set_binding(*bound).unwrap();
                    model.printings[printing].isbn = isbn.clone();
                    model.printings[printing].year = *year;
                    model.printings[printing].pages = *pages;
                    model.printings[printing].binding = *bound;
                }
            }
        }
    }
    (library, model, minted)
}

fn counted(rows: usize) -> Vec<usize> {
    (0..rows).collect()
}

fn printings_by_table(library: &Library, minted: &Minted) -> Vec<Printing> {
    minted
        .printings
        .iter()
        .map(|of| Printing {
            book: at(&minted.books, library.editions.book(*of).unwrap()),
            isbn: library
                .editions
                .isbn(*of)
                .unwrap()
                .map(|isbn| isbn.into_owned()),
            year: library.editions.year(*of).unwrap(),
            pages: library.editions.pages(*of).unwrap(),
            binding: library.editions.binding(*of).unwrap(),
        })
        .collect()
}

fn printings_by_row(library: &Library, minted: &Minted) -> Vec<Printing> {
    library
        .all::<Edition>()
        .map(|row| Printing {
            book: at(&minted.books, row.book().unwrap().id().unwrap()),
            isbn: row.isbn().unwrap().map(|isbn| isbn.into_owned()),
            year: row.year().unwrap(),
            pages: row.pages().unwrap(),
            binding: row.binding().unwrap(),
        })
        .collect()
}

proptest! {
    #[test]
    fn every_field_a_row_hands_back_is_the_one_its_table_stores(ops in ops()) {
        let (library, model, minted) = played(&ops);

        for (row, (title, author)) in model.books.iter().enumerate() {
            let of = minted.books[row];
            let book = library.at(of).unwrap();
            prop_assert_eq!(book.title().unwrap().into_owned(), title.clone());
            prop_assert_eq!(book.author().unwrap().into_owned(), author.clone());
            prop_assert_eq!(book.title().unwrap().into_owned(), library.books.title(of).unwrap());
        }

        for (row, (book, title)) in model.chapters.iter().enumerate() {
            let of = minted.chapters[row];
            let chapter = library.at(of).unwrap();
            prop_assert_eq!(chapter.title().unwrap().into_owned(), title.clone());
            prop_assert_eq!(at(&minted.books, chapter.book().unwrap().id().unwrap()), *book);
            prop_assert_eq!(chapter.book().unwrap().id().unwrap(), library.chapters.book(of).unwrap());
        }

        for (row, (chapter, text)) in model.paragraphs.iter().enumerate() {
            let of = minted.paragraphs[row];
            let paragraph = library.at(of).unwrap();
            prop_assert_eq!(paragraph.text().unwrap().into_owned(), text.clone());
            prop_assert_eq!(at(&minted.chapters, paragraph.chapter().unwrap().id().unwrap()), *chapter);
        }

        for (row, printing) in model.printings.iter().enumerate() {
            let edition = library.at(minted.printings[row]).unwrap();
            prop_assert_eq!(at(&minted.books, edition.book().unwrap().id().unwrap()), printing.book);
            prop_assert_eq!(
                edition.isbn().unwrap().map(|isbn| isbn.into_owned()),
                printing.isbn.clone()
            );
            prop_assert_eq!(edition.year().unwrap(), printing.year);
            prop_assert_eq!(edition.pages().unwrap(), printing.pages);
            prop_assert_eq!(edition.binding().unwrap(), printing.binding);
        }
    }

    #[test]
    fn walking_a_table_hands_back_every_row_once_in_the_order_they_were_pushed(ops in ops()) {
        let (library, model, minted) = played(&ops);

        let books: Vec<Id<Book>> = library.all::<Book>().map(|row| row.id().unwrap()).collect();
        let chapters: Vec<Id<Chapter>> =
            library.all::<Chapter>().map(|row| row.id().unwrap()).collect();
        let paragraphs: Vec<Id<Paragraph>> = library
            .all::<Paragraph>()
            .map(|row| row.id().unwrap())
            .collect();
        let printings: Vec<Id<Edition>> =
            library.all::<Edition>().map(|row| row.id().unwrap()).collect();

        prop_assert_eq!(books, minted.books.clone());
        prop_assert_eq!(chapters, minted.chapters.clone());
        prop_assert_eq!(paragraphs, minted.paragraphs.clone());
        prop_assert_eq!(printings, minted.printings.clone());
        prop_assert_eq!(library.all::<Book>().count(), model.books.len());
    }

    #[test]
    fn a_row_answers_for_the_id_it_was_minted_with_and_for_no_other(
        ops in ops(),
        at_row in props::row_index(),
    ) {
        let (library, model, minted) = played(&ops);

        if let Some(row) = wrapped(at_row, model.chapters.len()) {
            let chapter = library.at(minted.chapters[row]).unwrap();
            prop_assert_eq!(chapter.id().unwrap(), minted.chapters[row]);
            prop_assert_eq!(chapter.row().index(), row);
        }

        let stranger = Id::<Chapter>::mint();
        let said = refused(library.at(stranger));
        prop_assert!(said.contains("chapter has no row with id"), "{}", said);
        prop_assert!(said.contains(&stranger.to_string()), "{}", said);
        prop_assert!(!said.contains("book has no row"), "{}", said);
    }

    #[test]
    fn every_child_is_found_under_exactly_one_parent(ops in ops()) {
        let (library, model, minted) = played(&ops);

        let mut chapters = Vec::new();
        let mut printings = Vec::new();
        for book in library.all::<Book>() {
            let owner = at(&minted.books, book.id().unwrap());

            let found: Vec<usize> = book
                .chapters()
                .unwrap()
                .map(|row| at(&minted.chapters, row.id().unwrap()))
                .collect();
            let scanned: Vec<usize> = model
                .chapters
                .iter()
                .enumerate()
                .filter(|(_, (held, _))| *held == owner)
                .map(|(row, _)| row)
                .collect();
            prop_assert_eq!(&found, &scanned);
            chapters.extend(found);

            let found: Vec<usize> = book
                .editions()
                .unwrap()
                .map(|row| at(&minted.printings, row.id().unwrap()))
                .collect();
            let scanned: Vec<usize> = model
                .printings
                .iter()
                .enumerate()
                .filter(|(_, printing)| printing.book == owner)
                .map(|(row, _)| row)
                .collect();
            prop_assert_eq!(&found, &scanned);
            printings.extend(found);
        }

        let mut paragraphs = Vec::new();
        for chapter in library.all::<Chapter>() {
            let owner = at(&minted.chapters, chapter.id().unwrap());
            let found: Vec<usize> = chapter
                .paragraphs()
                .unwrap()
                .map(|row| at(&minted.paragraphs, row.id().unwrap()))
                .collect();
            let scanned: Vec<usize> = model
                .paragraphs
                .iter()
                .enumerate()
                .filter(|(_, (held, _))| *held == owner)
                .map(|(row, _)| row)
                .collect();
            prop_assert_eq!(&found, &scanned);
            paragraphs.extend(found);
        }

        chapters.sort_unstable();
        paragraphs.sort_unstable();
        printings.sort_unstable();
        prop_assert_eq!(chapters, counted(model.chapters.len()));
        prop_assert_eq!(paragraphs, counted(model.paragraphs.len()));
        prop_assert_eq!(printings, counted(model.printings.len()));
    }

    #[test]
    fn a_child_walked_down_to_names_the_parent_it_was_walked_from(ops in ops()) {
        let (library, _, _) = played(&ops);

        for book in library.all::<Book>() {
            for chapter in book.chapters().unwrap() {
                prop_assert_eq!(chapter.book().unwrap().id().unwrap(), book.id().unwrap());
                for paragraph in chapter.paragraphs().unwrap() {
                    prop_assert_eq!(
                        paragraph.chapter().unwrap().id().unwrap(),
                        chapter.id().unwrap()
                    );
                }
            }
            for edition in book.editions().unwrap() {
                prop_assert_eq!(edition.book().unwrap().id().unwrap(), book.id().unwrap());
            }
        }
    }

    #[test]
    fn a_field_written_through_a_row_is_the_only_field_that_changes(
        ops in ops(),
        at_row in props::row_index(),
        isbn in props::optional_text(),
    ) {
        let (mut library, _, mut minted) = played(&ops);
        let anchor = library.books.push("Anchor", "Nobody").unwrap();
        minted.books.push(anchor);
        minted.printings.push(
            library
                .editions
                .push(anchor, Some("anchor"), 1, Some(1), Binding::Cloth)
                .unwrap(),
        );
        let before = printings_by_table(&library, &minted);
        let row = at_row % before.len();

        library
            .at_mut(minted.printings[row])
            .unwrap()
            .set_isbn(isbn.as_deref())
            .unwrap();

        let after = printings_by_row(&library, &minted);
        prop_assert_eq!(after.len(), before.len());
        for (index, (was, now)) in before.iter().zip(&after).enumerate() {
            prop_assert_eq!(now.book, was.book);
            prop_assert_eq!(now.year, was.year);
            prop_assert_eq!(now.pages, was.pages);
            prop_assert_eq!(now.binding, was.binding);
            let want = if index == row { isbn.clone() } else { was.isbn.clone() };
            prop_assert_eq!(now.isbn.clone(), want);
        }
    }

    #[test]
    fn an_id_no_row_answers_for_is_refused_and_never_panics(
        ops in ops(),
        claim in props::uuid(),
    ) {
        let (mut library, model, minted) = played(&ops);
        let claimed = Id::<Book>::from_uuid(claim);

        match library.at(claimed) {
            Ok(book) => {
                prop_assert!(minted.books.contains(&claimed));
                prop_assert!(book.chapters().is_ok());
                prop_assert!(book.editions().is_ok());
            }
            Err(err) => {
                prop_assert!(!minted.books.contains(&claimed));
                prop_assert!(err.to_string().contains("book has no row with id"), "{}", err);
            }
        }

        library
            .chapters
            .push(claimed, "Names whatever that id names")
            .unwrap();
        for book in library.all::<Book>() {
            prop_assert!(book.chapters().is_ok() || book.chapters().is_err());
        }
        prop_assert_eq!(model.books.len(), minted.books.len());
    }
}
