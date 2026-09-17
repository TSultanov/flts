use super::*;
use crate::error::Result;
use crate::id::Id;
use crate::test_utils::library::{Binding, Book, Chapter, Edition, Library, Paragraph};

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
