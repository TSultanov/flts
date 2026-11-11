use super::*;
use crate::WriterId;
use crate::schema::RowStatus;
use crate::test_utils::{TempDir, copy_dir, shared_files};
use std::path::Path;

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
    let mut held = (0..Table::rows(table))
        .map(|i| {
            let row = Row::trusted(i);
            Ok((
                table.id_at(row)?.uuid(),
                table.versions().raw_at(row)?,
                table.versions().parents(row)?.to_vec(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    held.sort_by_key(|(_, version, _)| version.dot());
    Ok(held)
}

#[test]
fn chain_preserves_history_references_and_clocks_without_rewriting_a_shard() -> Result<()> {
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
    let shared = shared_files(&path);
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
    assert_eq!(shared_files(&path), shared, "no shard was rewritten");
    assert_eq!(old.books.title(id)?, "Alice");

    let (_, repeated) = migrations.migrate(&path)?;
    assert!(repeated.steps.is_empty());

    new.books.set_name(id.cast(), "A new edition")?;
    new.commit(writer(2))?;
    assert!(new.layout().unwrap().shard("volume", writer(2), 3).exists());
    assert_eq!(
        v3::Library::open(&path)?.books.name(id.cast())?,
        "A new edition"
    );
    let (reopened, again) = migrations.migrate(&path)?;
    assert!(again.steps.is_empty());
    assert_eq!(reopened.books.name(id.cast())?, "A new edition");
    Ok(())
}

#[test]
fn an_upgraded_replica_takes_what_an_older_peer_keeps_writing() -> Result<()> {
    let dir = TempDir::new("migrate_merge");
    let a = dir.join("a");
    let b = dir.join("b");
    let (mut left, id) = seed(&a)?;
    copy_dir(&a, &b);
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
    let told = left.merge_from(&b, writer(2))?;
    assert_eq!(told.taken, 0, "an older shard is not read as a newer one");
    assert_eq!(told.incompatible.len(), 1);
    let (merged, converted) = registry().refresh(&mut left)?;
    assert!(merged.incompatible.is_empty());
    assert_eq!(converted.steps.len(), 1);

    assert_eq!(left.books.versions_of(id.cast())?.len(), 3);
    assert_eq!(left.books.name(extra.cast())?, "Extra");
    assert_eq!(left.conflicts()?.len(), old_conflicts);
    let rows = Table::rows(&left.books);
    left.merge_from(&b, writer(2))?;
    let (_, again) = registry().refresh(&mut left)?;
    assert!(again.steps.is_empty());
    assert_eq!(Table::rows(&left.books), rows);
    left.verify()?;

    let later = right.books.push("Later", 9)?;
    right.commit(writer(3))?;
    left.merge_from(&b, writer(2))?;
    registry().refresh(&mut left)?;
    assert_eq!(left.books.name(later.cast())?, "Later");
    assert_eq!(Table::rows(&left.books), rows + 1);
    Ok(())
}

#[test]
fn conversion_failure_and_incomplete_registries_leave_every_shard_unchanged() -> Result<()> {
    let dir = TempDir::new("migrate_invalid");
    let path = dir.join("db");
    seed(&path)?;
    let original = shared_files(&path);
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
    let err = Migrations::<v2::Library>::new()
        .step(failing)
        .migrate(&path)
        .err()
        .unwrap();
    assert!(err.to_string().contains("cannot convert"));
    assert_eq!(shared_files(&path), original);
    v1::Library::open(&path)?.verify()?;
    assert_eq!(registry().migrate(&path)?.1.steps.len(), 1);
    Ok(())
}

#[test]
fn a_child_whose_parent_table_was_dropped_is_suppressed_rather_than_refused() -> Result<()> {
    let dir = TempDir::new("migrate_dangling");
    let path = dir.join("db");
    seed(&path)?;
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
    let (new, _) = Migrations::<v2::Library>::new()
        .step(migration)
        .migrate(&path)?;
    let chapter = new.chapters.id_at(Row::trusted(0))?;
    assert!(matches!(new.status(chapter)?, RowStatus::Suppressed(_)));
    assert_eq!(new.dangling()?.len(), 1);
    Ok(())
}

#[test]
fn a_newer_schema_refuses_to_open_past_older_shards_it_was_not_shown_how_to_read() -> Result<()> {
    let dir = TempDir::new("migrate_schema");
    let path = dir.join("db");
    seed(&path)?;
    let original = shared_files(&path);
    assert!(v2::Library::open(&path).err().unwrap().is_unsupported());
    assert_eq!(shared_files(&path), original);
    registry().migrate(&path)?;
    assert_eq!(Table::rows(&v2::Library::open(&path)?.books), 1);
    assert_eq!(Table::rows(&v1::Library::open(&path)?.books), 1);
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
    let original = shared_files(&path);
    assert!(matches!(
        edited::Library::open(&path),
        Err(Error::SchemaMismatch { .. })
    ));
    assert!(matches!(
        foreign::Library::open(&path),
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
    assert_eq!(shared_files(&path), original);
    Ok(())
}

#[test]
fn a_shard_written_by_an_edited_schema_is_reported_and_left_out() -> Result<()> {
    let dir = TempDir::new("migrate_edited");
    let path = dir.join("db");
    let mut edited = edited::Library::create(&path, writer(4))?;
    edited.books.push("Edited", 1.5)?;
    edited.commit(writer(4))?;
    std::fs::remove_dir_all(path.join("local"))?;

    let mut honest = v1::Library::open(&path)?;
    assert_eq!(Table::rows(&honest.books), 0);
    assert_eq!(honest.refresh_peers()?.incompatible.len(), 1);
    Ok(())
}

#[test]
fn a_failed_later_step_adopts_nothing() -> Result<()> {
    let dir = TempDir::new("migrate_late_failure");
    let path = dir.join("db");
    seed(&path)?;
    let original = shared_files(&path);
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
    assert_eq!(shared_files(&path), original);
    let (recovered, report) = Migrations::<v3::Library>::new()
        .step(upgrade())
        .step(next())
        .migrate(&path)?;
    assert_eq!(report.steps.len(), 2);
    assert_eq!(Table::rows(&recovered.books), 1);
    Ok(())
}
