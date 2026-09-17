use super::*;
use crate::test_utils::{TempDir, committed};

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
