use super::*;
use crate::WriterId;
use crate::test_utils::TempDir;

mod v1 {
    #[crate::entity]
    pub struct Book {
        #[index]
        title: String,
        pages: u32,
    }
    #[crate::entity]
    pub struct Chapter {
        book: One<Book>,
        title: String,
    }
    #[crate::entity]
    pub struct Retired {
        text: String,
    }
    #[crate::container(schema = "library", version = 1)]
    pub struct Library {
        books: Set<Book>,
        chapters: Set<Chapter>,
        retired: Set<Retired>,
    }
}
mod v2 {
    #[crate::entity]
    pub struct Volume {
        #[index]
        name: String,
        pages: Option<u64>,
    }
    #[crate::entity]
    pub struct Chapter {
        volume: One<Volume>,
        title: String,
    }
    #[crate::entity]
    pub struct Note {
        text: String,
    }
    #[crate::container(schema = "library", version = 2)]
    pub struct Library {
        books: Set<Volume>,
        chapters: Set<Chapter>,
        notes: Set<Note>,
    }
}
mod v3 {
    use super::v2::{Chapter, Note, Volume};
    #[crate::container(schema = "library", version = 3)]
    pub struct Library {
        books: Set<Volume>,
        chapters: Set<Chapter>,
        notes: Set<Note>,
    }
}
fn book(value: v1::BookValue) -> Result<v2::VolumeValue> {
    Ok(v2::VolumeValue {
        name: value.title,
        pages: Some(value.pages as u64),
    })
}
fn upgrade() -> Migration<v1::Library, v2::Library> {
    Migration::new()
        .map::<v1::Book, v2::Volume>(book)
        .map::<v1::Chapter, v2::Chapter>(|v| {
            Ok(v2::ChapterValue {
                volume: v.book.cast(),
                title: v.title,
            })
        })
        .drop::<v1::Retired>()
        .add::<v2::Note>()
}
fn next() -> Migration<v2::Library, v3::Library> {
    Migration::new()
        .map::<v2::Volume, v2::Volume>(Ok)
        .map::<v2::Chapter, v2::Chapter>(Ok)
        .map::<v2::Note, v2::Note>(Ok)
}
fn registry() -> Migrations<v2::Library> {
    Migrations::new().step(upgrade())
}
fn writer(n: u64) -> WriterId {
    WriterId::from_raw(n)
}
fn seed(path: &Path) -> Result<(v1::Library, Id<v1::Book>)> {
    let mut old = v1::Library::create(path, writer(1))?;
    let id = old.books.push("Alice", 100)?;
    old.chapters.push(id, "Down the Rabbit-Hole")?;
    old.retired.push("obsolete")?;
    old.commit(writer(1))?;
    Ok((old, id))
}
fn ancestry<T: Table>(table: &T) -> Result<Vec<(crate::Uuid, Version, Vec<Parent>)>> {
    (0..Table::rows(table))
        .map(|i| {
            let row = Row::trusted(i);
            Ok((
                table.id_at(row)?.uuid(),
                table.versions().raw_at(row)?,
                table.versions().parents(row)?.to_vec(),
            ))
        })
        .collect()
}

#[test]
fn chain_preserves_history_references_clocks_and_snapshots() -> Result<()> {
    let dir = TempDir::new("migrate_history");
    let path = dir.join("library");
    let (mut old, id) = seed(&path)?;
    let original_dot = old.books.versions().raw_at(Row::trusted(0))?.dot();
    old.books.set_title(id, "Alice in Wonderland")?;
    old.commit(writer(1))?;
    old.books.remove(id)?;
    old.commit(writer(1))?;
    old.restore(id, original_dot)?;
    old.commit(writer(1))?;
    let deleted = old.books.push("Gone", 7)?;
    old.commit(writer(1))?;
    old.books.remove(deleted)?;
    old.commit(writer(1))?;
    let records = ancestry(&old.books)?;
    let clock = old.clock().clone();
    let token = ColumnFile::read_commit_token(&path)?;
    let pinned = old.snapshot();
    let migrations = Migrations::<v3::Library>::new()
        .step(next())
        .step(upgrade());
    let (mut new, report) = migrations.migrate(&path)?;
    assert_eq!(report.steps.len(), 2);
    assert_eq!(report.steps[0].dropped_revisions, 1);
    assert_eq!(report.steps[0].converted_revisions, records.len() + 1);
    assert_eq!(new.clock(), &clock);
    assert_eq!(ancestry(&new.books)?, records);
    assert_eq!(new.books.name(id.cast())?, "Alice");
    assert_eq!(new.books.pages(id.cast())?, Some(100));
    assert_eq!(new.books.find_by_name("Alice")?, vec![id.cast()]);
    assert_eq!(new.books.versions_of(deleted.cast())?.len(), 2);
    assert!(new.books.name(deleted.cast()).is_err());
    let chapter = new.chapters.id_at(Row::trusted(0))?;
    assert_eq!(new.chapters.volume(chapter)?.uuid(), id.uuid());
    assert_eq!(Table::rows(&new.notes), 0);
    new.verify()?;
    assert_eq!(pinned.books.title(id)?, "Alice");
    let published = ColumnFile::read_commit_token(&path)?;
    assert_eq!(published.generation, token.generation + 1);
    assert_ne!(published.file_uuid, token.file_uuid);
    assert_eq!(SchemaMetadata::inspect(&path)?.version, 3);
    assert!(matches!(
        old.commit(writer(1)),
        Err(Error::SchemaMismatch { .. })
    ));
    let (_, repeated) = migrations.migrate(&path)?;
    assert!(repeated.steps.is_empty());
    assert_eq!(ColumnFile::read_commit_token(&path)?, published);
    new.books.set_name(id.cast(), "A new edition")?;
    new.commit(writer(2))?;
    assert_eq!(
        v3::Library::open(&path)?.books.name(id.cast())?,
        "A new edition"
    );
    Ok(())
}

#[test]
fn independently_upgraded_replicas_merge_and_deduplicate_shared_history() -> Result<()> {
    let dir = TempDir::new("migrate_merge");
    let a = dir.join("a");
    let b = dir.join("b");
    let (mut left, id) = seed(&a)?;
    std::fs::copy(&a, &b)?;
    let mut right = v1::Library::open(&b)?;
    left.books.set_title(id, "Left")?;
    left.commit(writer(2))?;
    right.books.set_title(id, "Right")?;
    right.commit(writer(3))?;
    let extra = right.books.push("Extra", 5)?;
    right.commit(writer(3))?;
    let old_conflicts = {
        let mut copy = left.snapshot();
        copy.merge_snapshot(&right)?;
        copy.conflicts()?.len()
    };
    assert!(old_conflicts > 0);
    let (mut left, _) = registry().migrate(&a)?;
    assert!(matches!(
        left.merge_from(&b, writer(2)),
        Err(Error::SchemaMismatch { .. })
    ));
    registry().migrate(&b)?;
    left.merge_from(&b, writer(2))?;
    assert_eq!(left.books.versions_of(id.cast())?.len(), 3);
    assert_eq!(left.books.name(extra.cast())?, "Extra");
    assert_eq!(left.conflicts()?.len(), old_conflicts);
    let rows = Table::rows(&left.books);
    left.merge_from(&b, writer(2))?;
    assert_eq!(Table::rows(&left.books), rows);
    left.verify()?;
    Ok(())
}

#[test]
fn conversion_failure_and_incomplete_registries_leave_source_unchanged() -> Result<()> {
    let dir = TempDir::new("migrate_invalid");
    let path = dir.join("db");
    seed(&path)?;
    let original = std::fs::read(&path)?;
    let bad = Migration::<v1::Library, v2::Library>::new()
        .map::<v1::Book, v2::Volume>(|_| panic!("invalid registry ran converter"));
    assert!(
        Migrations::<v2::Library>::new()
            .step(bad)
            .migrate(&path)
            .is_err()
    );
    assert!(
        Migrations::<v2::Library>::new()
            .step(upgrade().drop::<v1::Retired>())
            .migrate(&path)
            .is_err()
    );
    assert!(
        Migrations::<v2::Library>::new()
            .step(upgrade().add::<v2::Note>())
            .migrate(&path)
            .is_err()
    );
    assert!(
        Migrations::<v2::Library>::new()
            .step(upgrade())
            .step(upgrade())
            .migrate(&path)
            .is_err()
    );
    assert!(
        Migrations::<v3::Library>::new()
            .step(upgrade())
            .migrate(&path)
            .is_err()
    );
    assert!(Migrations::<v2::Library>::new().migrate(&path).is_err());
    let failing = Migration::<v1::Library, v2::Library>::new()
        .map::<v1::Book, v2::Volume>(|_| Err(Error::invalid("cannot convert")))
        .map::<v1::Chapter, v2::Chapter>(|v| {
            Ok(v2::ChapterValue {
                volume: v.book.cast(),
                title: v.title,
            })
        })
        .drop::<v1::Retired>()
        .add::<v2::Note>();
    assert!(
        Migrations::<v2::Library>::new()
            .step(failing)
            .migrate(&path)
            .is_err()
    );
    assert_eq!(std::fs::read(&path)?, original);
    v1::Library::open(&path)?.verify()?;
    Ok(())
}

#[test]
fn dangling_references_fail_before_publication() -> Result<()> {
    let dir = TempDir::new("migrate_dangling");
    let path = dir.join("db");
    seed(&path)?;
    let token = ColumnFile::read_commit_token(&path)?;
    let migration = Migration::<v1::Library, v2::Library>::new()
        .drop::<v1::Book>()
        .add::<v2::Volume>()
        .map::<v1::Chapter, v2::Chapter>(|v| {
            Ok(v2::ChapterValue {
                volume: v.book.cast(),
                title: v.title,
            })
        })
        .drop::<v1::Retired>()
        .add::<v2::Note>();
    assert!(
        Migrations::<v2::Library>::new()
            .step(migration)
            .migrate(&path)
            .is_err()
    );
    assert_eq!(ColumnFile::read_commit_token(&path)?, token);
    Ok(())
}

#[test]
fn schema_mismatches_are_structured_and_never_modify_files() -> Result<()> {
    let dir = TempDir::new("migrate_schema");
    let path = dir.join("db");
    seed(&path)?;
    let original = std::fs::read(&path)?;
    match v2::Library::open(&path) {
        Err(Error::SchemaMismatch { expected, actual }) => {
            assert_eq!(expected.version, 2);
            assert_eq!(actual.version, 1);
        }
        _ => panic!("expected schema mismatch"),
    }
    assert_eq!(std::fs::read(&path)?, original);
    registry().migrate(&path)?;
    assert!(matches!(
        Migrations::<v1::Library>::new().migrate(&path),
        Err(Error::SchemaMismatch { .. })
    ));
    Ok(())
}

#[test]
fn replacement_failures_leave_a_complete_old_or_new_schema() -> Result<()> {
    let dir = TempDir::new("migrate_faults");
    for point in ["before_replace", "after_replace", "before_unlock"] {
        let path = dir.join(point);
        seed(&path)?;
        let before = ColumnFile::read_commit_token(&path)?;
        crate::storage::column_file::fail_rewrite_at(point);
        let error = registry().migrate(&path).err().expect("injected failure");
        if point == "before_replace" {
            assert!(matches!(error, Error::Io(_)));
            assert_eq!(ColumnFile::read_commit_token(&path)?, before);
            v1::Library::open(&path)?.verify()?;
        } else {
            match error {
                Error::MigrationUncertain {
                    expected,
                    attempted,
                    ..
                } => {
                    assert_eq!(expected, before);
                    assert_eq!(attempted, ColumnFile::read_commit_token(&path)?);
                }
                other => panic!("unexpected error: {other}"),
            }
            v2::Library::open(&path)?.verify()?;
        }
        registry().migrate(&path)?.0.verify()?;
    }
    assert!(std::fs::read_dir(&dir.path)?.all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")
    }));
    Ok(())
}

thread_local! {
    static CONCURRENT: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
    static CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
fn racing_book(value: v1::BookValue) -> Result<v2::VolumeValue> {
    CALLS.set(CALLS.get() + 1);
    let path = CONCURRENT.with(|path| path.borrow_mut().take()).unwrap();
    let mut concurrent = v1::Library::open(&path)?;
    concurrent.books.push("Concurrent", 1)?;
    concurrent.commit(writer(8))?;
    book(value)
}

#[test]
fn a_concurrent_commit_is_preserved_and_converters_are_not_retried() -> Result<()> {
    let dir = TempDir::new("migrate_race");
    let path = dir.join("db");
    seed(&path)?;
    CONCURRENT.with(|held| *held.borrow_mut() = Some(path.clone()));
    CALLS.set(0);
    let migration = Migration::<v1::Library, v2::Library>::new()
        .map::<v1::Book, v2::Volume>(racing_book)
        .map::<v1::Chapter, v2::Chapter>(|v| {
            Ok(v2::ChapterValue {
                volume: v.book.cast(),
                title: v.title,
            })
        })
        .drop::<v1::Retired>()
        .add::<v2::Note>();
    assert!(matches!(
        Migrations::<v2::Library>::new()
            .step(migration)
            .migrate(&path),
        Err(Error::Conflict { .. })
    ));
    assert_eq!(CALLS.get(), 1);
    assert_eq!(
        v1::Library::open(&path)?
            .books
            .find_by_title("Concurrent")?
            .len(),
        1
    );
    let (latest, _) = registry().migrate(&path)?;
    assert_eq!(latest.books.find_by_name("Concurrent")?.len(), 1);
    Ok(())
}

mod edited {
    #[crate::entity]
    pub struct Book {
        title: String,
        pages: f32,
    }
    #[crate::entity]
    pub struct Chapter {
        book: One<Book>,
        title: String,
    }
    #[crate::entity]
    pub struct Retired {
        text: String,
    }
    #[crate::container(schema = "library", version = 1)]
    pub struct Library {
        books: Set<Book>,
        chapters: Set<Chapter>,
        retired: Set<Retired>,
    }
}
mod foreign {
    use super::v1::{Book, Chapter, Retired};
    #[crate::container(schema = "other", version = 1)]
    pub struct Library {
        books: Set<Book>,
        chapters: Set<Chapter>,
        retired: Set<Retired>,
    }
}
mod v2_alias {
    use super::v2::{Chapter, Note, Volume};
    #[crate::container(schema = "library", version = 2)]
    pub struct Library {
        books: Set<Volume>,
        chapters: Set<Chapter>,
        notes: Set<Note>,
    }
}

#[test]
fn schema_family_and_descriptor_are_checked_even_when_widths_and_versions_match() -> Result<()> {
    let dir = TempDir::new("migrate_identity");
    let path = dir.join("db");
    seed(&path)?;
    let original = std::fs::read(&path)?;
    assert!(matches!(
        edited::Library::open(&path),
        Err(Error::SchemaMismatch { .. })
    ));
    assert!(matches!(
        foreign::Library::open(&path),
        Err(Error::SchemaMismatch { .. })
    ));
    let edited_path = dir.join("edited");
    edited::Library::create(&edited_path, writer(1))?;
    assert!(matches!(
        registry().migrate(&edited_path),
        Err(Error::SchemaMismatch { .. })
    ));
    let foreign_path = dir.join("foreign");
    foreign::Library::create(&foreign_path, writer(1))?;
    assert!(matches!(
        registry().migrate(&foreign_path),
        Err(Error::SchemaMismatch { .. })
    ));
    let alias_step = Migration::<v2_alias::Library, v3::Library>::new()
        .map::<v2::Volume, v2::Volume>(Ok)
        .map::<v2::Chapter, v2::Chapter>(Ok)
        .map::<v2::Note, v2::Note>(Ok);
    assert!(
        Migrations::<v3::Library>::new()
            .step(upgrade())
            .step(alias_step)
            .migrate(&path)
            .is_err()
    );
    assert!(
        Migrations::<v3::Library>::new()
            .step(Migration::<v1::Library, v3::Library>::new())
            .migrate(&path)
            .is_err()
    );
    assert!(
        Migrations::<v2::Library>::new()
            .step(Migration::<foreign::Library, v2::Library>::new())
            .migrate(&path)
            .is_err()
    );
    assert_eq!(std::fs::read(&path)?, original);
    Ok(())
}

#[test]
fn a_failed_later_step_never_publishes_an_intermediate_version() -> Result<()> {
    let dir = TempDir::new("migrate_late_failure");
    let path = dir.join("db");
    seed(&path)?;
    let original = std::fs::read(&path)?;
    let failing = Migration::<v2::Library, v3::Library>::new()
        .map::<v2::Volume, v2::Volume>(|_| Err(Error::invalid("later conversion failed")))
        .map::<v2::Chapter, v2::Chapter>(Ok)
        .map::<v2::Note, v2::Note>(Ok);
    let err = Migrations::<v3::Library>::new()
        .step(upgrade())
        .step(failing)
        .migrate(&path)
        .err()
        .unwrap();
    assert!(err.to_string().contains("later conversion failed"));
    assert_eq!(std::fs::read(&path)?, original);
    Ok(())
}

#[test]
fn unsupported_metadata_format_cannot_be_opened_or_migrated() -> Result<()> {
    let dir = TempDir::new("migrate_format");
    let path = dir.join("db");
    seed(&path)?;
    let file = ColumnFile::open(&path)?;
    let mut metadata = file.column::<u8>(super::super::SCHEMA)?;
    metadata.set(8, 2)?;
    let mut updater = file.update(writer(1))?;
    for column in file.columns() {
        if column.name != super::super::SCHEMA {
            updater.keep_column(&column.name)?;
        }
    }
    updater.column(super::super::SCHEMA, &metadata)?;
    updater.commit()?;
    let original = std::fs::read(&path)?;
    assert!(SchemaMetadata::inspect(&path).unwrap_err().is_unsupported());
    assert!(v1::Library::open(&path).err().unwrap().is_unsupported());
    assert!(registry().migrate(&path).err().unwrap().is_unsupported());
    assert_eq!(std::fs::read(&path)?, original);
    Ok(())
}
