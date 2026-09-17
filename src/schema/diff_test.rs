use super::*;
use crate::schema::{ColumnKind, Store};
use crate::storage::{Arena, WriterId};
use crate::test_utils::TempDir;
use crate::test_utils::library::Library;

fn spec(name: &str, elem_size: usize) -> ColumnSpec {
    ColumnSpec {
        name: name.to_owned(),
        elem_size,
        kind: ColumnKind::Scalar,
        counts_rows: true,
    }
}

fn this_device() -> WriterId {
    WriterId::from_device_id("KRZH3XY-7PH4NJK-2WQ5LMC")
}

fn a_library(dir: &TempDir) -> (std::path::PathBuf, Library) {
    let path = dir.join("library.dbs");
    let mut library = Library::create(&path, this_device()).unwrap();
    library.books.push("Alice", "Lewis Carroll").unwrap();
    library.commit(this_device()).unwrap();
    (path, library)
}

#[test]
fn a_file_written_by_this_schema_diffs_clean() {
    let dir = TempDir::new("clean");
    let (path, library) = a_library(&dir);

    let diff = library.diff(&ColumnFile::open(&path).unwrap());

    assert!(diff.is_clean(), "{diff:?}");
    assert!(diff.to_error().is_none());
}

#[test]
fn every_missing_column_is_named_in_one_message_not_one_at_a_time() {
    let dir = TempDir::new("missing");
    let (path, library) = a_library(&dir);
    let file = ColumnFile::open(&path).unwrap();

    let mut wanted = library.specs();
    wanted.push(spec("book.isbn", 8));
    wanted.push(spec("book.dewey", 4));

    let diff = SchemaDiff::of(&wanted, &file);
    assert_eq!(diff.missing, ["book.dewey", "book.isbn"]);

    let err = diff.to_error().expect("a missing column is a mismatch");
    assert!(
        err.to_string()
            .contains(r#"file is missing columns ["book.dewey", "book.isbn"]"#),
        "{err}"
    );
}

#[test]
fn a_column_of_the_wrong_width_is_reported_with_both_widths() {
    let dir = TempDir::new("width");
    let (path, library) = a_library(&dir);
    let file = ColumnFile::open(&path).unwrap();

    let wanted: Vec<ColumnSpec> = library
        .specs()
        .into_iter()
        .map(|mut held| {
            if held.name == "book.title" {
                held.elem_size = 8;
            }
            held
        })
        .collect();

    let err = SchemaDiff::of(&wanted, &file)
        .to_error()
        .expect("a changed width is a mismatch");

    assert!(
        err.to_string()
            .contains(r#"column "book.title" holds 12-byte rows, the schema wants 8"#),
        "{err}"
    );
}

#[test]
fn a_column_the_schema_does_not_name_is_reported_but_is_not_a_mismatch() {
    let dir = TempDir::new("unknown");
    let (path, library) = a_library(&dir);

    let file = ColumnFile::open(&path).unwrap();
    let mut extra = Arena::<u64>::new();
    extra.extend_from_slice(&[1]).unwrap();
    let mut updater = file.update(this_device()).unwrap();
    for column in file.columns() {
        updater.keep_column(&column.name).unwrap();
    }
    updater.column("book.isbn", &extra).unwrap();
    updater.commit().unwrap();

    let diff = library.diff(&ColumnFile::open(&path).unwrap());

    assert_eq!(diff.unknown, ["book.isbn"]);
    assert!(!diff.is_clean());
    assert!(
        diff.to_error().is_none(),
        "a newer peer's column is carried, not refused"
    );
}

#[test]
fn one_message_reports_what_is_missing_what_changed_and_what_is_extra() {
    let dir = TempDir::new("everything");
    let (path, library) = a_library(&dir);
    let file = ColumnFile::open(&path).unwrap();

    let mut wanted: Vec<ColumnSpec> = library
        .specs()
        .into_iter()
        .filter(|held| held.name != "book.author")
        .map(|mut held| {
            if held.name == "book.title" {
                held.elem_size = 8;
            }
            held
        })
        .collect();
    wanted.push(spec("book.isbn", 8));

    let err = SchemaDiff::of(&wanted, &file)
        .to_error()
        .expect("this schema does not match this file");
    let said = err.to_string();

    assert!(said.contains(r#"missing columns ["book.isbn"]"#), "{said}");
    assert!(
        said.contains(r#""book.title" holds 12-byte rows"#),
        "{said}"
    );
    assert!(said.contains(r#"does not name: ["book.author"]"#), "{said}");
}

#[test]
fn opening_a_file_that_does_not_match_the_schema_says_so_before_reading_a_row() {
    let dir = TempDir::new("mismatch_open");
    let path = dir.join("partial.dbs");

    let mut only_books = Arena::<u8>::new();
    only_books.push_str("Alice").unwrap();
    crate::storage::ColumnFileWriter::create(&path, this_device())
        .unwrap()
        .finish()
        .unwrap();
    let file = ColumnFile::open(&path).unwrap();
    let mut updater = file.update(this_device()).unwrap();
    updater.column("book.title.bytes", &only_books).unwrap();
    updater.commit().unwrap();

    let opened = Library::open(&path);
    let err = match opened {
        Ok(_) => panic!("a file missing nine of ten columns must not open"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("file is missing columns"), "{err}");
    assert!(err.to_string().contains("paragraph.text"), "{err}");
    assert!(
        err.to_string().contains("book.author"),
        "every missing column at once, not the first one it tried: {err}"
    );
}

fn derived(name: &str, kind: ColumnKind) -> ColumnSpec {
    ColumnSpec {
        name: name.to_owned(),
        elem_size: 8,
        kind,
        counts_rows: false,
    }
}

#[test]
fn a_column_an_index_derives_is_not_a_column_a_file_has_to_have() {
    let dir = TempDir::new("derived");
    let (path, _library) = a_library(&dir);
    let file = ColumnFile::open(&path).unwrap();

    let diff = SchemaDiff::of(
        &[
            spec("book.title", 12),
            derived("book.author.buckets", ColumnKind::Bucket),
            derived("book.author.links", ColumnKind::Link),
        ],
        &file,
    );

    assert_eq!(diff.derived, ["book.author.buckets", "book.author.links"]);
    assert!(diff.missing.is_empty(), "{diff:?}");
    assert!(
        diff.to_error().is_none(),
        "an index is rebuilt from the column it indexes, so its absence is not a mismatch"
    );
    assert!(!diff.is_clean(), "a file that needs work is not yet clean");
}

#[test]
fn a_derived_column_of_the_wrong_width_is_rebuilt_where_a_stored_one_is_refused() {
    let dir = TempDir::new("derived_width");
    let (path, _library) = a_library(&dir);
    let file = ColumnFile::open(&path).unwrap();

    let diff = SchemaDiff::of(
        &[
            derived("book.id.links", ColumnKind::Link),
            spec("book.id", 8),
        ],
        &file,
    );

    assert_eq!(diff.derived, ["book.id.links"]);
    assert_eq!(diff.widths, [("book.id".to_owned(), 16, 8)]);
    let err = diff.to_error().expect("a stored column of the wrong width");
    assert!(err.to_string().contains("book.id"), "{err}");
    assert!(
        !err.to_string().contains("book.id.links"),
        "a link of the wrong width is rewritten, not reported: {err}"
    );
}
