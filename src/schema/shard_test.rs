use super::*;
use crate::test_utils::TempDir;

fn writer() -> WriterId {
    WriterId::from_raw(0xabc)
}

fn row(counter: u64, parents: &[u64], title: &str) -> ShardRow {
    ShardRow {
        id: Uuid::from_u128(counter as u128 + 100),
        version: Version::of(Dot::new(writer(), counter)),
        parents: parents
            .iter()
            .map(|counter| Dot::new(writer(), *counter))
            .collect(),
        values: vec![
            ("title".to_owned(), title.as_bytes().to_vec()),
            ("pages".to_owned(), vec![counter as u8; 4]),
        ],
    }
}

#[test]
fn a_shard_is_named_for_its_writer_and_schema_version() {
    let path = shard_path(Path::new("/s"), "book", writer(), 3);
    assert_eq!(path, Path::new("/s/book/0000000000000abc.v3.dbs"));

    let found = named("book", &path).unwrap();
    assert_eq!(found.writer, writer());
    assert_eq!(found.schema_version, 3);
    assert_eq!(found.entity, "book");
}

#[test]
fn a_file_that_is_not_a_shard_is_not_taken_for_one() {
    for name in [
        "0000000000000abc.v3.sync-conflict-20260101.dbs",
        "0000000000000abc.dbs",
        "abc.v3.dbs",
        "0000000000000ABC.v3.dbs",
        "0000000000000000.v3.dbs",
        "0000000000000abc.v0.dbs",
        "0000000000000abc.v3.tmp",
        ".syncthing.0000000000000abc.v3.dbs.tmp",
    ] {
        assert!(
            named("book", &Path::new("/s/book").join(name)).is_none(),
            "{name}"
        );
    }
}

#[test]
fn appended_rows_read_back_and_a_later_read_takes_only_the_new_ones() {
    let dir = TempDir::new("shard_append");
    let path = shard_path(&dir.path, "book", writer(), 1);
    let fields = ["title", "pages"];

    let (first, held) = append(&path, writer(), b"schema", &fields, &[row(1, &[], "one")]).unwrap();
    assert_eq!(held, 1);
    let (second, held) = append(
        &path,
        writer(),
        b"schema",
        &fields,
        &[row(2, &[1], "two"), row(3, &[1, 2], "three")],
    )
    .unwrap();
    assert_eq!(held, 3);
    assert_ne!(first, second);
    assert_eq!(token_of(&path).unwrap(), second);

    let all = read(&path, 0).unwrap();
    assert_eq!(all.rows, 3);
    assert_eq!(all.schema, b"schema");
    assert_eq!(
        all.read,
        [
            row(1, &[], "one"),
            row(2, &[1], "two"),
            row(3, &[1, 2], "three")
        ]
    );

    let tail = read(&path, 2).unwrap();
    assert_eq!(tail.read, [row(3, &[1, 2], "three")]);
    assert!(read(&path, 3).unwrap().read.is_empty());
    assert!(read(&path, 4).is_err());
}

#[test]
fn a_field_the_writer_learned_later_is_absent_from_its_earlier_rows() {
    let dir = TempDir::new("shard_grow");
    let path = shard_path(&dir.path, "book", writer(), 1);
    append(
        &path,
        writer(),
        b"one",
        &["title", "pages"],
        &[row(1, &[], "one")],
    )
    .unwrap();

    let mut grown = row(2, &[], "two");
    grown.values.push(("isbn".to_owned(), vec![7]));
    append(
        &path,
        writer(),
        b"two",
        &["title", "pages", "isbn"],
        &[grown.clone()],
    )
    .unwrap();

    let all = read(&path, 0).unwrap();
    assert_eq!(all.schema, b"two");
    assert_eq!(all.read[0].value("isbn"), None);
    assert_eq!(all.read[1].value("isbn"), Some(&[7u8][..]));
    assert_eq!(all.read[0].value("title"), Some(&b"one"[..]));
}

#[test]
fn a_shard_refuses_rows_that_are_not_its_writers() {
    let dir = TempDir::new("shard_foreign");
    let path = shard_path(&dir.path, "book", writer(), 1);
    let mut foreign = row(1, &[], "one");
    foreign.version = Version::of(Dot::new(WriterId::from_raw(9), 1));
    assert!(append(&path, writer(), b"", &["title", "pages"], &[foreign]).is_err());

    let mut pending = row(1, &[], "one");
    pending.version = Version::UNSTAMPED;
    assert!(append(&path, writer(), b"", &["title", "pages"], &[pending]).is_err());
}

#[test]
fn discovery_separates_shards_from_whatever_else_is_in_the_directory() {
    let dir = TempDir::new("shard_discover");
    let ours = shard_path(&dir.path, "book", writer(), 1);
    let theirs = shard_path(&dir.path, "book", WriterId::from_raw(2), 1);
    append(
        &ours,
        writer(),
        b"",
        &["title", "pages"],
        &[row(1, &[], "one")],
    )
    .unwrap();
    std::fs::copy(&ours, &theirs).unwrap();
    let conflict = dir
        .path
        .join("book")
        .join("0000000000000abc.v1.sync-conflict.dbs");
    std::fs::write(&conflict, b"junk").unwrap();

    let (found, strangers) = discover(&dir.path, "book").unwrap();
    assert_eq!(
        found
            .iter()
            .map(|shard| shard.writer.raw())
            .collect::<Vec<_>>(),
        [2, 0xabc]
    );
    assert_eq!(strangers, [conflict]);
    assert!(discover(&dir.path, "chapter").unwrap().0.is_empty());
}
