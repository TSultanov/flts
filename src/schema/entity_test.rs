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
fn a_link_moved_to_another_pair_is_gone_from_the_row_it_used_to_name() {
    let (mut library, atlas, primer, ada, bram) = credited();
    let moved = library.authorships.of_book(atlas).unwrap()[1];

    library.at_mut(moved).unwrap().set_book(primer).unwrap();

    assert_eq!(library.authorships.of_book(atlas).unwrap().len(), 1);
    assert_eq!(library.authorships.of_book(primer).unwrap().len(), 2);
    assert_eq!(library.authorships.of_author(bram).unwrap(), [moved]);
    assert_eq!(library.authorships.of_author(ada).unwrap().len(), 2);
    assert_eq!(library.authors.name(bram).unwrap(), "Bram");
    Store::audit(&library).unwrap();
}

#[test]
fn a_join_refuses_a_pair_of_parents_it_already_holds_and_leaves_every_column_as_it_was() {
    let (mut library, atlas, _, ada, _) = credited();
    let before = Table::rows(&library.authorships);

    let err = library.authorships.push(atlas, ada).unwrap_err();

    assert!(err.to_string().contains("authorship.book_author"), "{err}");
    assert!(
        err.to_string().contains("no two rows may share one value"),
        "{err}"
    );
    assert_eq!(Table::rows(&library.authorships), before);
    Table::check_rows_agree(&library.authorships).unwrap();
    Store::audit(&library).unwrap();
}

#[test]
fn a_link_moved_onto_a_pair_another_link_holds_is_refused_and_moves_nothing() {
    let (mut library, atlas, primer, ada, _) = credited();
    let moved = library.authorships.of_book(atlas).unwrap()[0];

    let err = library.at_mut(moved).unwrap().set_book(primer).unwrap_err();

    assert!(err.to_string().contains("authorship.book_author"), "{err}");
    assert_eq!(library.authorships.book(moved).unwrap(), atlas);
    assert_eq!(library.authorships.author(moved).unwrap(), ada);
    assert_eq!(library.authorships.of_book(atlas).unwrap().len(), 2);
    assert_eq!(library.authorships.of_book(primer).unwrap().len(), 1);
    Store::audit(&library).unwrap();
}

#[test]
fn a_link_that_names_nobody_is_no_pair_at_all_however_many_of_them_there_are() {
    let (mut library, atlas, _, _, _) = credited();
    let links = library.authorships.of_book(atlas).unwrap();

    library
        .at_mut(links[0])
        .unwrap()
        .set_author(Id::NIL)
        .unwrap();
    library
        .at_mut(links[1])
        .unwrap()
        .set_author(Id::NIL)
        .unwrap();

    assert_eq!(
        library.authorships.of_book(atlas).unwrap().len(),
        2,
        "both are still rows of the join; neither names an author"
    );
    Store::audit(&library).unwrap();
}

#[test]
fn a_pair_a_link_gave_up_may_be_taken_by_another() {
    let (mut library, atlas, _, ada, _) = credited();
    let held = library.authorships.of_book(atlas).unwrap()[0];

    library.at_mut(held).unwrap().set_author(Id::NIL).unwrap();
    let again = library.authorships.push(atlas, ada).unwrap();

    assert_eq!(library.authorships.author(again).unwrap(), ada);
    assert_eq!(library.authorships.of_author(ada).unwrap().len(), 2);
    Store::audit(&library).unwrap();
}

#[test]
fn an_audit_refuses_a_pair_a_link_no_longer_agrees_with() {
    let (library, _, _, _, _) = credited();
    Store::audit(&library).unwrap();

    let mut drifted = library.clone();
    let stranger = drifted.authors.push("Nobody").unwrap();
    drifted
        .authorships
        .quietly_repoint_author(Row::trusted(0), stranger);

    let err = Store::audit(&drifted).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(
        err.to_string().contains("authorship.book_author row 0"),
        "{err}"
    );
}

#[test]
fn a_chapter_written_over_is_not_listed_under_its_book_twice() {
    let mut library = Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    let rabbit = library
        .chapters
        .push(alice, "Down the Rabbit-Hole")
        .unwrap();

    library
        .chapters
        .write_over(rabbit, alice, "Down the Rabbit Hole")
        .unwrap();

    assert_eq!(
        children_of::<Library, Book, Chapter>(&library, alice).unwrap(),
        vec![rabbit],
        "a book has one chapter, however many versions that chapter has had"
    );
    assert_eq!(Table::rows(&library.chapters), 2);
}

#[test]
fn a_chapter_moved_to_another_book_is_listed_under_the_new_one_alone() {
    let mut library = Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    let kokoro = library.books.push("Kokoro", "Natsume Soseki").unwrap();
    let rabbit = library
        .chapters
        .push(alice, "Down the Rabbit-Hole")
        .unwrap();

    library
        .chapters
        .write_over(rabbit, kokoro, "Down the Rabbit-Hole")
        .unwrap();

    assert!(
        children_of::<Library, Book, Chapter>(&library, alice)
            .unwrap()
            .is_empty(),
        "the version that named the old book is not the one the chapter is now"
    );
    assert_eq!(
        children_of::<Library, Book, Chapter>(&library, kokoro).unwrap(),
        vec![rabbit]
    );
}

#[test]
fn a_link_written_over_is_crossed_once_and_from_its_newest_end() {
    let mut library = Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    let carroll = library.authors.push("Lewis Carroll").unwrap();
    let tenniel = library.authors.push("John Tenniel").unwrap();
    let link = library.authorships.push(alice, carroll).unwrap();

    library
        .authorships
        .write_over(link, alice, tenniel)
        .unwrap();

    let reached: Vec<Id<Author>> =
        across_rows_of::<Library, Book, Authorship, Author>(&library, alice)
            .unwrap()
            .into_iter()
            .map(|row| Table::id_at(&library.authors, row).unwrap())
            .collect();

    assert_eq!(
        reached,
        vec![tenniel],
        "one link crosses once, and it crosses from the end it names now"
    );
}

#[test]
fn a_book_taken_out_is_gone_from_every_walk_and_every_finder() {
    let mut library = Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    let kokoro = library.books.push("Kokoro", "Natsume Soseki").unwrap();

    library.remove(alice).unwrap();

    assert_eq!(library.all::<Book>().count(), 1);
    assert_eq!(library.all::<Book>().next().unwrap().id().unwrap(), kokoro);
    assert!(library.books.find_by_title("Alice").unwrap().is_empty());
    assert!(library.at(alice).is_err());
    assert_eq!(
        Table::rows(&library.books),
        3,
        "the row that was taken out is still a row, and so is the one it replaced"
    );
    Store::audit(&library).unwrap();
}

#[test]
fn a_title_a_book_took_with_it_is_free_for_another_to_use() {
    let mut library = Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();

    library.remove(alice).unwrap();
    let again = library.books.push("Alice", "Somebody Else").unwrap();

    assert_eq!(library.books.find_by_title("Alice").unwrap(), vec![again]);
    Store::audit(&library).unwrap();
}

#[test]
fn a_book_with_a_chapter_still_in_it_cannot_be_taken_out() {
    let mut library = Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    library
        .chapters
        .push(alice, "Down the Rabbit-Hole")
        .unwrap();
    let before = Table::rows(&library.books);

    let err = library
        .remove(alice)
        .expect_err("a chapter of a book that is gone belongs to nobody");

    assert!(err.to_string().contains("cannot be taken out"), "{err}");
    assert!(err.to_string().contains("was taken out of book"), "{err}");
    assert_eq!(
        Table::rows(&library.books),
        before,
        "a refused removal leaves the table exactly as it was"
    );
    assert_eq!(library.all::<Book>().count(), 1);
    Store::audit(&library).unwrap();
}

#[test]
fn a_book_whose_chapters_went_first_can_be_taken_out_after_them() {
    let mut library = Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    let rabbit = library
        .chapters
        .push(alice, "Down the Rabbit-Hole")
        .unwrap();

    library.remove(rabbit).unwrap();
    library.remove(alice).unwrap();

    assert_eq!(library.all::<Book>().count(), 0);
    assert_eq!(library.all::<Chapter>().count(), 0);
    Store::audit(&library).unwrap();
}

#[test]
fn a_link_that_names_nobody_does_not_keep_a_book_from_being_taken_out() {
    let mut library = Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    let ada = library.authors.push("Ada").unwrap();
    let link = library.authorships.push(alice, ada).unwrap();
    library.at_mut(link).unwrap().set_book(Id::NIL).unwrap();

    library.remove(alice).unwrap();

    assert_eq!(library.all::<Book>().count(), 0);
    Store::audit(&library).unwrap();
}

#[test]
fn a_row_already_taken_out_cannot_be_taken_out_again() {
    let mut library = Library::default();
    let alice = library.books.push("Alice", "Lewis Carroll").unwrap();
    library.remove(alice).unwrap();

    let err = library.remove(alice).expect_err("it is already gone");

    assert!(err.to_string().contains("has no row with id"), "{err}");
}
