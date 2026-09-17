use super::*;
use crate::error::Result;
use crate::id::Id;
use crate::test_utils::library::{Book, Chapter, Library, Paragraph};

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
