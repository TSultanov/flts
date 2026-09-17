use super::*;
use crate::test_utils::library::{Book, Chapter, Library, Paragraph};
use crate::test_utils::{TempDir, committed};

fn stocked() -> (Library, Id<Book>, Id<Book>) {
    let mut library = Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    let kokoro = library.books.push("Kokoro", "Natsume Soseki").unwrap();

    let rabbit = library
        .chapters
        .push(alice, "Down the Rabbit-Hole")
        .unwrap();
    let pool = library.chapters.push(alice, "The Pool of Tears").unwrap();
    let sensei = library.chapters.push(kokoro, "Sensei and I").unwrap();

    library.paragraphs.push(rabbit, "Alice was tired.").unwrap();
    library
        .paragraphs
        .push(sensei, "I always called him Sensei.")
        .unwrap();
    library
        .paragraphs
        .push(rabbit, "So she considered.")
        .unwrap();
    library
        .paragraphs
        .push(pool, "Curiouser and curiouser!")
        .unwrap();
    (library, alice, kokoro)
}

#[test]
fn a_parent_finds_its_children_through_the_store_that_holds_both_tables() {
    let (library, alice, _) = stocked();

    let chapters: &[Id<Chapter>] = children_of(&library, alice).unwrap();

    assert_eq!(chapters.len(), 2);
    assert_eq!(
        library.chapters.title(chapters[0]).unwrap(),
        "Down the Rabbit-Hole"
    );
    assert_eq!(
        library.chapters.title(chapters[1]).unwrap(),
        "The Pool of Tears"
    );
}

#[test]
fn the_join_goes_two_levels_down_without_the_levels_knowing_about_each_other() {
    let (library, _, kokoro) = stocked();

    let chapters: &[Id<Chapter>] = children_of(&library, kokoro).unwrap();
    let paragraphs: &[Id<Paragraph>] = children_of(&library, chapters[0]).unwrap();

    assert_eq!(paragraphs.len(), 1);
    assert_eq!(
        library.paragraphs.text(paragraphs[0]).unwrap(),
        "I always called him Sensei."
    );
}

#[test]
fn a_parent_with_no_children_gets_an_empty_answer_not_an_error() {
    let mut library = Library::default();
    let lonely = library.books.push("Unread", "Nobody").unwrap();

    let chapters: &[Id<Chapter>] = children_of(&library, lonely).unwrap();

    assert!(chapters.is_empty());
}

#[test]
fn the_store_reaches_each_table_it_holds_by_the_entity_that_names_it() {
    let (mut library, _, _) = stocked();

    assert_eq!(Table::rows(Holds::<Book>::table(&library)), 2);
    assert_eq!(Table::rows(Holds::<Chapter>::table(&library)), 3);

    Holds::<Book>::table_mut(&mut library)
        .push("Added through the trait", "Someone")
        .unwrap();
    assert_eq!(Table::rows(Holds::<Book>::table(&library)), 3);
}

#[test]
fn children_found_after_a_reopen_are_the_ones_that_were_written() {
    let dir = TempDir::new("join_roundtrip");
    let path = dir.join("library.dbs");

    let (library, alice, _) = stocked();
    let file = committed(&path, |updater| {
        library.books.offer_to(updater)?;
        library.chapters.offer_to(updater)?;
        library.paragraphs.offer_to(updater)
    });

    let mut read = Library::default();
    read.books.read_from(&file).unwrap();
    read.chapters.read_from(&file).unwrap();
    read.paragraphs.read_from(&file).unwrap();

    let chapters: &[Id<Chapter>] = children_of(&read, alice).unwrap();
    assert_eq!(chapters.len(), 2);
}
