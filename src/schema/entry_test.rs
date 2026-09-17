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

fn stocked() -> Library {
    let mut library = Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    library.books.push("Kokoro", "Natsume Soseki").unwrap();

    let rabbit = library
        .chapters
        .push(alice, "Down the Rabbit-Hole")
        .unwrap();
    library.paragraphs.push(rabbit, "Alice was tired.").unwrap();
    library
}

#[test]
fn a_handle_reads_the_fields_of_the_row_it_was_minted_for() {
    let library = stocked();

    let kokoro = library.at(Id::<Book>::from_index(1)).unwrap();

    assert_eq!(kokoro.title().unwrap(), "Kokoro");
    assert_eq!(kokoro.author().unwrap(), "Natsume Soseki");
}

#[test]
fn a_handle_answers_with_the_id_it_was_minted_from() {
    let library = stocked();
    let asked = Id::<Book>::from_index(1);

    assert_eq!(library.at(asked).unwrap().id(), asked);
}

#[test]
fn a_handle_is_refused_for_an_id_past_the_last_row() {
    let library = stocked();

    let err = refused(library.at(Id::<Book>::from_index(2)));

    assert!(err.contains("book has no row 2"), "{err}");
    assert!(err.contains("it holds 2"), "{err}");
}

#[test]
fn a_handle_is_refused_by_the_table_that_holds_the_entity_not_by_another() {
    let library = stocked();

    let err = refused(library.at(Id::<Paragraph>::from_index(1)));

    assert!(err.contains("paragraph has no row 1"), "{err}");
}

#[test]
fn a_handle_reaches_the_store_and_the_table_it_was_minted_from() {
    let library = stocked();

    let alice = library.at(Id::<Book>::from_index(0)).unwrap();

    assert_eq!(Table::rows(alice.table()), 2);
    assert_eq!(Table::rows(&alice.store().paragraphs), 1);
    assert_eq!(alice.row().index(), 0);
}

#[test]
fn two_handles_into_the_same_store_are_alive_at_once() {
    let library = stocked();

    let alice = library.at(Id::<Book>::from_index(0)).unwrap();
    let kokoro = library.at(Id::<Book>::from_index(1)).unwrap();

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
    let library = stocked();

    let titles: Vec<String> = library
        .all::<Book>()
        .map(|book| book.title().unwrap().into_owned())
        .collect();

    assert_eq!(titles, ["Alice", "Kokoro"]);
}

#[test]
fn iterating_a_table_of_one_row_yields_that_row_and_stops() {
    let library = stocked();

    let mut paragraphs = library.all::<Paragraph>();

    assert_eq!(
        paragraphs.next().unwrap().text().unwrap(),
        "Alice was tired."
    );
    assert!(paragraphs.next().is_none());
}

#[test]
fn an_iterated_handle_carries_the_id_of_the_row_it_stands_for() {
    let library = stocked();

    let ids: Vec<Id<Book>> = library.all::<Book>().map(|book| book.id()).collect();

    assert_eq!(ids, [Id::from_index(0), Id::from_index(1)]);
}

#[test]
fn every_table_in_a_store_mints_handles_of_its_own_entity() {
    let library = stocked();

    let chapters: Vec<String> = library
        .all::<Chapter>()
        .map(|chapter| chapter.title().unwrap().into_owned())
        .collect();

    assert_eq!(chapters, ["Down the Rabbit-Hole"]);
    assert_eq!(library.all::<Book>().count(), 2);
}

#[test]
fn a_handle_follows_a_reference_into_the_row_it_points_at() {
    let library = stocked();

    let chapter = library.at(Id::<Chapter>::from_index(0)).unwrap();

    assert_eq!(chapter.book().unwrap().title().unwrap(), "Alice");
    assert_eq!(chapter.book().unwrap().id(), Id::<Book>::from_index(0));
}

#[test]
fn a_handle_walks_two_levels_down_and_one_back_up() {
    let library = stocked();

    let alice = library.at(Id::<Book>::from_index(0)).unwrap();
    let chapter = alice.chapters().unwrap().next().unwrap();
    let paragraph = chapter.paragraphs().unwrap().next().unwrap();

    assert_eq!(paragraph.text().unwrap(), "Alice was tired.");
    assert_eq!(paragraph.chapter().unwrap().id(), chapter.id());
}

#[test]
fn a_handle_whose_row_has_no_children_walks_nowhere() {
    let library = stocked();

    let kokoro = library.at(Id::<Book>::from_index(1)).unwrap();

    assert_eq!(kokoro.chapters().unwrap().count(), 0);
}

#[test]
fn walking_the_children_of_a_row_hands_back_only_that_row_s_children() {
    let mut library = stocked();
    let kokoro = Id::<Book>::from_index(1);
    library.chapters.push(kokoro, "Sensei and I").unwrap();
    library
        .chapters
        .push(Id::from_index(0), "The Pool of Tears")
        .unwrap();

    let titles: Vec<String> = library
        .at(kokoro)
        .unwrap()
        .chapters()
        .unwrap()
        .map(|chapter| chapter.title().unwrap().into_owned())
        .collect();

    assert_eq!(titles, ["Sensei and I"]);
    assert_eq!(
        library
            .at(Id::<Book>::from_index(0))
            .unwrap()
            .chapters()
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn walking_the_children_of_a_row_refuses_a_reference_no_parent_answers_for() {
    let mut library = Library::default();
    library.books.push("Alice", "Lewis Carroll").unwrap();
    library
        .chapters
        .push(Id::from_index(7), "Belongs to nobody")
        .unwrap();

    let err = refused(library.at(Id::<Book>::from_index(0)).unwrap().chapters());

    assert!(err.contains("chapter.book"), "{err}");
}

#[test]
fn a_mutable_handle_writes_the_field_it_names_and_leaves_its_neighbours_alone() {
    let mut library = stocked();
    let alice = Id::<Book>::from_index(0);

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
    assert_eq!(
        library
            .at(Id::<Book>::from_index(1))
            .unwrap()
            .title()
            .unwrap(),
        "Kokoro"
    );
}

#[test]
fn a_mutable_handle_answers_with_the_id_it_was_minted_from() {
    let mut library = stocked();
    let asked = Id::<Paragraph>::from_index(0);

    let mut paragraph = library.at_mut(asked).unwrap();
    paragraph.set_text("She was very tired indeed.").unwrap();

    assert_eq!(paragraph.id(), asked);
    assert_eq!(paragraph.row().index(), 0);
    assert_eq!(
        library.at(asked).unwrap().text().unwrap(),
        "She was very tired indeed."
    );
}

#[test]
fn a_mutable_handle_is_refused_for_an_id_past_the_last_row() {
    let mut library = stocked();

    let err = refused(library.at_mut(Id::<Book>::from_index(2)));

    assert!(err.contains("book has no row 2"), "{err}");
}

#[test]
fn moving_a_child_through_a_handle_forgets_the_grouping_it_belonged_to() {
    let mut library = stocked();
    let alice = Id::<Book>::from_index(0);
    let kokoro = Id::<Book>::from_index(1);
    assert_eq!(library.at(alice).unwrap().chapters().unwrap().count(), 1);
    assert!(library.chapters.is_indexed());

    library
        .at_mut(Id::<Chapter>::from_index(0))
        .unwrap()
        .set_book(kokoro)
        .unwrap();

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
    let mut library = stocked();
    let alice = Id::<Book>::from_index(0);
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
    let mut library = stocked();
    let bound = shelved(&mut library, Id::<Book>::from_index(1));

    let edition = library.at(bound).unwrap();
    assert_eq!(
        edition.book().unwrap().id(),
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
    let mut library = stocked();
    let alice = Id::<Book>::from_index(0);
    let kokoro = Id::<Book>::from_index(1);
    let bound = shelved(&mut library, alice);
    let untouched = shelved(&mut library, alice);

    let mut edition = library.at_mut(bound).unwrap();
    edition.set_book(kokoro).unwrap();
    edition.set_isbn(None).unwrap();
    edition.set_year(1946).unwrap();
    edition.set_pages(None).unwrap();
    edition.set_binding(Binding::Leather).unwrap();

    let edition = library.at(bound).unwrap();
    assert_eq!(edition.book().unwrap().id(), kokoro);
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
    let mut library = stocked();
    let alice = Id::<Book>::from_index(0);
    shelved(&mut library, alice);
    shelved(&mut library, alice);
    shelved(&mut library, Id::<Book>::from_index(1));

    let book = library.at(alice).unwrap();

    assert_eq!(book.chapters().unwrap().count(), 1);
    assert_eq!(book.editions().unwrap().count(), 2);
    assert_eq!(
        library
            .at(Id::<Book>::from_index(1))
            .unwrap()
            .editions()
            .unwrap()
            .count(),
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

fn played(ops: &[Op]) -> (Library, Model) {
    let mut library = Library::default();
    let mut model = Model::default();

    for op in ops {
        match op {
            Op::Book(title, author) => {
                library.books.push(title, author).unwrap();
                model.books.push((title.clone(), author.clone()));
            }
            Op::Chapter(at, title) => {
                if let Some(book) = wrapped(*at, model.books.len()) {
                    library.chapters.push(Id::from_index(book), title).unwrap();
                    model.chapters.push((book, title.clone()));
                }
            }
            Op::Paragraph(at, text) => {
                if let Some(chapter) = wrapped(*at, model.chapters.len()) {
                    library
                        .paragraphs
                        .push(Id::from_index(chapter), text)
                        .unwrap();
                    model.paragraphs.push((chapter, text.clone()));
                }
            }
            Op::Print(at, isbn, year, pages, bound) => {
                if let Some(book) = wrapped(*at, model.books.len()) {
                    library
                        .editions
                        .push(Id::from_index(book), isbn.as_deref(), *year, *pages, *bound)
                        .unwrap();
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
                        .at_mut(Id::<Book>::from_index(book))
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
                        .at_mut(Id::<Chapter>::from_index(chapter))
                        .unwrap()
                        .set_book(Id::from_index(book))
                        .unwrap();
                    model.chapters[chapter].0 = book;
                }
            }
            Op::Retext(at, text) => {
                if let Some(paragraph) = wrapped(*at, model.paragraphs.len()) {
                    library
                        .at_mut(Id::<Paragraph>::from_index(paragraph))
                        .unwrap()
                        .set_text(text)
                        .unwrap();
                    model.paragraphs[paragraph].1 = text.clone();
                }
            }
            Op::Reprint(at, isbn, year, pages, bound) => {
                if let Some(printing) = wrapped(*at, model.printings.len()) {
                    let mut row = library.at_mut(Id::<Edition>::from_index(printing)).unwrap();
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
    (library, model)
}

fn counted(rows: usize) -> Vec<usize> {
    (0..rows).collect()
}

fn printings_by_table(library: &Library) -> Vec<Printing> {
    (0..Table::rows(&library.editions))
        .map(|row| {
            let of = Id::<Edition>::from_index(row);
            Printing {
                book: library.editions.book(of).unwrap().index(),
                isbn: library
                    .editions
                    .isbn(of)
                    .unwrap()
                    .map(|isbn| isbn.into_owned()),
                year: library.editions.year(of).unwrap(),
                pages: library.editions.pages(of).unwrap(),
                binding: library.editions.binding(of).unwrap(),
            }
        })
        .collect()
}

fn printings_by_row(library: &Library) -> Vec<Printing> {
    library
        .all::<Edition>()
        .map(|row| Printing {
            book: row.book().unwrap().id().index(),
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
        let (library, model) = played(&ops);

        for (row, (title, author)) in model.books.iter().enumerate() {
            let of = Id::<Book>::from_index(row);
            let book = library.at(of).unwrap();
            prop_assert_eq!(book.title().unwrap().into_owned(), title.clone());
            prop_assert_eq!(book.author().unwrap().into_owned(), author.clone());
            prop_assert_eq!(book.title().unwrap().into_owned(), library.books.title(of).unwrap());
        }

        for (row, (book, title)) in model.chapters.iter().enumerate() {
            let of = Id::<Chapter>::from_index(row);
            let chapter = library.at(of).unwrap();
            prop_assert_eq!(chapter.title().unwrap().into_owned(), title.clone());
            prop_assert_eq!(chapter.book().unwrap().id().index(), *book);
            prop_assert_eq!(chapter.book().unwrap().id(), library.chapters.book(of).unwrap());
        }

        for (row, (chapter, text)) in model.paragraphs.iter().enumerate() {
            let of = Id::<Paragraph>::from_index(row);
            let paragraph = library.at(of).unwrap();
            prop_assert_eq!(paragraph.text().unwrap().into_owned(), text.clone());
            prop_assert_eq!(paragraph.chapter().unwrap().id().index(), *chapter);
        }

        for (row, printing) in model.printings.iter().enumerate() {
            let edition = library.at(Id::<Edition>::from_index(row)).unwrap();
            prop_assert_eq!(edition.book().unwrap().id().index(), printing.book);
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
        let (library, model) = played(&ops);

        let books: Vec<usize> = library.all::<Book>().map(|row| row.id().index()).collect();
        let chapters: Vec<usize> = library.all::<Chapter>().map(|row| row.id().index()).collect();
        let paragraphs: Vec<usize> = library
            .all::<Paragraph>()
            .map(|row| row.id().index())
            .collect();
        let printings: Vec<usize> = library.all::<Edition>().map(|row| row.id().index()).collect();

        prop_assert_eq!(books, counted(model.books.len()));
        prop_assert_eq!(chapters, counted(model.chapters.len()));
        prop_assert_eq!(paragraphs, counted(model.paragraphs.len()));
        prop_assert_eq!(printings, counted(model.printings.len()));
        prop_assert_eq!(library.all::<Book>().count(), model.books.len());
    }

    #[test]
    fn a_row_is_refused_exactly_when_it_is_past_the_last_row(
        ops in ops(),
        at in props::row_index(),
    ) {
        let (library, model) = played(&ops);

        match library.at(Id::<Chapter>::from_index(at)) {
            Ok(chapter) => {
                prop_assert!(at < model.chapters.len());
                prop_assert_eq!(chapter.id().index(), at);
            }
            Err(err) => {
                prop_assert!(at >= model.chapters.len());
                let said = err.to_string();
                prop_assert!(said.contains("chapter has no row"), "{}", said);
                prop_assert!(!said.contains("book has no row"), "{}", said);
            }
        }
    }

    #[test]
    fn every_child_is_found_under_exactly_one_parent(ops in ops()) {
        let (library, model) = played(&ops);

        let mut chapters = Vec::new();
        let mut printings = Vec::new();
        for book in library.all::<Book>() {
            let at = book.id().index();

            let found: Vec<usize> = book.chapters().unwrap().map(|row| row.id().index()).collect();
            let scanned: Vec<usize> = model
                .chapters
                .iter()
                .enumerate()
                .filter(|(_, (owner, _))| *owner == at)
                .map(|(row, _)| row)
                .collect();
            prop_assert_eq!(&found, &scanned);
            chapters.extend(found);

            let found: Vec<usize> = book.editions().unwrap().map(|row| row.id().index()).collect();
            let scanned: Vec<usize> = model
                .printings
                .iter()
                .enumerate()
                .filter(|(_, printing)| printing.book == at)
                .map(|(row, _)| row)
                .collect();
            prop_assert_eq!(&found, &scanned);
            printings.extend(found);
        }

        let mut paragraphs = Vec::new();
        for chapter in library.all::<Chapter>() {
            let at = chapter.id().index();
            let found: Vec<usize> = chapter
                .paragraphs()
                .unwrap()
                .map(|row| row.id().index())
                .collect();
            let scanned: Vec<usize> = model
                .paragraphs
                .iter()
                .enumerate()
                .filter(|(_, (owner, _))| *owner == at)
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
        let (library, _) = played(&ops);

        for book in library.all::<Book>() {
            for chapter in book.chapters().unwrap() {
                prop_assert_eq!(chapter.book().unwrap().id(), book.id());
                for paragraph in chapter.paragraphs().unwrap() {
                    prop_assert_eq!(paragraph.chapter().unwrap().id(), chapter.id());
                }
            }
            for edition in book.editions().unwrap() {
                prop_assert_eq!(edition.book().unwrap().id(), book.id());
            }
        }
    }

    #[test]
    fn a_field_written_through_a_row_is_the_only_field_that_changes(
        ops in ops(),
        at in props::row_index(),
        isbn in props::optional_text(),
    ) {
        let (mut library, _) = played(&ops);
        let anchor = library.books.push("Anchor", "Nobody").unwrap();
        library
            .editions
            .push(anchor, Some("anchor"), 1, Some(1), Binding::Cloth)
            .unwrap();
        let before = printings_by_table(&library);
        let row = at % before.len();

        library
            .at_mut(Id::<Edition>::from_index(row))
            .unwrap()
            .set_isbn(isbn.as_deref())
            .unwrap();

        let after = printings_by_row(&library);
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
        claim in props::interesting_u64(),
    ) {
        let (mut library, model) = played(&ops);
        let index = claim as usize;

        match library.at(Id::<Book>::from_index(index)) {
            Ok(book) => {
                prop_assert!(index < model.books.len());
                prop_assert!(book.chapters().is_ok());
                prop_assert!(book.editions().is_ok());
            }
            Err(err) => {
                prop_assert!(index >= model.books.len());
                prop_assert!(err.to_string().contains("book has no row"), "{}", err);
            }
        }

        let anchor = library.books.push("Anchor", "Nobody").unwrap();
        library
            .chapters
            .push(Id::from_index(index), "Belongs to whom?")
            .unwrap();

        match library.at(anchor).unwrap().chapters() {
            Ok(children) => {
                prop_assert!(
                    index == Id::<Book>::NULL as usize || index < Table::rows(&library.books)
                );
                prop_assert!(children.count() <= Table::rows(&library.chapters));
            }
            Err(err) => prop_assert!(err.to_string().contains("chapter.book"), "{}", err),
        }
    }
}
