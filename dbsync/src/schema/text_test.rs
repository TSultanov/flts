use proptest::prelude::*;

use super::*;
use crate::storage::ColumnFile;
use crate::test_utils::{TempDir, committed, props};

fn row(index: usize, rows: usize) -> Row {
    Row::of(index, rows, "book").unwrap()
}

#[test]
fn a_text_column_reads_back_every_string_it_was_given() {
    let mut titles = TextColumn::default();
    titles.push("Le Petit Prince").unwrap();
    titles.push("").unwrap();
    titles.push("こころ").unwrap();

    assert_eq!(titles.rows(), 3);
    assert_eq!(titles.get(row(0, 3)).unwrap(), "Le Petit Prince");
    assert_eq!(titles.get(row(1, 3)).unwrap(), "");
    assert_eq!(titles.get(row(2, 3)).unwrap(), "こころ");
}

#[test]
fn overwriting_a_string_appends_and_leaves_the_old_bytes_behind() {
    let mut titles = TextColumn::default();
    titles.push("first").unwrap();
    let before = titles.heap_len();

    titles.set(row(0, 1), "a longer second").unwrap();

    assert_eq!(titles.get(row(0, 1)).unwrap(), "a longer second");
    assert_eq!(
        titles.heap_len(),
        before + "a longer second".len(),
        "the heap only ever grows; the old bytes are garbage, not reclaimed"
    );
}

#[test]
fn a_text_column_asks_for_a_blob_column_and_a_heap_named_after_it() {
    let mut specs = Vec::new();
    TextColumn::default().describe("book", "title", &mut specs);

    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].name, "book.title");
    assert_eq!(specs[0].kind, ColumnKind::Text);
    assert!(specs[0].counts_rows);
    assert_eq!(specs[1].name, "book.title.bytes");
    assert_eq!(specs[1].kind, ColumnKind::Heap);
    assert!(
        !specs[1].counts_rows,
        "a heap is measured in bytes, so it must not be compared against a row count"
    );
}

#[test]
fn a_text_column_survives_a_commit_and_a_reopen() {
    let dir = TempDir::new("text");
    let path = dir.join("titles.dbs");

    let mut titles = TextColumn::default();
    titles.push("Alice").unwrap();
    titles.push("Kokoro").unwrap();
    let file = committed(&path, |updater| titles.offer_to("book", "title", updater));

    let mut read = TextColumn::default();
    read.read_from("book", "title", &file).unwrap();

    assert_eq!(read.rows(), 2);
    assert_eq!(read.get(row(1, 2)).unwrap(), "Kokoro");
    assert!(file.column_meta("book.title").is_some());
    assert!(file.column_meta("book.title.bytes").is_some());
}

#[test]
fn a_nullable_text_column_tells_an_absent_string_from_an_empty_one() {
    let mut notes = NullableTextColumn::default();
    notes.push(Some("a note")).unwrap();
    notes.push(None).unwrap();
    notes.push(Some("")).unwrap();

    assert_eq!(notes.get(row(0, 3)).unwrap().as_deref(), Some("a note"));
    assert_eq!(notes.get(row(1, 3)).unwrap().as_deref(), None);
    assert_eq!(
        notes.get(row(2, 3)).unwrap().as_deref(),
        Some(""),
        "an empty string is present and empty, not absent"
    );
}

#[test]
fn an_absent_string_can_be_given_a_value_and_taken_away_again() {
    let mut notes = NullableTextColumn::default();
    notes.push(None).unwrap();

    notes.set(row(0, 1), Some("later")).unwrap();
    assert_eq!(notes.get(row(0, 1)).unwrap().as_deref(), Some("later"));

    notes.set(row(0, 1), None).unwrap();
    assert_eq!(notes.get(row(0, 1)).unwrap().as_deref(), None);
}

#[test]
fn an_absent_string_costs_no_bytes_in_the_heap() {
    let mut notes = NullableTextColumn::default();
    notes.push(None).unwrap();
    notes.push(None).unwrap();

    assert_eq!(notes.heap_len(), 0);
    assert_eq!(notes.rows(), 2, "but it still occupies a row");

    notes.push(Some("paid for")).unwrap();
    assert_eq!(
        notes.heap_len(),
        "paid for".len(),
        "only a string that is there spends anything"
    );
}

#[test]
fn a_nullable_text_column_asks_for_no_presence_column() {
    let mut specs = Vec::new();
    NullableTextColumn::default().describe("card", "note", &mut specs);

    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].name, "card.note");
    assert_eq!(
        specs[0].kind,
        ColumnKind::Nullable(Box::new(ColumnKind::Text))
    );
    assert_eq!(specs[1].name, "card.note.bytes");
    assert!(
        !specs.iter().any(|spec| spec.name.ends_with(".present")),
        "a blob has a niche of its own, so nullability needs no second column"
    );
}

#[test]
fn a_nullable_text_column_survives_a_commit_and_a_reopen() {
    let dir = TempDir::new("nullable_text");
    let path = dir.join("notes.dbs");

    let mut notes = NullableTextColumn::default();
    notes.push(Some("kept")).unwrap();
    notes.push(None).unwrap();
    notes.push(Some("also kept")).unwrap();
    let file = committed(&path, |updater| notes.offer_to("card", "note", updater));

    let mut read = NullableTextColumn::default();
    read.read_from("card", "note", &file).unwrap();

    assert_eq!(read.rows(), 3);
    assert_eq!(read.get(row(0, 3)).unwrap().as_deref(), Some("kept"));
    assert_eq!(read.get(row(1, 3)).unwrap().as_deref(), None);
    assert_eq!(read.get(row(2, 3)).unwrap().as_deref(), Some("also kept"));
}

#[test]
fn touching_the_tail_of_a_rotten_nullable_heap_reports_the_damage() {
    let dir = TempDir::new("nullable_text_tail");
    let path = dir.join("notes.dbs");

    let mut notes = NullableTextColumn::default();
    notes.push(Some("a note")).unwrap();
    let file = committed(&path, |updater| notes.offer_to("card", "note", updater));
    let tail = file.column_meta("card.note.bytes").unwrap().chunks[0];
    drop(file);
    crate::test_utils::flip_byte(&path, tail.offset);

    let mut read = NullableTextColumn::default();
    read.read_from("card", "note", &ColumnFile::open(&path).unwrap())
        .unwrap();

    let err = read.touch_tail().unwrap_err();
    assert!(
        err.is_corrupt(),
        "a blob column that reads cleanly does not excuse the heap it points into: {err}"
    );
}

proptest! {
    #[test]
    fn two_values_make_one_key_only_when_they_are_the_same_value(
        left in props::indexable_text(),
        right in props::indexable_text(),
    ) {
        let one_key = TextKey::of(&left) == TextKey::of(&right);
        prop_assert_eq!(
            one_key,
            left == right,
            "the key says {} and the value says {} for {:?} and {:?}",
            one_key,
            left == right,
            left,
            right
        );
    }

    #[test]
    fn every_value_makes_the_same_key_wherever_its_bytes_were_put(
        value in props::indexable_text(),
        here in props::interesting_u64(),
        there in props::interesting_u64(),
    ) {
        let span = value.len() as u32;
        let one = TextKey::stored(&value, Blob::new(here, span));
        let other = TextKey::stored(&value, Blob::new(there, span));

        prop_assert_eq!(one, other, "where {:?} was put is not which value it is", value);
        prop_assert_eq!(one, TextKey::of(&value));
        prop_assert_eq!(one.raw(), other.raw());
        prop_assert_eq!(one, TextKey::maybe(Some(&value)));
    }

    #[test]
    fn a_key_carries_its_value_whole_or_names_the_heap_that_holds_it(
        value in props::indexable_text(),
        start in props::interesting_u64(),
    ) {
        let span = value.len() as u32;
        let key = TextKey::stored(&value, Blob::new(start, span));

        prop_assert!(!key.is_absent());
        prop_assert_eq!(key.is_inline(), value.len() <= INLINE);
        prop_assert_ne!(key, TextKey::ABSENT);

        if !key.is_inline() {
            prop_assert_eq!(
                key.blob(),
                Blob::new(start, span),
                "a key too long to carry {:?} must name where it was put",
                value
            );
        }
    }

    #[test]
    fn every_twenty_four_bytes_decode_as_a_key_and_re_encode_unchanged(
        bytes in prop::collection::vec(any::<u8>(), <TextKey as Record>::SIZE),
    ) {
        let key = TextKey::decode(&bytes);
        let mut written = vec![0u8; <TextKey as Record>::SIZE];
        key.encode(&mut written);

        prop_assert_eq!(&written, &bytes);
        prop_assert_eq!(key, TextKey::decode(&bytes));
        prop_assert_eq!(key.raw(), TextKey::decode(&bytes).raw());
        prop_assert_eq!(key.blob().len, key.len);
    }
}

#[test]
fn heap_compaction_reclaims_replaced_bytes_and_preserves_nulls_and_sharing() -> Result<()> {
    let mut text = TextColumn::default();
    text.push("old unused bytes")?;
    text.set(row(0, 1), "retained")?;
    text.push("retained")?;
    assert_eq!(text.unused_heap_bytes()?, "old unused bytes".len() as u64);
    let snapshot = text.clone();
    text.compact_storage()?;
    assert_eq!(text.heap_len(), "retained".len());
    assert_eq!(text.unused_heap_bytes()?, 0);
    assert_eq!(text.get(row(0, 2))?, snapshot.get(row(0, 2))?);
    assert_eq!(text.get(row(1, 2))?, "retained");

    let mut optional = NullableTextColumn::default();
    optional.push(Some("gone"))?;
    optional.push(Some("retained"))?;
    optional.push(Some("retained"))?;
    optional.push(Some(""))?;
    optional.set(row(0, 4), None)?;
    assert_eq!(optional.unused_heap_bytes()?, 4);
    optional.compact_storage()?;
    assert_eq!(optional.heap_len(), "retained".len());
    assert_eq!(optional.get(row(0, 4))?, None);
    assert_eq!(optional.get(row(3, 4))?.as_deref(), Some(""));
    assert_eq!(optional.unused_heap_bytes()?, 0);
    Ok(())
}
