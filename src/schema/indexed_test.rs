use super::*;
use crate::record::Record;
use crate::schema::{
    EnumColumn, IndexKey, Maybe, NullableColumn, NullableTextColumn, TextColumn, TextKey,
};
use crate::storage::{Blob, ColumnFile};
use crate::test_utils::library::Binding;
use crate::test_utils::{TempDir, committed, flip_byte};

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
    let key = column.append(long).unwrap();
    column.append(other).unwrap();

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
    let key = column.append(Some(long)).unwrap();
    column.append(Some(other)).unwrap();

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
