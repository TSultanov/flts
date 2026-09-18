use super::super::store::Store;
use super::*;
use crate::test_utils::library::{Author, Authorship, Book, Chapter, Library, Paragraph};
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

    let chapters: Vec<Id<Chapter>> = children_of(&library, alice).unwrap();

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

    let chapters: Vec<Id<Chapter>> = children_of(&library, kokoro).unwrap();
    let paragraphs: Vec<Id<Paragraph>> = children_of(&library, chapters[0]).unwrap();

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

    let chapters: Vec<Id<Chapter>> = children_of(&library, lonely).unwrap();

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

    let chapters: Vec<Id<Chapter>> = children_of(&read, alice).unwrap();
    assert_eq!(chapters.len(), 2);
}

fn credited() -> (Library, Id<Book>, Id<Book>, Id<Author>, Id<Author>) {
    let mut library = Library::default();
    let atlas = library.books.push("An Atlas", "many hands").unwrap();
    let primer = library.books.push("A Primer", "many hands").unwrap();
    let ada = library.authors.push("Ada").unwrap();
    let bram = library.authors.push("Bram").unwrap();

    library.authorships.push(atlas, ada).unwrap();
    library.authorships.push(atlas, bram).unwrap();
    library.authorships.push(primer, ada).unwrap();
    (library, atlas, primer, ada, bram)
}

#[test]
fn a_table_that_names_two_parents_answers_to_each_of_them() {
    let (library, atlas, primer, ada, bram) = credited();

    let of_atlas: Vec<Id<Authorship>> = children_of(&library, atlas).unwrap();
    let of_ada: Vec<Id<Authorship>> = children_of(&library, ada).unwrap();

    assert_eq!(of_atlas.len(), 2, "a book two people wrote has two links");
    assert_eq!(of_ada.len(), 2, "and a person who wrote two books has two");
    assert_eq!(library.authorships.book(of_ada[1]).unwrap(), primer);
    assert_eq!(library.authorships.author(of_atlas[1]).unwrap(), bram);
    assert_eq!(library.authorships.of_book(atlas).unwrap(), of_atlas);
    assert_eq!(library.authorships.of_author(ada).unwrap(), of_ada);
}

#[test]
fn a_join_is_walked_from_either_end_without_either_table_holding_a_column_for_it() {
    let (library, atlas, _, _, _) = credited();
    let ada = library.authors.find_by_name("Ada").unwrap()[0];

    assert_eq!(
        library
            .at(atlas)
            .unwrap()
            .authorships()
            .unwrap()
            .map(|link| link.author().unwrap().name().unwrap().into_owned())
            .collect::<Vec<_>>(),
        ["Ada", "Bram"]
    );
    assert_eq!(
        library
            .at(ada)
            .unwrap()
            .authorships()
            .unwrap()
            .map(|link| link.book().unwrap().title().unwrap().into_owned())
            .collect::<Vec<_>>(),
        ["An Atlas", "A Primer"]
    );
}

#[test]
fn an_audit_holds_a_join_to_both_of_the_tables_it_names() {
    let (library, atlas, _, ada, _) = credited();
    Store::audit(&library).unwrap();

    let mut lost_book = library.clone();
    lost_book.authorships.push(Id::mint(), ada).unwrap();
    let err = Store::audit(&lost_book).unwrap_err();
    assert!(err.to_string().contains("authorship.book row 3"), "{err}");

    let mut lost_author = library.clone();
    lost_author.authorships.push(atlas, Id::mint()).unwrap();
    let err = Store::audit(&lost_author).unwrap_err();
    assert!(err.to_string().contains("authorship.author row 3"), "{err}");
}

#[test]
fn a_link_moved_to_another_pair_is_gone_from_the_two_rows_it_used_to_name() {
    let (mut library, atlas, primer, ada, bram) = credited();
    let first = library.authorships.of_book(atlas).unwrap()[0];

    library.at_mut(first).unwrap().set_book(primer).unwrap();
    library.at_mut(first).unwrap().set_author(bram).unwrap();

    assert_eq!(library.authorships.of_book(atlas).unwrap().len(), 1);
    assert_eq!(library.authorships.of_author(ada).unwrap().len(), 1);
    assert_eq!(library.authorships.of_book(primer).unwrap().len(), 2);
    assert_eq!(library.authorships.of_author(bram).unwrap().len(), 2);
    assert_eq!(library.authors.name(bram).unwrap(), "Bram");
    Store::audit(&library).unwrap();
}
