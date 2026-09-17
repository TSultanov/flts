use super::*;
use crate::id::Id;
use crate::storage::Arena;
use crate::storage::ColumnFile;
use crate::test_utils::{TempDir, committed};

struct Book;
struct Chapter;

impl Entity for Book {
    const NAME: &'static str = "book";
}

impl Entity for Chapter {
    const NAME: &'static str = "chapter";
}

fn owners(parents: &[usize]) -> RefColumn<Book, Chapter> {
    let mut column = RefColumn::<Book, Chapter>::default();
    for parent in parents {
        column.push(Id::from_index(*parent)).unwrap();
    }
    column
}

fn rows(children: &[Id<Chapter>]) -> Vec<usize> {
    children.iter().map(|child| child.index()).collect()
}

fn row(index: usize, rows: usize) -> Row {
    Row::of(index, rows, "chapter").unwrap()
}

#[test]
fn a_reference_column_reads_back_the_parent_it_was_given() {
    let column = owners(&[1, 0]);

    assert_eq!(column.rows(), 2);
    assert_eq!(column.get(row(0, 2)).unwrap(), Id::from_index(1));
    assert_eq!(column.get(row(1, 2)).unwrap(), Id::from_index(0));
}

#[test]
fn the_first_question_builds_the_index_and_the_second_reuses_it() {
    let column = owners(&[0, 1, 0]);
    assert!(
        !column.is_indexed(),
        "nothing is grouped until someone asks"
    );

    assert_eq!(
        rows(column.children_of(2, Id::from_index(0)).unwrap()),
        [0, 2]
    );
    assert!(column.is_indexed());

    assert_eq!(rows(column.children_of(2, Id::from_index(1)).unwrap()), [1]);
    assert!(column.is_indexed(), "the second question built nothing");
}

#[test]
fn pushing_a_child_drops_the_index() {
    let mut column = owners(&[0]);
    column.children_of(1, Id::from_index(0)).unwrap();
    assert!(column.is_indexed());

    column.push(Id::from_index(0)).unwrap();

    assert!(
        !column.is_indexed(),
        "a new child cannot be missed by a stale grouping"
    );
    assert_eq!(
        rows(column.children_of(1, Id::from_index(0)).unwrap()),
        [0, 1]
    );
}

#[test]
fn repointing_a_child_at_another_parent_drops_the_index() {
    let mut column = owners(&[0, 0]);
    column.children_of(2, Id::from_index(0)).unwrap();
    assert!(column.is_indexed());

    column.set(row(1, 2), Id::from_index(1)).unwrap();

    assert!(!column.is_indexed());
    assert_eq!(rows(column.children_of(2, Id::from_index(0)).unwrap()), [0]);
    assert_eq!(rows(column.children_of(2, Id::from_index(1)).unwrap()), [1]);
}

#[test]
fn reading_a_reference_column_from_a_file_drops_the_index() {
    let dir = TempDir::new("ref_read");
    let path = dir.join("chapters.dbs");

    let mut column = owners(&[0, 0]);
    column.children_of(1, Id::from_index(0)).unwrap();
    assert!(column.is_indexed());

    let file = committed(&path, |updater| column.offer_to("chapter", "book", updater));
    column.read_from("chapter", "book", &file).unwrap();

    assert!(
        !column.is_indexed(),
        "the rows were replaced wholesale, so the grouping of the old ones is worthless"
    );
    assert_eq!(column.rows(), 2);
}

#[test]
fn a_clone_inherits_the_grouping_it_was_cloned_from() {
    let column = owners(&[0, 0]);
    column.children_of(1, Id::from_index(0)).unwrap();

    let snapshot = column.clone();

    assert!(
        snapshot.is_indexed(),
        "a frozen view inherits warmth, as a cloned arena inherits resident chunks"
    );
    assert_eq!(
        rows(snapshot.children_of(1, Id::from_index(0)).unwrap()),
        [0, 1]
    );
}

#[test]
fn a_grouping_built_before_a_parent_arrived_still_answers_for_it() {
    let column = owners(&[0, 0]);
    assert_eq!(
        rows(column.children_of(1, Id::from_index(0)).unwrap()),
        [0, 1]
    );

    assert!(
        column.children_of(5, Id::from_index(4)).unwrap().is_empty(),
        "a parent pushed after the build has no children, and the stale count cannot lie about that"
    );
}

#[test]
fn a_reference_column_describes_itself_as_pointing_at_its_parent() {
    let mut specs = Vec::new();
    owners(&[]).describe("chapter", "book", &mut specs);

    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "chapter.book");
    assert_eq!(specs[0].elem_size, 8);
    assert_eq!(specs[0].kind, ColumnKind::Ref { parent: "book" });
}

#[test]
fn a_reference_column_survives_a_commit_and_a_reopen() {
    let dir = TempDir::new("ref_roundtrip");
    let path = dir.join("chapters.dbs");

    let column = owners(&[1, 0, 1]);
    let file = committed(&path, |updater| column.offer_to("chapter", "book", updater));

    let mut read = RefColumn::<Book, Chapter>::default();
    read.read_from("chapter", "book", &file).unwrap();

    assert_eq!(read.rows(), 3);
    assert_eq!(
        rows(read.children_of(2, Id::from_index(1)).unwrap()),
        [0, 2]
    );
}

#[test]
fn a_file_whose_reference_points_past_its_parent_is_corrupt() {
    let dir = TempDir::new("dangling");
    let path = dir.join("chapters.dbs");

    let mut ids = Arena::<Id<Book>>::with_chunk_rows(4);
    ids.push(Id::from_index(9)).unwrap();
    let file = committed(&path, |updater| updater.column("chapter.book", &ids));

    let mut read = RefColumn::<Book, Chapter>::default();
    read.read_from("chapter", "book", &file).unwrap();

    let err = read.children_of(1, Id::from_index(0)).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("chapter.book row 0"), "{err}");
}

#[test]
fn a_failed_grouping_is_not_remembered() {
    let mut column = owners(&[3]);

    assert!(column.children_of(1, Id::from_index(0)).is_err());
    assert!(
        !column.is_indexed(),
        "the parent table may simply not have grown yet; the answer must be asked again"
    );

    assert_eq!(rows(column.children_of(4, Id::from_index(3)).unwrap()), [0]);
    column.push(Id::from_index(0)).unwrap();
    assert!(!column.is_indexed());
}

#[test]
fn orphans_are_listed_apart_from_every_parent() {
    let mut column = RefColumn::<Book, Chapter>::default();
    column.push(Id::from_index(0)).unwrap();
    column.push(Id::from_raw(Id::<Book>::NULL)).unwrap();

    assert_eq!(rows(column.children_of(1, Id::from_index(0)).unwrap()), [0]);
    assert_eq!(rows(column.orphans(1).unwrap()), [1]);
}

#[test]
fn touching_the_tail_of_a_rotten_reference_column_reports_the_damage() {
    let dir = TempDir::new("ref_tail");
    let path = dir.join("chapters.dbs");

    let column = owners(&[0]);
    let file = committed(&path, |updater| column.offer_to("chapter", "book", updater));
    let tail = file.column_meta("chapter.book").unwrap().chunks[0];
    drop(file);
    crate::test_utils::flip_byte(&path, tail.offset);

    let mut read = RefColumn::<Book, Chapter>::default();
    read.read_from("chapter", "book", &ColumnFile::open(&path).unwrap())
        .unwrap();

    let err = read.touch_tail().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}
