use proptest::prelude::*;

use super::super::indexed::Indexed;
use super::*;
use crate::schema::index::{HashIndex, capacity_for, home_of};
use crate::storage::ColumnFile;
use crate::test_utils::library::{Author, Authorship, Book};
use crate::test_utils::{TempDir, committed, flip_byte};

const _: () = assert!(<Pair<Book, Author> as Record>::SIZE == 32);

fn pair(left: u128, right: u128) -> Pair<Book, Author> {
    Pair::of(
        Id::from_uuid(crate::id::Uuid::from_u128(left)),
        Id::from_uuid(crate::id::Uuid::from_u128(right)),
    )
}

fn filed(pairs: &[Pair<Book, Author>]) -> Distinct<Book, Author> {
    let mut column: Distinct<Book, Author> = Indexed::named("authorship.book_author");
    for held in pairs {
        column.push(*held).unwrap();
    }
    column
}

fn minted() -> impl Strategy<Value = Pair<Book, Author>> {
    (
        prop_oneof![Just(0u128), 1u128..5],
        prop_oneof![Just(0u128), 1u128..5],
    )
        .prop_map(|(left, right)| pair(left, right))
}

#[test]
fn a_pair_hands_back_the_two_ids_it_was_made_of() {
    let book = Id::<Book>::mint();
    let author = Id::<Author>::mint();

    let held = Pair::of(book, author);

    assert_eq!(held.left(), book);
    assert_eq!(held.right(), author);
    assert!(held.is_whole());
}

#[test]
fn a_pair_missing_either_of_its_ids_is_not_a_whole_pair() {
    let book = Id::<Book>::mint();
    let author = Id::<Author>::mint();

    assert!(!Pair::<Book, Author>::of(Id::NIL, author).is_whole());
    assert!(!Pair::of(book, Id::<Author>::NIL).is_whole());
    assert!(!Pair::<Book, Author>::of(Id::NIL, Id::NIL).is_whole());
}

#[test]
fn a_pair_that_differs_at_either_end_is_a_different_key_at_a_different_bucket() {
    assert_ne!(
        pair(1, 2).raw(),
        pair(1, 3).raw(),
        "a key blind to its right end files every link of one book in one bucket"
    );
    assert_ne!(
        pair(2, 1).raw(),
        pair(3, 1).raw(),
        "and one blind to its left end does the same for every link of one author"
    );

    let capacity = capacity_for(64);
    for shared in [pair, |right, left| pair(left, right)] {
        let homes: std::collections::BTreeSet<usize> = (1..=64)
            .map(|at| home_of(shared(1, at).raw(), capacity))
            .collect();
        assert!(
            homes.len() > 8,
            "64 pairs sharing one end land in {} of {capacity} buckets",
            homes.len()
        );
    }
}

#[test]
fn a_pair_column_refuses_a_value_it_already_holds_and_never_an_absent_one() {
    let mut column = filed(&[pair(1, 2)]);

    let err = column.refuse_duplicate(pair(1, 2)).unwrap_err();
    assert!(err.to_string().contains("authorship.book_author"), "{err}");
    column.refuse_duplicate(pair(1, 3)).unwrap();
    column.refuse_duplicate(pair(2, 2)).unwrap();

    column.refuse_duplicate(pair(0, 2)).unwrap();
    column.push(pair(0, 2)).unwrap();
    column.push(pair(0, 2)).unwrap();
    assert!(
        column.refuse_duplicate(pair(0, 2)).is_ok(),
        "half a pair names nobody, so no two of them are the same link"
    );
}

#[test]
fn a_pair_column_finds_nothing_for_a_pair_that_is_missing_an_end() {
    let column = filed(&[pair(0, 2), pair(1, 0)]);

    assert_eq!(column.find_one(pair(0, 2)).unwrap(), None);
    assert_eq!(column.find_one(pair(1, 0)).unwrap(), None);
    assert_eq!(
        column.find(pair(0, 2)).unwrap().len(),
        1,
        "a scan still sees it"
    );
}

#[test]
fn a_pair_column_is_written_and_read_back_whole() {
    let dir = TempDir::new("pair_file");
    let path = dir.join("links.dbs");

    let column = filed(&[pair(1, 2), pair(1, 3), pair(2, 2)]);
    let file = committed(&path, |updater| {
        column.offer_to("authorship", "book_author", updater)
    });

    let mut read: Distinct<Book, Author> = Indexed::named("authorship.book_author");
    read.read_from("authorship", "book_author", &file).unwrap();

    assert_eq!(read.find_one(pair(1, 3)).unwrap().unwrap().index(), 1);
    assert_eq!(read.find_one(pair(3, 3)).unwrap(), None);
    read.audit().unwrap();
}

#[test]
fn a_pair_column_stripped_of_its_index_rebuilds_it_on_open() {
    let dir = TempDir::new("pair_rebuild");
    let path = dir.join("links.dbs");

    let column = filed(&[pair(1, 2), pair(1, 3)]);
    let file = committed(&path, |updater| {
        column
            .column()
            .offer_to("authorship", "book_author", updater)
    });
    assert!(file.column_meta("authorship.book_author.buckets").is_none());

    let mut read: Distinct<Book, Author> = Indexed::named("authorship.book_author");
    read.read_from("authorship", "book_author", &file).unwrap();

    assert_eq!(read.find_one(pair(1, 3)).unwrap().unwrap().index(), 1);
    read.audit().unwrap();
}

#[test]
fn an_audit_refuses_a_pair_that_disagrees_with_the_references_beside_it() {
    let (book, author) = (Id::<Book>::mint(), Id::<Author>::mint());
    let mut books: RefColumn<Book, Authorship> = RefColumn::named("authorship.book");
    let mut authors: RefColumn<Author, Authorship> = RefColumn::named("authorship.author");
    books.push(book).unwrap();
    authors.push(author).unwrap();

    let agreeing = filed(&[Pair::of(book, author)]);
    check_pairs::<Book, Author, Authorship>(&agreeing, &books, &authors).unwrap();

    let stranger = Id::<Author>::mint();
    let drifted = filed(&[Pair::of(book, stranger)]);
    let err = check_pairs::<Book, Author, Authorship>(&drifted, &books, &authors).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(
        err.to_string().contains("authorship.book_author row 0"),
        "{err}"
    );
    assert!(
        err.to_string().contains(&format!("author {stranger}")),
        "the pair on file names both of its ends: {err}"
    );
    assert!(
        err.to_string().contains(&format!("author {author}")),
        "and so does the pair the row actually holds: {err}"
    );
    assert_eq!(
        err.to_string().matches(&format!("book {book}")).count(),
        2,
        "the end they agree on is named by both: {err}"
    );
}

#[test]
fn a_pair_column_describes_the_one_column_it_keeps_and_how_wide_it_is() {
    let column = filed(&[pair(1, 2)]);

    let mut told = Vec::new();
    Column::describe(&column, "authorship", "book_author", &mut told);

    assert_eq!(told[0].name, "authorship.book_author");
    assert_eq!(told[0].elem_size, 32);
    assert_eq!(
        told[0].kind,
        ColumnKind::Pair {
            left: "book",
            right: "author"
        }
    );
    assert!(told[0].counts_rows);
    assert_eq!(
        told.iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>(),
        [
            "authorship.book_author",
            "authorship.book_author.buckets",
            "authorship.book_author.links"
        ]
    );
}

#[test]
fn a_pair_column_reports_the_chunks_it_holds_and_the_ones_it_has_read() {
    let column = filed(&[pair(1, 2), pair(1, 3)]);

    let mut told = Vec::new();
    Column::residency(&column, "authorship", "book_author", &mut told);

    let pairs = told
        .iter()
        .find(|held| held.name == "authorship.book_author")
        .expect("the pair column names itself");
    assert_eq!(pairs.chunks, 1);
    assert_eq!(pairs.resident, 1, "a column just written is in hand");
}

#[test]
fn a_cloned_pair_column_answers_for_every_row_the_one_it_came_from_holds() {
    let column = filed(&[pair(1, 2), pair(1, 3), pair(2, 2)]);

    let copy = column.clone();

    assert_eq!(Column::rows(&copy), 3);
    assert_eq!(copy.find_one(pair(1, 3)).unwrap().unwrap().index(), 1);
    Column::audit(&copy).unwrap();
}

#[test]
fn a_pair_written_over_another_moves_the_row_to_the_key_it_now_holds() {
    let mut column = filed(&[pair(1, 2), pair(1, 3)]);
    let row = column.find_one(pair(1, 2)).unwrap().unwrap();

    column.set(row, pair(4, 5)).unwrap();

    assert_eq!(column.find_one(pair(1, 2)).unwrap(), None);
    assert_eq!(column.find_one(pair(4, 5)).unwrap().unwrap(), row);
    assert_eq!(column.column().key_at(row.index()).unwrap(), pair(4, 5));
    Column::audit(&column).unwrap();

    let err = column.set(row, pair(1, 3)).unwrap_err();
    assert!(err.to_string().contains("authorship.book_author"), "{err}");
}

#[test]
fn touching_the_tail_of_a_rotten_pair_column_reports_the_damage() {
    let dir = TempDir::new("pair_tail");
    let path = dir.join("links.dbs");

    let column = filed(&[pair(1, 2)]);
    let file = committed(&path, |updater| {
        column.offer_to("authorship", "book_author", updater)
    });
    let tail = file.column_meta("authorship.book_author").unwrap().chunks[0];
    drop(file);
    flip_byte(&path, tail.offset);

    let mut read: Distinct<Book, Author> = Indexed::named("authorship.book_author");
    read.read_from(
        "authorship",
        "book_author",
        &ColumnFile::open(&path).unwrap(),
    )
    .unwrap();

    let err = Column::touch_tail(&read).unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    let err = Column::make_room(&read, 2).unwrap_err();
    assert!(
        err.is_corrupt(),
        "a push must fault its pages before it writes: {err}"
    );
}

proptest! {
    #[test]
    fn two_pairs_are_one_key_only_when_both_of_their_ends_are_the_same(
        left in minted(),
        right in minted(),
    ) {
        prop_assert_eq!(
            left == right,
            left.left() == right.left() && left.right() == right.right()
        );
    }

    #[test]
    fn two_pairs_that_are_equal_always_name_the_same_bucket(
        left in minted(),
        right in minted(),
    ) {
        if left != right {
            return Ok(());
        }
        prop_assert_eq!(left.raw(), right.raw(), "{:?} is one pair at two hashes", left);
        for rows in [1usize, 9, 5000] {
            let capacity = capacity_for(rows);
            prop_assert_eq!(
                home_of(left.raw(), capacity),
                home_of(right.raw(), capacity)
            );
        }
    }

    #[test]
    fn every_thirty_two_bytes_decode_as_a_pair_and_re_encode_unchanged(
        bytes in prop::array::uniform32(any::<u8>()),
    ) {
        let held = <Pair<Book, Author> as Record>::decode(&bytes);
        let mut again = [0u8; 32];
        held.encode(&mut again);

        prop_assert_eq!(again, bytes);
    }

    #[test]
    fn a_pair_column_names_exactly_the_rows_a_scan_of_it_would_name(
        pairs in prop::collection::vec(minted(), 0..60),
        sought in minted(),
    ) {
        let column = filed(&pairs);
        let scanned: Vec<usize> = pairs
            .iter()
            .enumerate()
            .filter(|(_, held)| **held == sought)
            .map(|(row, _)| row)
            .collect();

        let found: Vec<usize> = column.find(sought)?.iter().map(|row| row.index()).collect();

        prop_assert_eq!(found, scanned, "the index disagrees with a scan for {:?}", sought);
        Column::audit(&column)?;
    }

    #[test]
    fn a_pair_index_covers_every_row_of_the_column_it_is_built_over(
        pairs in prop::collection::vec(minted(), 0..60),
    ) {
        let column = filed(&pairs);
        let mut rebuilt = HashIndex::new("authorship.book_author".to_owned());
        rebuilt.rebuild(column.column())?;

        for (row, held) in pairs.iter().enumerate() {
            prop_assert!(
                rebuilt.rows_of(*held)?.contains(&row),
                "row {} holding {:?} is not in its own bucket",
                row,
                held
            );
        }
    }
}
