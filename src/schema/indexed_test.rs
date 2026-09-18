use proptest::prelude::*;

use super::*;
use crate::record::Record;
use crate::schema::index::{capacity_for, home_of};
use crate::schema::{
    EnumColumn, IndexKey, Maybe, NullableColumn, NullableTextColumn, TextColumn, TextKey,
};
use crate::storage::{Blob, ColumnFile};
use crate::test_utils::library::Binding;
use crate::test_utils::{TempDir, committed, flip_byte, props};

const _: () = assert!(<TextKey as Record>::SIZE == 24);
const _: () = assert!(<Maybe<u32> as Record>::SIZE == 5);

fn titles(values: &[&str]) -> Indexed<TextColumn, false> {
    let mut column = Indexed::named("book.title");
    for value in values {
        column.push(*value).unwrap();
    }
    column
}

fn found(column: &Indexed<TextColumn, false>, value: &str) -> Vec<usize> {
    column
        .find(value)
        .unwrap()
        .into_iter()
        .map(Row::index)
        .collect()
}

fn at(column: &Indexed<TextColumn, false>, row: usize) -> Row {
    Row::of(row, Column::rows(column), "book.title").unwrap()
}

#[test]
fn a_whole_number_keys_an_index_by_the_value_it_is() {
    assert_eq!(7u8.raw(), 7);
    assert_eq!(u8::MAX.raw(), 255);
    assert_eq!(0u64.raw(), 0);
    assert_eq!(u64::MAX.raw(), u128::from(u64::MAX));
    assert_eq!(1234u16.raw(), 1234);
    assert_eq!(70_000u32.raw(), 70_000);
    assert_eq!(7i8.raw(), 7);
    assert_eq!((-1i8).raw(), u128::MAX);
    assert_eq!((-1i64).raw(), u128::MAX);
    assert_eq!(i32::MIN.raw(), u128::MAX - 2_147_483_647);
    assert_eq!(true.raw(), 1);
    assert_eq!(false.raw(), 0);
}

#[test]
fn an_absent_value_keys_an_index_apart_from_every_value_that_is_there() {
    assert_eq!(Maybe(Some(7u32)).raw(), 7);
    assert_eq!(Maybe::<u32>(None).raw(), u128::MAX);
    assert_ne!(Maybe(Some(0u32)).raw(), Maybe::<u32>(None).raw());
    assert!(TextKey::maybe(None).is_absent());
    assert!(!TextKey::maybe(None).is_inline());
    assert!(!TextKey::maybe(Some("")).is_absent());
    assert_ne!(TextKey::maybe(None), TextKey::maybe(Some("")));
}

#[test]
fn a_string_keys_the_same_way_the_file_format_has_always_keyed_it() {
    for (value, inline, raw) in [
        ("", true, 0xcbf2_9ce4_8422_2325u64),
        ("Alice", true, 0x1239_09cb_9f15_d167),
        ("Alice in Wonderland", true, 0xdd46_dc9c_499c_a76c),
        ("exactly twenty bytes", true, 0xad80_5921_126f_5c1f),
        ("twenty one bytes long", false, 0xe6c7_de41_32df_8353),
    ] {
        let key = TextKey::of(value);
        assert_eq!(
            key.is_inline(),
            inline,
            "{value:?} is {} bytes",
            value.len()
        );
        assert_eq!(key.raw(), u128::from(raw), "{value:?}");
    }
}

#[test]
fn two_short_values_that_begin_with_the_same_eight_bytes_are_not_one_key() {
    assert_eq!(TextKey::of("prefixed-one"), TextKey::of("prefixed-one"));
    assert_ne!(
        TextKey::of("prefixed-one"),
        TextKey::of("prefixed-two"),
        "a key with room for the whole value is compared by the whole of it"
    );
    assert_eq!(TextKey::maybe(None), TextKey::maybe(None));
    assert_ne!(TextKey::of("Alice"), TextKey::of("Kokoro"));
}

#[test]
fn a_key_that_carries_its_own_bytes_stops_where_the_room_for_them_stops() {
    assert!(TextKey::of(&"x".repeat(20)).is_inline());
    assert!(!TextKey::of(&"x".repeat(21)).is_inline());
}

#[test]
fn a_long_key_is_the_same_key_wherever_the_bytes_it_names_were_put() {
    let long = "twenty one bytes long";
    let here = TextKey::stored(long, Blob::new(0, long.len() as u32));
    let there = TextKey::stored(long, Blob::new(4096, long.len() as u32));

    assert_eq!(
        here, there,
        "where a value sits is what it holds, not which value it is"
    );
    assert_eq!(here.raw(), there.raw());
    assert_ne!(here.blob(), there.blob());
}

#[test]
fn a_column_names_every_row_that_holds_the_value_it_is_asked_for() {
    let column = titles(&["Alice", "Kokoro", "Alice", "Dune"]);

    assert_eq!(found(&column, "Alice"), [0, 2]);
    assert_eq!(found(&column, "Dune"), [3]);
    assert!(found(&column, "Moby-Dick").is_empty());
}

#[test]
fn a_column_tells_two_long_values_apart_that_no_short_key_could_hold() {
    let first = "a title longer than twenty bytes, one";
    let second = "a title longer than twenty bytes, two";
    let column = titles(&[first, second, first]);

    assert_eq!(found(&column, first), [0, 2]);
    assert_eq!(found(&column, second), [1]);
}

#[test]
fn a_value_moved_off_a_row_leaves_no_trace_under_the_value_it_left() {
    let mut column = titles(&["Alice", "Kokoro", "Alice"]);

    column.set(at(&column, 2), "Dune").unwrap();

    assert_eq!(found(&column, "Alice"), [0]);
    assert_eq!(found(&column, "Dune"), [2]);
    assert_eq!(column.column().get(at(&column, 2)).unwrap(), "Dune");
}

#[test]
fn an_index_over_a_value_survives_a_commit_and_a_reopen() {
    let dir = TempDir::new("indexed_roundtrip");
    let path = dir.join("books.dbs");

    let column = titles(&["Alice", "Kokoro", "Alice"]);
    let file = committed(&path, |updater| column.offer_to("book", "title", updater));

    let mut read: Indexed<TextColumn, false> = Indexed::named("book.title");
    read.read_from("book", "title", &file).unwrap();

    assert_eq!(found(&read, "Alice"), [0, 2]);
    assert_eq!(found(&read, "Kokoro"), [1]);
    read.audit().unwrap();
}

#[test]
fn an_audit_refuses_an_index_that_files_a_row_under_a_value_it_does_not_hold() {
    let mut column = titles(&["Alice", "Kokoro"]);
    column.index.append(TextKey::of("Dune")).unwrap();

    let err = column.audit().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
    assert!(err.to_string().contains("book.title"), "{err}");
}

#[test]
fn a_column_whose_index_disagrees_with_it_is_rebuilt_when_it_is_read() {
    let dir = TempDir::new("indexed_rebuild");
    let path = dir.join("books.dbs");

    let mut column = titles(&["Alice", "Kokoro", "Alice"]);
    column.index = HashIndex::new("book.title".to_owned());
    let file = committed(&path, |updater| column.offer_to("book", "title", updater));

    let mut read: Indexed<TextColumn, false> = Indexed::named("book.title");
    read.read_from("book", "title", &file).unwrap();

    assert_eq!(found(&read, "Alice"), [0, 2]);
    read.audit().unwrap();
}

#[test]
fn a_column_that_keys_by_a_digest_reads_a_row_back_before_it_says_it_matched() {
    let mut column = TextColumn::default();
    let long = "a title longer than twenty bytes, one";
    let other = "a title longer than twenty bytes, two";
    let key = column.append(long, None).unwrap();
    column.append(other, None).unwrap();

    assert!(column.holds(key, long).unwrap());
    assert!(
        !column.holds(key, other).unwrap(),
        "two values of one length and one digest are still two values"
    );
    assert!(!TextColumn::key_for(long).is_inline());
}

#[test]
fn a_unique_column_refuses_a_value_another_row_already_holds() {
    let mut isbns: Unique<NullableTextColumn> = Indexed::named("edition.isbn");
    isbns.push(Some("978-0-19-955829-2")).unwrap();
    isbns.push(None).unwrap();
    isbns.push(None).unwrap();

    isbns.refuse_duplicate(Some("978-0-14-243724-7")).unwrap();
    isbns
        .refuse_duplicate(None)
        .expect("absence is not a value two rows may not share");

    let err = isbns
        .refuse_duplicate(Some("978-0-19-955829-2"))
        .unwrap_err();
    assert!(err.to_string().contains("edition.isbn"), "{err}");
    assert!(err.to_string().contains("row 0"), "{err}");
}

#[test]
fn a_unique_column_names_the_one_row_that_holds_a_value() {
    let mut isbns: Unique<NullableTextColumn> = Indexed::named("edition.isbn");
    isbns.push(Some("978-0-19-955829-2")).unwrap();
    isbns.push(None).unwrap();

    assert_eq!(
        isbns
            .find_one(Some("978-0-19-955829-2"))
            .unwrap()
            .map(Row::index),
        Some(0)
    );
    assert_eq!(isbns.find_one(Some("978-0-14-243724-7")).unwrap(), None);
    assert_eq!(
        isbns.find_one(None).unwrap(),
        None,
        "many rows may be absent, so absence names no one row"
    );
}

#[test]
fn a_unique_column_lets_a_row_be_set_to_the_value_it_already_holds() {
    let mut isbns: Unique<NullableTextColumn> = Indexed::named("edition.isbn");
    isbns.push(Some("978-0-19-955829-2")).unwrap();
    isbns.push(Some("978-0-14-243724-7")).unwrap();
    let first = Row::of(0, 2, "edition.isbn").unwrap();
    let second = Row::of(1, 2, "edition.isbn").unwrap();

    isbns
        .set(first, Some("978-0-19-955829-2"))
        .expect("a row does not collide with itself");

    let err = isbns.set(second, Some("978-0-19-955829-2")).unwrap_err();
    assert!(err.to_string().contains("row 0"), "{err}");
    assert_eq!(
        isbns.column().get(second).unwrap().as_deref(),
        Some("978-0-14-243724-7"),
        "a refused set leaves the row the value it had"
    );
}

#[test]
fn a_column_of_numbers_names_the_rows_that_hold_one() {
    let mut years: Indexed<Arena<u32>, false> = Indexed::named("edition.year");
    for year in [1865u32, 1914, 1865, 2001] {
        years.push(year).unwrap();
    }

    let rows: Vec<usize> = years
        .find(1865)
        .unwrap()
        .into_iter()
        .map(Row::index)
        .collect();
    assert_eq!(rows, [0, 2]);
    assert!(years.find(1900).unwrap().is_empty());
}

#[test]
fn a_column_of_absent_numbers_keeps_them_apart_from_the_zero_it_stores() {
    let mut pages: Indexed<NullableColumn<u32>, false> = Indexed::named("edition.pages");
    for count in [Some(0u32), None, Some(0), None] {
        pages.push(count).unwrap();
    }

    let zeroes: Vec<usize> = pages
        .find(Some(0))
        .unwrap()
        .into_iter()
        .map(Row::index)
        .collect();
    let absent: Vec<usize> = pages
        .find(None)
        .unwrap()
        .into_iter()
        .map(Row::index)
        .collect();
    assert_eq!(zeroes, [0, 2]);
    assert_eq!(
        absent,
        [1, 3],
        "a column stores a default where a value is absent"
    );
}

#[test]
fn a_column_of_variants_names_the_rows_that_hold_one() {
    let mut bindings: Indexed<EnumColumn<Binding>, false> = Indexed::named("edition.binding");
    for binding in [Binding::Cloth, Binding::Paper, Binding::Cloth] {
        bindings.push(binding).unwrap();
    }

    let cloth: Vec<usize> = bindings
        .find(Binding::Cloth)
        .unwrap()
        .into_iter()
        .map(Row::index)
        .collect();
    assert_eq!(cloth, [0, 2]);
}

#[test]
fn an_indexed_column_describes_and_reports_residency_for_the_same_columns_in_order() {
    let column = titles(&["Alice"]);

    let mut specs = Vec::new();
    column.describe("book", "title", &mut specs);
    let mut held = Vec::new();
    column.residency("book", "title", &mut held);

    let named: Vec<&String> = specs.iter().map(|spec| &spec.name).collect();
    let reported: Vec<&String> = held.iter().map(|entry| &entry.name).collect();
    assert_eq!(
        named,
        [
            "book.title",
            "book.title.bytes",
            "book.title.buckets",
            "book.title.links"
        ]
    );
    assert_eq!(named, reported);
    assert_eq!(
        specs[3].elem_size, 32,
        "a link carries a whole key and a next"
    );
}

#[test]
fn a_short_value_is_found_without_reading_one_byte_of_the_column_that_holds_it() {
    let dir = TempDir::new("indexed_cold");
    let path = dir.join("books.dbs");

    let mut column: Indexed<TextColumn, false> = Indexed::named("book.title");
    for row in 0..6000 {
        column.push(&format!("title number {row}")).unwrap();
    }
    let file = committed(&path, |updater| column.offer_to("book", "title", updater));
    drop(file);

    let mut read: Indexed<TextColumn, false> = Indexed::named("book.title");
    read.read_from("book", "title", &ColumnFile::open(&path).unwrap())
        .unwrap();
    let mut cold = Vec::new();
    read.residency("book", "title", &mut cold);
    assert!(
        cold.iter().all(|held| held.resident == 0),
        "opening a column reads no payload: {cold:?}"
    );

    assert_eq!(found(&read, "title number 4000"), [4000]);

    let mut warm = Vec::new();
    read.residency("book", "title", &mut warm);
    let held = |name: &str| {
        warm.iter()
            .find(|held| held.name == name)
            .expect("the column names this")
            .resident
    };
    assert_eq!(
        held("book.title"),
        0,
        "the key carries the whole value, so the column it indexes is never read"
    );
    assert_eq!(
        held("book.title.bytes"),
        0,
        "and neither is the heap those values live in"
    );
    assert_eq!(held("book.title.buckets"), 1, "one bucket was read");
    assert!(warm.iter().any(|held| held.chunks > 1), "{warm:?}");
}

#[test]
fn a_long_value_is_confirmed_from_the_heap_and_not_from_the_column_it_indexes() {
    let dir = TempDir::new("indexed_long");
    let path = dir.join("books.dbs");

    let mut column: Indexed<TextColumn, false> = Indexed::named("book.title");
    for row in 0..6000 {
        column
            .push(&format!(
                "a title far longer than twenty bytes, number {row}"
            ))
            .unwrap();
    }
    let file = committed(&path, |updater| column.offer_to("book", "title", updater));
    drop(file);

    let mut read: Indexed<TextColumn, false> = Indexed::named("book.title");
    read.read_from("book", "title", &ColumnFile::open(&path).unwrap())
        .unwrap();

    assert_eq!(
        found(&read, "a title far longer than twenty bytes, number 4000"),
        [4000]
    );

    let mut warm = Vec::new();
    read.residency("book", "title", &mut warm);
    let held = |name: &str| {
        warm.iter()
            .find(|held| held.name == name)
            .expect("the column names this")
            .resident
    };
    assert_eq!(
        held("book.title"),
        0,
        "a key too long to carry still names where its bytes are"
    );
    assert_eq!(
        held("book.title.bytes"),
        1,
        "one chunk of the heap confirmed it"
    );
}

#[test]
fn two_values_that_begin_alike_are_still_two_values() {
    let column = titles(&["prefixed-one", "prefixed-two", "prefixed-one"]);

    assert_eq!(
        found(&column, "prefixed-one"),
        [0, 2],
        "a key short enough to carry its bytes is compared by all of them"
    );
    assert_eq!(found(&column, "prefixed-two"), [1]);
}

#[test]
fn a_column_no_two_rows_may_share_refuses_a_value_that_is_there_whatever_its_shape() {
    let mut column: Unique<TextColumn> = Indexed::named("book.title");
    column.push("Alice").unwrap();
    column.push("").unwrap();

    let err = column.refuse_duplicate("Alice").unwrap_err();
    assert!(err.to_string().contains("row 0"), "{err}");
    let err = column.refuse_duplicate("").unwrap_err();
    assert!(
        err.to_string().contains("row 1"),
        "a column with nothing in it still holds a value: {err}"
    );
    column.refuse_duplicate("Kokoro").unwrap();
}

#[test]
fn a_column_rows_may_share_lets_a_row_be_set_to_a_value_another_row_holds() {
    let mut column = titles(&["Alice", "Kokoro"]);

    column
        .set(at(&column, 1), "Alice")
        .expect("only a unique column refuses a value that is already there");

    assert_eq!(found(&column, "Alice"), [0, 1]);
    assert!(found(&column, "Kokoro").is_empty());
}

#[test]
fn an_optional_string_is_indexed_by_the_whole_of_a_value_too_long_to_carry() {
    let first = "an isbn-like value far longer than twenty bytes, one";
    let second = "an isbn-like value far longer than twenty bytes, two";
    let mut column: Indexed<NullableTextColumn, false> = Indexed::named("edition.isbn");
    for value in [Some(first), None, Some(second), Some(first)] {
        column.push(value).unwrap();
    }

    let rows = |value| -> Vec<usize> {
        column
            .find(value)
            .unwrap()
            .into_iter()
            .map(Row::index)
            .collect()
    };
    assert_eq!(rows(Some(first)), [0, 3]);
    assert_eq!(rows(Some(second)), [2]);
    assert_eq!(rows(None), [1]);
}

#[test]
fn an_optional_string_moved_off_a_row_leaves_no_trace_under_what_it_left() {
    let long = "an isbn-like value far longer than twenty bytes";
    let mut column: Indexed<NullableTextColumn, false> = Indexed::named("edition.isbn");
    for value in [Some(long), None] {
        column.push(value).unwrap();
    }
    let second = Row::of(1, 2, "edition.isbn").unwrap();

    column.set(second, Some(long)).unwrap();

    let rows = |value| -> Vec<usize> {
        column
            .find(value)
            .unwrap()
            .into_iter()
            .map(Row::index)
            .collect()
    };
    assert_eq!(rows(Some(long)), [0, 1]);
    assert!(rows(None).is_empty());
}

#[test]
fn an_index_over_an_optional_string_is_rebuilt_from_the_column_that_holds_it() {
    let long = "an isbn-like value far longer than twenty bytes";
    let mut column: Indexed<NullableTextColumn, false> = Indexed::named("edition.isbn");
    for value in [Some(long), None, Some("short")] {
        column.push(value).unwrap();
    }
    column.index = HashIndex::new("edition.isbn".to_owned());

    column.index.rebuild(&column.column).unwrap();

    assert_eq!(column.find(Some(long)).unwrap()[0].index(), 0);
    assert_eq!(column.find(None).unwrap()[0].index(), 1);
    assert_eq!(column.find(Some("short")).unwrap()[0].index(), 2);
    column.audit().unwrap();
}

#[test]
fn an_optional_number_keeps_its_index_across_a_commit_and_a_reopen() {
    let dir = TempDir::new("indexed_maybe");
    let path = dir.join("editions.dbs");

    let mut pages: Indexed<NullableColumn<u32>, false> = Indexed::named("edition.pages");
    for count in [Some(7u32), None, Some(7), Some(0)] {
        pages.push(count).unwrap();
    }
    let file = committed(&path, |updater| pages.offer_to("edition", "pages", updater));

    let mut read: Indexed<NullableColumn<u32>, false> = Indexed::named("edition.pages");
    read.read_from("edition", "pages", &file).unwrap();

    let rows = |value| -> Vec<usize> {
        read.find(value)
            .unwrap()
            .into_iter()
            .map(Row::index)
            .collect()
    };
    assert_eq!(rows(Some(7)), [0, 2]);
    assert_eq!(rows(None), [1]);
    assert_eq!(rows(Some(0)), [3]);
    read.audit().unwrap();
}

#[test]
fn an_optional_number_moved_off_a_row_leaves_no_trace_under_what_it_left() {
    let mut pages: Indexed<NullableColumn<u32>, false> = Indexed::named("edition.pages");
    for count in [Some(7u32), None] {
        pages.push(count).unwrap();
    }
    let first = Row::of(0, 2, "edition.pages").unwrap();

    pages.set(first, None).unwrap();

    assert!(pages.find(Some(7)).unwrap().is_empty());
    assert_eq!(pages.find(None).unwrap().len(), 2);
    assert_eq!(pages.column().get(first).unwrap(), None);
    pages.audit().unwrap();
}

#[test]
fn an_index_over_an_optional_number_is_rebuilt_from_the_column_that_holds_it() {
    let mut pages: Indexed<NullableColumn<u32>, false> = Indexed::named("edition.pages");
    for count in [Some(7u32), None, Some(0)] {
        pages.push(count).unwrap();
    }
    pages.index = HashIndex::new("edition.pages".to_owned());

    pages.index.rebuild(&pages.column).unwrap();

    assert_eq!(pages.find(Some(7)).unwrap()[0].index(), 0);
    assert_eq!(pages.find(None).unwrap()[0].index(), 1);
    assert_eq!(pages.find(Some(0)).unwrap()[0].index(), 2);
    pages.audit().unwrap();
}

#[test]
fn a_variant_moved_off_a_row_leaves_no_trace_under_the_one_it_left() {
    let mut bindings: Indexed<EnumColumn<Binding>, false> = Indexed::named("edition.binding");
    for binding in [Binding::Cloth, Binding::Paper] {
        bindings.push(binding).unwrap();
    }
    let second = Row::of(1, 2, "edition.binding").unwrap();

    bindings.set(second, Binding::Leather).unwrap();

    assert!(bindings.find(Binding::Paper).unwrap().is_empty());
    assert_eq!(bindings.find(Binding::Leather).unwrap()[0].index(), 1);
    assert_eq!(bindings.column().get(second).unwrap(), Binding::Leather);
}

#[test]
fn an_index_over_a_variant_is_rebuilt_from_the_column_that_holds_it() {
    let mut bindings: Indexed<EnumColumn<Binding>, false> = Indexed::named("edition.binding");
    for binding in [Binding::Cloth, Binding::Paper, Binding::Cloth] {
        bindings.push(binding).unwrap();
    }
    bindings.index = HashIndex::new("edition.binding".to_owned());

    bindings.index.rebuild(&bindings.column).unwrap();

    let cloth: Vec<usize> = bindings
        .find(Binding::Cloth)
        .unwrap()
        .into_iter()
        .map(Row::index)
        .collect();
    assert_eq!(cloth, [0, 2]);
    assert_eq!(bindings.find(Binding::Paper).unwrap()[0].index(), 1);
    bindings.audit().unwrap();
}

#[test]
fn a_number_moved_off_a_row_leaves_no_trace_under_the_number_it_left() {
    let mut years: Indexed<Arena<u32>, false> = Indexed::named("edition.year");
    for year in [1865u32, 1914] {
        years.push(year).unwrap();
    }
    let second = Row::of(1, 2, "edition.year").unwrap();

    years.set(second, 1865).unwrap();

    assert!(years.find(1914).unwrap().is_empty());
    let both: Vec<usize> = years
        .find(1865)
        .unwrap()
        .into_iter()
        .map(Row::index)
        .collect();
    assert_eq!(both, [0, 1]);
    years.audit().unwrap();
}

#[test]
fn touching_the_tail_of_an_indexed_column_reaches_the_index_as_well_as_the_column() {
    let dir = TempDir::new("indexed_tail");
    let path = dir.join("books.dbs");

    let column = titles(&["Alice", "Kokoro"]);
    let file = committed(&path, |updater| column.offer_to("book", "title", updater));
    let tail = file.column_meta("book.title.links").unwrap().chunks[0];
    drop(file);

    flip_byte(&path, tail.offset);

    let mut read: Indexed<TextColumn, false> = Indexed::named("book.title");
    read.read_from("book", "title", &ColumnFile::open(&path).unwrap())
        .unwrap();

    let err = read.touch_tail().unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}

#[test]
fn making_room_in_an_indexed_column_reads_the_links_a_growth_would_rehash() {
    let dir = TempDir::new("indexed_room");
    let path = dir.join("books.dbs");

    let mut column: Indexed<TextColumn, false> = Indexed::named("book.title");
    for row in 0..4096 {
        column.push(&format!("title number {row}")).unwrap();
    }
    let file = committed(&path, |updater| column.offer_to("book", "title", updater));
    drop(file);

    let mut read: Indexed<TextColumn, false> = Indexed::named("book.title");
    read.read_from("book", "title", &ColumnFile::open(&path).unwrap())
        .unwrap();
    read.make_room(4097).unwrap();

    let mut held = Vec::new();
    read.residency("book", "title", &mut held);
    let links = held
        .iter()
        .find(|held| held.name == "book.title.links")
        .expect("the column names this");
    assert!(links.chunks > 1, "{links:?}");
    assert_eq!(links.resident, links.chunks, "{links:?}");
}

#[test]
fn an_optional_column_that_keys_by_a_digest_reads_a_row_back_before_it_says_it_matched() {
    let mut column = NullableTextColumn::default();
    let long = "an isbn-like value far longer than twenty bytes, one";
    let other = "an isbn-like value far longer than twenty bytes, two";
    let key = column.append(Some(long), None).unwrap();
    column.append(Some(other), None).unwrap();

    assert!(column.holds(key, Some(long)).unwrap());
    assert!(
        !column.holds(key, Some(other)).unwrap(),
        "two values of one length and one digest are still two values"
    );
    assert!(column.holds(TextKey::maybe(None), None).unwrap());
}

#[test]
fn a_unique_column_of_optional_numbers_lets_every_row_be_absent_but_none_agree() {
    let mut pages: Unique<NullableColumn<u32>> = Indexed::named("edition.pages");
    pages.push(Some(7u32)).unwrap();
    pages.push(None).unwrap();
    pages.push(None).unwrap();

    pages
        .refuse_duplicate(None)
        .expect("absence is not a value two rows may not share");
    pages.refuse_duplicate(Some(8)).unwrap();

    let err = pages.refuse_duplicate(Some(7)).unwrap_err();
    assert!(err.to_string().contains("edition.pages"), "{err}");
    assert!(err.to_string().contains("row 0"), "{err}");
}

#[test]
fn a_long_value_a_row_repeats_is_stored_once_and_not_again() {
    let long = "a title far longer than twenty bytes";
    let mut column: Indexed<TextColumn, false> = Indexed::named("book.title");

    column.push(long).unwrap();
    let after_first = column.column().heap_len();
    column.push(long).unwrap();
    column.push(long).unwrap();

    assert_eq!(after_first, long.len());
    assert_eq!(
        column.column().heap_len(),
        after_first,
        "three rows hold one value, so the file holds its bytes once"
    );
    assert_eq!(found(&column, long), [0, 1, 2]);
}

#[test]
fn a_short_value_a_row_repeats_is_stored_again_because_nothing_reads_it_back() {
    let mut column: Indexed<TextColumn, false> = Indexed::named("book.title");

    column.push("Alice").unwrap();
    let after_first = column.column().heap_len();
    column.push("Alice").unwrap();

    assert_eq!(
        column.column().heap_len(),
        after_first * 2,
        "a key that carries its own bytes is never confirmed, so sharing them buys nothing"
    );
    assert_eq!(found(&column, "Alice"), [0, 1]);
}

#[test]
fn a_long_value_set_onto_a_row_joins_the_bytes_the_other_rows_already_share() {
    let long = "a title far longer than twenty bytes";
    let mut column = titles(&[long, "Kokoro"]);
    let after_both = column.column().heap_len();

    column.set(at(&column, 1), long).unwrap();

    assert_eq!(
        column.column().heap_len(),
        after_both,
        "a row moved onto a value that is already there writes no bytes"
    );
    assert_eq!(found(&column, long), [0, 1]);
}

#[test]
fn a_long_value_many_rows_share_is_confirmed_from_one_chunk_of_the_heap() {
    let dir = TempDir::new("indexed_intern");
    let path = dir.join("books.dbs");

    let shared = "a title far longer than twenty bytes, shared by many rows";
    let mut column: Indexed<TextColumn, false> = Indexed::named("book.title");
    for row in 0..6000 {
        match row % 3 {
            0 => column.push(shared).unwrap(),
            _ => column
                .push(&format!(
                    "a title far longer than twenty bytes, number {row}"
                ))
                .unwrap(),
        };
    }
    let file = committed(&path, |updater| column.offer_to("book", "title", updater));
    drop(file);

    let mut read: Indexed<TextColumn, false> = Indexed::named("book.title");
    read.read_from("book", "title", &ColumnFile::open(&path).unwrap())
        .unwrap();

    assert_eq!(read.find(shared).unwrap().len(), 2000);

    let mut warm = Vec::new();
    read.residency("book", "title", &mut warm);
    let heap = warm
        .iter()
        .find(|held| held.name == "book.title.bytes")
        .expect("the column names this");
    assert!(heap.chunks > 1, "{heap:?}");
    assert_eq!(
        heap.resident, 1,
        "two thousand rows share one blob, so confirming them all faults one chunk"
    );
    assert_eq!(
        warm.iter()
            .find(|held| held.name == "book.title")
            .expect("the column names this")
            .resident,
        0
    );
}

#[test]
fn a_long_optional_value_a_row_repeats_is_stored_once_and_not_again() {
    let long = "an isbn-like value far longer than twenty bytes";
    let mut column: Indexed<NullableTextColumn, false> = Indexed::named("edition.isbn");

    column.push(Some(long)).unwrap();
    let after_first = column.column().heap_len();
    column.push(None).unwrap();
    column.push(Some(long)).unwrap();
    column.push(Some("short")).unwrap();

    assert_eq!(after_first, long.len());
    assert_eq!(
        column.column().heap_len(),
        after_first + "short".len(),
        "the repeat shared the bytes and the absent row needed none"
    );
    let rows: Vec<usize> = column
        .find(Some(long))
        .unwrap()
        .into_iter()
        .map(Row::index)
        .collect();
    assert_eq!(rows, [0, 2]);
    assert_eq!(column.find(None).unwrap()[0].index(), 1);
}

#[test]
fn a_push_onto_a_column_whose_rows_share_nothing_never_walks_the_bucket_it_lands_in() {
    let dir = TempDir::new("indexed_no_walk");
    let path = dir.join("editions.dbs");

    let mut years: Indexed<Arena<u32>, false> = Indexed::named("edition.year");
    for _ in 0..6000 {
        years.push(1865).unwrap();
    }
    let capacity = capacity_for(6000);
    let crowded = home_of(1865u32.raw(), capacity);
    let alongside = (1u32..200_000)
        .find(|year| *year != 1865 && home_of(year.raw(), capacity) == crowded)
        .expect("some year of two hundred thousand shares a bucket with 1865");

    let file = committed(&path, |updater| years.offer_to("edition", "year", updater));
    drop(file);

    let mut read: Indexed<Arena<u32>, false> = Indexed::named("edition.year");
    read.read_from("edition", "year", &ColumnFile::open(&path).unwrap())
        .unwrap();
    read.push(alongside).unwrap();

    let mut held = Vec::new();
    read.residency("edition", "year", &mut held);
    let links = held
        .iter()
        .find(|held| held.name == "edition.year.links")
        .expect("the column names this");
    assert!(links.chunks > 1, "{links:?}");
    assert_eq!(
        links.resident, 1,
        "a number carries itself, so a push has no bytes to look for and no chain to walk"
    );
}

#[test]
fn reaching_for_a_value_reads_the_bucket_a_push_of_it_would_write() {
    let dir = TempDir::new("indexed_reach");
    let path = dir.join("books.dbs");

    let mut column: Indexed<TextColumn, false> = Indexed::named("book.title");
    for row in 0..8193 {
        column.push(&format!("title number {row}")).unwrap();
    }
    let file = committed(&path, |updater| column.offer_to("book", "title", updater));
    drop(file);

    let mut read: Indexed<TextColumn, false> = Indexed::named("book.title");
    read.read_from("book", "title", &ColumnFile::open(&path).unwrap())
        .unwrap();
    let mut cold = Vec::new();
    read.residency("book", "title", &mut cold);
    assert!(cold.iter().all(|held| held.resident == 0), "{cold:?}");

    read.reach("a title no row holds").unwrap();

    let mut warm = Vec::new();
    read.residency("book", "title", &mut warm);
    let buckets = warm
        .iter()
        .find(|held| held.name == "book.title.buckets")
        .expect("the column names this");
    assert!(buckets.chunks > 1, "{buckets:?}");
    assert_eq!(
        buckets.resident, 1,
        "the bucket a push would write is the one bucket it reads before minting"
    );
    assert!(
        warm.iter()
            .filter(|held| held.name != "book.title.buckets")
            .all(|held| held.resident == 0),
        "and nothing else: {warm:?}"
    );
}

#[derive(Clone, Debug, PartialEq)]
struct Held {
    title: String,
    isbn: Option<String>,
    year: u32,
    pages: Option<u32>,
    binding: Binding,
}

#[derive(Clone)]
struct Shelf {
    title: Indexed<TextColumn, false>,
    isbn: Unique<NullableTextColumn>,
    year: Indexed<Arena<u32>, false>,
    pages: Indexed<NullableColumn<u32>, false>,
    binding: Indexed<EnumColumn<Binding>, false>,
}

impl Default for Shelf {
    fn default() -> Self {
        Self {
            title: Indexed::named("shelf.title"),
            isbn: Indexed::named("shelf.isbn"),
            year: Indexed::named("shelf.year"),
            pages: Indexed::named("shelf.pages"),
            binding: Indexed::named("shelf.binding"),
        }
    }
}

impl Shelf {
    fn every(&self) -> [(&'static str, &dyn Column); 5] {
        [
            ("shelf.title", &self.title),
            ("shelf.isbn", &self.isbn),
            ("shelf.year", &self.year),
            ("shelf.pages", &self.pages),
            ("shelf.binding", &self.binding),
        ]
    }
}

#[derive(Clone, Debug)]
enum Deed {
    Shelve(Held),
    Retitle(usize, String),
    Reissue(usize, Option<String>),
    Redate(usize, u32),
    Repaginate(usize, Option<u32>),
    Rebind(usize, Binding),
}

#[derive(Default)]
struct Ever {
    titles: Vec<String>,
    isbns: Vec<Option<String>>,
    years: Vec<u32>,
    pages: Vec<Option<u32>>,
}

fn bound() -> impl Strategy<Value = Binding> {
    prop_oneof![
        Just(Binding::Cloth),
        Just(Binding::Paper),
        Just(Binding::Leather),
    ]
}

fn counted() -> impl Strategy<Value = u32> {
    prop_oneof![
        3 => 0u32..4,
        1 => props::interesting_u32(),
    ]
}

fn held() -> impl Strategy<Value = Held> {
    (
        props::indexable_text(),
        props::indexable_optional_text(),
        counted(),
        proptest::option::of(counted()),
        bound(),
    )
        .prop_map(|(title, isbn, year, pages, binding)| Held {
            title,
            isbn,
            year,
            pages,
            binding,
        })
}

fn deed() -> impl Strategy<Value = Deed> {
    prop_oneof![
        6 => held().prop_map(Deed::Shelve),
        2 => (props::row_index(), props::indexable_text()).prop_map(|(at, title)| Deed::Retitle(at, title)),
        2 => (props::row_index(), props::indexable_optional_text()).prop_map(|(at, isbn)| Deed::Reissue(at, isbn)),
        1 => (props::row_index(), counted()).prop_map(|(at, year)| Deed::Redate(at, year)),
        1 => (props::row_index(), proptest::option::of(counted())).prop_map(|(at, pages)| Deed::Repaginate(at, pages)),
        1 => (props::row_index(), bound()).prop_map(|(at, binding)| Deed::Rebind(at, binding)),
    ]
}

fn deeds() -> impl Strategy<Value = Vec<Deed>> {
    prop::collection::vec(deed(), 0..24)
}

fn wrapped(at: usize, rows: usize) -> Option<usize> {
    (rows > 0).then(|| at % rows)
}

fn claimed(model: &[Held], isbn: &Option<String>, but: Option<usize>) -> bool {
    isbn.is_some()
        && model
            .iter()
            .enumerate()
            .any(|(row, held)| Some(row) != but && held.isbn == *isbn)
}

fn shelved(deeds: &[Deed]) -> (Shelf, Vec<Held>, Ever) {
    let mut shelf = Shelf::default();
    let mut model: Vec<Held> = Vec::new();
    let mut ever = Ever::default();

    for deed in deeds {
        match deed {
            Deed::Shelve(wanted) => {
                let taken = claimed(&model, &wanted.isbn, None);
                let refused = shelf.isbn.refuse_duplicate(wanted.isbn.as_deref()).is_err();
                assert_eq!(refused, taken, "{wanted:?} against {model:?}");
                if refused {
                    continue;
                }

                shelf.title.push(&wanted.title).unwrap();
                shelf.isbn.push(wanted.isbn.as_deref()).unwrap();
                shelf.year.push(wanted.year).unwrap();
                shelf.pages.push(wanted.pages).unwrap();
                shelf.binding.push(wanted.binding).unwrap();

                ever.titles.push(wanted.title.clone());
                ever.isbns.push(wanted.isbn.clone());
                ever.years.push(wanted.year);
                ever.pages.push(wanted.pages);
                model.push(wanted.clone());
            }
            Deed::Retitle(at, title) => {
                if let Some(row) = wrapped(*at, model.len()) {
                    shelf.title.set(Row::trusted(row), title).unwrap();
                    ever.titles.push(title.clone());
                    model[row].title = title.clone();
                }
            }
            Deed::Reissue(at, isbn) => {
                if let Some(row) = wrapped(*at, model.len()) {
                    let taken = claimed(&model, isbn, Some(row));
                    let said = shelf.isbn.set(Row::trusted(row), isbn.as_deref());
                    assert_eq!(said.is_err(), taken, "{isbn:?} onto row {row} of {model:?}");
                    if said.is_ok() {
                        ever.isbns.push(isbn.clone());
                        model[row].isbn = isbn.clone();
                    }
                }
            }
            Deed::Redate(at, year) => {
                if let Some(row) = wrapped(*at, model.len()) {
                    shelf.year.set(Row::trusted(row), *year).unwrap();
                    ever.years.push(*year);
                    model[row].year = *year;
                }
            }
            Deed::Repaginate(at, pages) => {
                if let Some(row) = wrapped(*at, model.len()) {
                    shelf.pages.set(Row::trusted(row), *pages).unwrap();
                    ever.pages.push(*pages);
                    model[row].pages = *pages;
                }
            }
            Deed::Rebind(at, binding) => {
                if let Some(row) = wrapped(*at, model.len()) {
                    shelf.binding.set(Row::trusted(row), *binding).unwrap();
                    model[row].binding = *binding;
                }
            }
        }
    }
    (shelf, model, ever)
}

fn seated(found: Vec<Row>) -> Vec<usize> {
    found.into_iter().map(Row::index).collect()
}

fn holding(model: &[Held], same: impl Fn(&Held) -> bool) -> Vec<usize> {
    model
        .iter()
        .enumerate()
        .filter(|(_, held)| same(held))
        .map(|(row, _)| row)
        .collect()
}

proptest! {
    #[test]
    fn every_indexed_column_names_exactly_the_rows_a_scan_of_it_would_name(
        deeds in deeds(),
        wanted in held(),
    ) {
        let (shelf, model, _) = shelved(&deeds);
        let mine = [wanted];

        for held in model.iter().chain(&mine) {
            let title = &held.title;
            prop_assert_eq!(
                seated(shelf.title.find(title).unwrap()),
                holding(&model, |row| row.title == *title),
                "shelf.title and a scan disagree about {:?}",
                title
            );

            let isbn = held.isbn.as_deref();
            prop_assert_eq!(
                seated(shelf.isbn.find(isbn).unwrap()),
                holding(&model, |row| row.isbn.as_deref() == isbn),
                "shelf.isbn and a scan disagree about {:?}",
                isbn
            );

            prop_assert_eq!(
                seated(shelf.year.find(held.year).unwrap()),
                holding(&model, |row| row.year == held.year),
                "shelf.year and a scan disagree about {}",
                held.year
            );

            prop_assert_eq!(
                seated(shelf.pages.find(held.pages).unwrap()),
                holding(&model, |row| row.pages == held.pages),
                "shelf.pages and a scan disagree about {:?}",
                held.pages
            );

            prop_assert_eq!(
                seated(shelf.binding.find(held.binding).unwrap()),
                holding(&model, |row| row.binding == held.binding),
                "shelf.binding and a scan disagree about {:?}",
                held.binding
            );
        }
    }

    #[test]
    fn no_value_a_row_has_left_ever_names_that_row_again(deeds in deeds()) {
        let (shelf, model, ever) = shelved(&deeds);

        for title in &ever.titles {
            prop_assert_eq!(
                seated(shelf.title.find(title).unwrap()),
                holding(&model, |row| row.title == *title),
                "shelf.title still answers for {:?}",
                title
            );
        }
        for isbn in &ever.isbns {
            let isbn = isbn.as_deref();
            prop_assert_eq!(
                seated(shelf.isbn.find(isbn).unwrap()),
                holding(&model, |row| row.isbn.as_deref() == isbn),
                "shelf.isbn still answers for {:?}",
                isbn
            );
        }
        for year in &ever.years {
            prop_assert_eq!(
                seated(shelf.year.find(*year).unwrap()),
                holding(&model, |row| row.year == *year)
            );
        }
        for pages in &ever.pages {
            prop_assert_eq!(
                seated(shelf.pages.find(*pages).unwrap()),
                holding(&model, |row| row.pages == *pages)
            );
        }
    }

    #[test]
    fn every_indexed_column_audits_clean_however_it_was_driven(deeds in deeds()) {
        let (shelf, model, _) = shelved(&deeds);

        for (name, column) in shelf.every() {
            prop_assert_eq!(Column::rows(column), model.len(), "{} holds another count", name);
            if let Err(said) = column.audit() {
                prop_assert!(false, "{} was left damaged: {}", name, said);
            }
        }
    }

    #[test]
    fn a_column_no_two_rows_may_share_refuses_exactly_the_values_it_already_holds(
        deeds in deeds(),
        wanted in props::indexable_optional_text(),
    ) {
        let (shelf, model, _) = shelved(&deeds);

        prop_assert_eq!(shelf.isbn.find_one(None).unwrap(), None);
        prop_assert!(shelf.isbn.refuse_duplicate(None).is_ok());
        prop_assert_eq!(
            seated(shelf.isbn.find(None).unwrap()),
            holding(&model, |row| row.isbn.is_none()),
            "an absent value is a key for the finder that names many rows"
        );

        for isbn in model.iter().map(|row| row.isbn.clone()).chain([wanted]) {
            let sought = isbn.as_deref();
            let held = holding(&model, |row| row.isbn.as_deref() == sought);
            let one = match sought {
                None => None,
                Some(_) => held.first().copied(),
            };

            prop_assert_eq!(
                shelf.isbn.find_one(sought).unwrap().map(Row::index),
                one,
                "shelf.isbn names another row for {:?}",
                sought
            );
            prop_assert_eq!(
                shelf.isbn.refuse_duplicate(sought).is_err(),
                one.is_some(),
                "shelf.isbn refuses the wrong values around {:?}",
                sought
            );
            prop_assert!(held.len() <= 1 || sought.is_none());
        }
    }

    #[test]
    fn an_audit_refuses_a_row_filed_under_any_value_but_the_one_it_holds(
        deeds in deeds(),
        at in props::row_index(),
        other in props::indexable_text(),
    ) {
        let (mut shelf, model, _) = shelved(&deeds);
        let Some(row) = wrapped(at, model.len()) else { return Ok(()); };

        shelf.title.index.repoint_row(row, TextKey::of(&other)).unwrap();
        let said = shelf.title.audit();

        prop_assert_eq!(
            said.is_err(),
            model[row].title != other,
            "row {} holds {:?} and was filed under {:?}",
            row,
            model[row].title,
            other
        );
        if let Err(damage) = said {
            prop_assert!(damage.is_corrupt(), "{}", damage);
            prop_assert!(damage.to_string().contains("shelf.title"), "{}", damage);
        }
    }

    #[test]
    fn a_long_value_a_column_already_holds_is_never_stored_twice(
        value in props::indexable_text(),
        again in 1usize..5,
        at in props::row_index(),
    ) {
        let mut column: Indexed<TextColumn, false> = Indexed::named("shelf.title");
        column.push(&value).unwrap();
        let once = column.column().heap_len();

        for _ in 0..again {
            column.push(&value).unwrap();
        }
        let shared = column.column().heap_len();

        match !TextKey::of(&value).is_inline() {
            true => prop_assert_eq!(
                shared,
                once,
                "{:?} is too long to carry, so every row must share one copy",
                value
            ),
            false => prop_assert_eq!(shared, once + again * value.len()),
        }

        let row = at % Column::rows(&column);
        column.set(Row::trusted(row), &value).unwrap();
        if !TextKey::of(&value).is_inline() {
            prop_assert_eq!(column.column().heap_len(), shared, "a set joins the bytes too");
        }
        prop_assert_eq!(
            seated(column.find(&value).unwrap()),
            (0..=again).collect::<Vec<usize>>()
        );
    }
}

fn after_a_cold_find(values: &[String], sought: &str, named: &str) -> Vec<Residency> {
    let dir = TempDir::new(named);
    let path = dir.join("shelf.dbs");

    let mut column: Indexed<TextColumn, false> = Indexed::named("shelf.title");
    for value in values {
        column.push(value).unwrap();
    }
    let file = committed(&path, |updater| column.offer_to("shelf", "title", updater));
    drop(file);

    let mut read: Indexed<TextColumn, false> = Indexed::named("shelf.title");
    read.read_from("shelf", "title", &ColumnFile::open(&path).unwrap())
        .unwrap();

    let mut cold = Vec::new();
    read.residency("shelf", "title", &mut cold);
    assert!(
        cold.iter().all(|held| held.resident == 0),
        "opening a column reads no payload: {cold:?}"
    );

    read.find(sought).unwrap();

    let mut warm = Vec::new();
    read.residency("shelf", "title", &mut warm);
    warm
}

fn resident(reported: &[Residency], name: &str) -> usize {
    reported
        .iter()
        .find(|held| held.name == name)
        .expect("the column names this")
        .resident
}

proptest! {
    #![proptest_config(props::scaled(8))]

    #[test]
    fn finding_a_value_never_reads_a_byte_of_the_column_that_holds_it(
        values in prop::collection::vec(props::indexable_text(), 40..90),
        at in 0usize..90,
    ) {
        let sought = values[at % values.len()].clone();
        let warm = after_a_cold_find(&values, &sought, "indexed_scan");

        prop_assert!(
            resident(&warm, "shelf.title.buckets") > 0,
            "a find that read no bucket never looked: {warm:?}"
        );
        prop_assert_eq!(
            resident(&warm, "shelf.title"),
            0,
            "the key answers for {:?}, so the column it indexes is never read",
            sought
        );
    }

    #[test]
    fn finding_a_short_value_never_reads_the_heap_and_finding_a_long_one_always_does(
        values in prop::collection::vec(props::indexable_text(), 40..90),
        at in 0usize..90,
    ) {
        let sought = values[at % values.len()].clone();
        let warm = after_a_cold_find(&values, &sought, "indexed_heap");
        let heap = resident(&warm, "shelf.title.bytes");

        match TextKey::of(&sought).is_inline() {
            true => prop_assert_eq!(
                heap,
                0,
                "{:?} is carried whole by its key, so the heap holds nothing to read",
                sought
            ),
            false => prop_assert!(
                heap > 0,
                "{:?} is only named by its key, so the row must be read back: {warm:?}",
                sought
            ),
        }
    }
}
