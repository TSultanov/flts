use proptest::prelude::*;

use super::*;

const _: () = assert!(<Dot as Record>::SIZE == 16);
const _: () = assert!(<Version as Record>::SIZE == 24);

fn round_trip<T: Record + PartialEq + fmt::Debug>(value: T) -> T {
    let mut bytes = vec![0u8; T::SIZE];
    value.encode(&mut bytes);
    T::decode(&bytes)
}

fn wrote(raw: u64, counter: u64) -> Dot {
    Dot::new(WriterId::from_raw(raw), counter)
}

fn writer() -> impl Strategy<Value = WriterId> {
    prop_oneof![
        1 => Just(WriterId::ANONYMOUS),
        4 => (0u64..4).prop_map(WriterId::from_raw),
        4 => any::<u64>().prop_map(WriterId::from_raw),
    ]
}

fn minted() -> impl Strategy<Value = Dot> {
    (writer(), prop_oneof![0u64..4, any::<u64>()])
        .prop_map(|(writer, counter)| Dot::new(writer, counter))
}

#[test]
fn a_dot_names_the_writer_that_minted_it_and_the_count_it_was_minted_at() {
    let writer = WriterId::from_device_id("the reading room");
    let dot = Dot::new(writer, 91);

    assert_eq!(dot.writer(), writer);
    assert_eq!(dot.counter(), 91);
}

#[test]
fn a_dot_is_ordered_by_its_count_and_falls_back_to_its_writer_only_for_a_tie() {
    assert!(
        wrote(9, 1) < wrote(1, 2),
        "a larger writer id must not outrank a later count, or a quiet replica never wins"
    );
    assert!(
        wrote(1, 5) < wrote(2, 5),
        "two writers that counted the same far still need one of them to come first"
    );
}

#[test]
fn a_version_is_live_until_a_later_one_replaces_it_or_a_removal_withdraws_it() {
    let fresh = Version::of(wrote(1, 7));

    assert!(fresh.is_live());
    assert!(fresh.is_current());
    assert!(!fresh.supersede().is_live());
    assert!(!fresh.withdraw().is_live());
}

#[test]
fn a_withdrawn_version_is_still_the_current_one_until_something_supersedes_it() {
    let gone = Version::of(wrote(1, 7)).withdraw();

    assert!(
        gone.is_current(),
        "a removal is the newest thing said about an id, not the absence of anything said"
    );
    assert!(gone.is_removed());
    assert!(!gone.supersede().is_current());
}

#[test]
fn a_version_keeps_the_dot_it_was_made_with_through_every_mark_it_gathers() {
    let dot = wrote(3, 11);

    assert_eq!(Version::of(dot).dot(), dot);
    assert_eq!(Version::of(dot).supersede().withdraw().dot(), dot);
}

#[test]
fn a_version_that_gathered_both_marks_still_reports_each_one() {
    let both = Version::of(wrote(1, 7)).withdraw().supersede();

    assert!(both.is_superseded());
    assert!(both.is_removed());
    assert!(!both.is_live());
}

#[test]
fn a_version_marked_with_something_this_build_does_not_know_is_refused() {
    let mut bytes = vec![0u8; <Version as Record>::SIZE];
    Version::of(wrote(1, 7)).encode(&mut bytes);
    bytes[16] = 0b1000;

    let err = Version::decode(&bytes)
        .check("book.version", 4)
        .expect_err("a flag nobody wrote must not be read as no flag at all");

    assert!(err.is_unsupported(), "{err}");
    assert!(err.to_string().contains("book.version row 4"), "{err}");
    assert!(err.to_string().contains("0x8"), "{err}");
}

#[test]
fn a_version_this_build_did_write_passes_the_check_whatever_it_is_marked_with() {
    for held in [
        Version::of(wrote(1, 7)),
        Version::of(wrote(1, 7)).supersede(),
        Version::of(wrote(1, 7)).withdraw(),
        Version::of(wrote(1, 7)).supersede().withdraw(),
    ] {
        held.check("book.version", 0)
            .unwrap_or_else(|err| panic!("{held:?} is a version this build wrote: {err}"));
    }
}

#[test]
fn a_purge_is_a_removal_that_also_says_it_is_final() {
    let dot = wrote(1, 7);
    let held = Version::of(dot).purge();

    assert!(held.is_purged() && held.is_removed());
    assert!(held.is_current() && !held.is_live() && !held.is_restoration());
    assert_eq!(held.dot(), dot);
    assert!(!Version::of(dot).is_purged());
    assert!(!Version::of(dot).withdraw().is_purged());
    assert_eq!(Version::of(dot).restoration().purge(), held);
    assert_eq!(round_trip(held), held);
    assert_ne!(held, Version::of(dot).withdraw());
    held.check("book.version", 0).unwrap();
    assert_eq!(format!("{held:?}"), "0x0000000000000001@7 removed purged");

    let mut bytes = vec![0u8; <Version as Record>::SIZE];
    held.encode(&mut bytes);
    assert_eq!(
        bytes[16], 0b1_0010,
        "a build that predates purging reads a removal, and refuses the other bit when it audits"
    );
    bytes[16] = 0b10_0000;
    assert!(Version::decode(&bytes).check("book.version", 0).is_err());
}

#[test]
fn a_mark_put_on_a_version_twice_says_no_more_than_it_did_once() {
    let held = Version::of(wrote(1, 7));

    assert_eq!(held.supersede().supersede(), held.supersede());
    assert_eq!(held.withdraw().withdraw(), held.withdraw());
    assert!(
        held.supersede().supersede().is_superseded(),
        "a row superseded twice is not a row handed back"
    );
    assert!(
        held.withdraw().withdraw().is_removed(),
        "and a row removed twice is not a row restored"
    );
}

#[test]
fn a_dot_says_which_writer_counted_and_how_far() {
    assert_eq!(format!("{:?}", wrote(0x2a, 7)), "0x000000000000002a@7");
}

#[test]
fn a_version_says_in_words_every_mark_it_carries() {
    let dot = wrote(1, 7);

    assert_eq!(format!("{:?}", Version::of(dot)), "0x0000000000000001@7");
    assert_eq!(
        format!("{:?}", Version::of(dot).supersede()),
        "0x0000000000000001@7 superseded"
    );
    assert_eq!(
        format!("{:?}", Version::of(dot).withdraw()),
        "0x0000000000000001@7 removed"
    );
    assert_eq!(
        format!("{:?}", Version::of(dot).supersede().withdraw()),
        "0x0000000000000001@7 superseded removed"
    );
}

proptest! {
    #[test]
    fn every_dot_comes_back_from_the_bytes_it_was_written_as(dot in minted()) {
        prop_assert_eq!(round_trip(dot), dot);
    }

    #[test]
    fn every_version_comes_back_from_the_bytes_it_was_written_as(
        dot in minted(),
        superseded: bool,
        removed: bool,
    ) {
        let mut held = Version::of(dot);
        if superseded {
            held = held.supersede();
        }
        if removed {
            held = held.withdraw();
        }

        let read = round_trip(held);
        prop_assert_eq!(read, held);
        prop_assert_eq!(read.is_superseded(), superseded);
        prop_assert_eq!(read.is_removed(), removed);
    }

    #[test]
    fn two_dots_compare_equal_only_when_they_name_the_same_write(
        one in minted(),
        other in minted(),
    ) {
        prop_assert_eq!(one.cmp(&other) == Ordering::Equal, one == other);
    }
}

fn column(versions: &[Version]) -> VersionColumn<crate::test_utils::library::Book> {
    let mut held = VersionColumn::default();
    for version in versions {
        held.push(*version).unwrap();
    }
    held
}

fn at(held: &VersionColumn<crate::test_utils::library::Book>, row: usize) -> Version {
    held.raw_at(Row::of(row, Column::rows(held), "book").unwrap())
        .unwrap()
}

#[test]
fn a_version_column_hands_back_the_write_that_was_pushed_into_it() {
    let held = column(&[Version::of(wrote(1, 5)), Version::UNSTAMPED]);

    assert_eq!(Column::rows(&held), 2);
    assert_eq!(at(&held, 0).dot(), wrote(1, 5));
    assert!(!at(&held, 1).is_stamped());
}

#[test]
fn a_column_of_versions_refuses_a_mark_this_build_does_not_know() {
    let mut bytes = vec![0u8; <Version as Record>::SIZE];
    Version::of(wrote(1, 7)).encode(&mut bytes);
    bytes[16] = 0b1000;

    let held = column(&[Version::of(wrote(1, 5)), Version::decode(&bytes)]);
    let err = Column::audit(&held).expect_err("an audit reads every row, marks included");

    assert!(err.to_string().contains("book.version row 1"), "{err}");
}

#[test]
fn stamping_counts_once_for_every_unstamped_row_and_leaves_the_rest_alone() {
    let already = wrote(4, 9);
    let mut held = column(&[
        Version::UNSTAMPED,
        Version::of(already),
        Version::UNSTAMPED.withdraw(),
    ]);
    let mut clock = VersionVector::default();

    held.stamp(&mut clock, WriterId::from_raw(1)).unwrap();

    assert_eq!(
        at(&held, 1).dot(),
        already,
        "a stamped row is not stamped again"
    );
    assert!(at(&held, 0).is_stamped());
    assert!(at(&held, 2).is_stamped());
    assert!(
        at(&held, 0).dot() < at(&held, 2).dot(),
        "rows are stamped in the order they were appended"
    );
    assert!(
        at(&held, 2).is_removed(),
        "stamping a row says when it was written, not what was written"
    );
    assert_eq!(held.unstamped_from().unwrap(), 3);
}

#[test]
fn a_column_of_versions_read_from_a_file_has_nothing_left_to_stamp() {
    let mut held: VersionColumn<crate::test_utils::library::Book> = VersionColumn::default();
    held.push(Version::UNSTAMPED).unwrap();

    assert_eq!(
        held.unstamped_from().unwrap(),
        0,
        "a row pushed here has not been published yet"
    );

    let mut clock = VersionVector::default();
    held.stamp(&mut clock, WriterId::from_raw(1)).unwrap();

    assert_eq!(
        held.unstamped_from().unwrap(),
        1,
        "a commit that stamped every row has nothing to walk on the next one"
    );
}

#[test]
fn a_version_column_names_itself_beside_the_rows_it_counts() {
    let held = column(&[Version::of(wrote(1, 5))]);

    let mut told = Vec::new();
    Column::describe(&held, "book", "version", &mut told);

    assert_eq!(told.len(), 5, "{told:?}");
    assert_eq!(told[0].name, "book.version");
    assert_eq!(told[0].elem_size, 24);
    assert_eq!(told[0].kind, ColumnKind::Version);
    assert!(
        told[0].counts_rows,
        "every row carries the write that made it, so this column is as long as the table"
    );
}
