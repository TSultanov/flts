use std::path::Path;

use crate::error::{Error, Result};
use crate::schema::{RowStatus, SchemaMetadata, Store, Table};
use crate::storage::WriterId;
use crate::test_utils::{TempDir, shared_files};

mod old {
    #[crate::field_enum]
    #[derive(PartialEq, Eq, Debug)]
    pub enum Binding {
        Cloth = 0,
        Paper = 1,
    }

    #[crate::entity]
    pub struct Book {
        #[index]
        title: String,
        pages: u32,
        binding: Binding,
    }

    #[crate::container(schema = "shelf", version = 1)]
    pub struct Shelf {
        pub books: Set<Book>,
    }
}

mod new {
    #[crate::field_enum]
    #[derive(PartialEq, Eq, Debug)]
    pub enum Binding {
        Cloth = 0,
        Paper = 1,
        Leather = 2,
    }

    #[crate::entity]
    pub struct Book {
        #[index]
        title: String,
        pages: u32,
        binding: Binding,
        subtitle: Option<String>,
        rating: Option<u8>,
        notes: Many<Note>,
    }

    #[crate::entity]
    pub struct Note {
        book: One<Book>,
        text: String,
    }

    #[crate::container(schema = "shelf", version = 1)]
    pub struct Shelf {
        pub books: Set<Book>,
        pub notes: Set<Note>,
    }
}

mod retyped {
    #[crate::field_enum]
    pub enum Binding {
        Cloth = 0,
        Paper = 1,
    }

    #[crate::entity]
    pub struct Book {
        title: String,
        pages: u64,
        binding: Binding,
    }

    #[crate::container(schema = "shelf", version = 1)]
    pub struct Shelf {
        pub books: Set<Book>,
    }
}

mod demanding {
    #[crate::field_enum]
    pub enum Binding {
        Cloth = 0,
        Paper = 1,
    }

    #[crate::entity]
    pub struct Book {
        title: String,
        pages: u32,
        binding: Binding,
        isbn: String,
    }

    #[crate::container(schema = "shelf", version = 1)]
    pub struct Shelf {
        pub books: Set<Book>,
    }
}

fn phone() -> WriterId {
    WriterId::from_raw(0x01d)
}

fn desk() -> WriterId {
    WriterId::from_raw(0x0e7)
}

fn deliver(from: &Path, to: &Path, writer: WriterId, entity: &str) {
    let name = format!("{:016x}.v1.dbs", writer.raw());
    std::fs::create_dir_all(to.join(entity)).unwrap();
    std::fs::copy(from.join(entity).join(&name), to.join(entity).join(&name)).unwrap();
}

#[test]
fn an_older_reader_takes_a_newer_writers_rows_and_ignores_what_it_does_not_know() -> Result<()> {
    let dir = TempDir::new("evolve_older_reader");
    let (old_dir, new_dir) = (dir.join("phone"), dir.join("desk"));
    let mut phone_shelf = old::Shelf::create(&old_dir, phone())?;
    let mut desk_shelf = new::Shelf::create(&new_dir, desk())?;
    let emma = desk_shelf
        .books
        .push("Emma", 474, new::Binding::Cloth, Some("a novel"), Some(5))?;
    desk_shelf.notes.push(emma, "lent to a friend")?;
    desk_shelf.commit(desk())?;
    deliver(&new_dir, &old_dir, desk(), "book");
    deliver(&new_dir, &old_dir, desk(), "note");

    let told = phone_shelf.refresh_peers()?;

    assert_eq!(told.taken, 1);
    assert!(told.incompatible.is_empty(), "{told:?}");
    assert_eq!(phone_shelf.books.title(emma.cast())?, "Emma");
    assert_eq!(phone_shelf.books.pages(emma.cast())?, 474);
    phone_shelf.verify()?;
    Ok(())
}

#[test]
fn an_edit_from_an_older_writer_keeps_the_fields_it_never_knew() -> Result<()> {
    let dir = TempDir::new("evolve_preserve");
    let (old_dir, new_dir) = (dir.join("phone"), dir.join("desk"));
    let mut phone_shelf = old::Shelf::create(&old_dir, phone())?;
    let mut desk_shelf = new::Shelf::create(&new_dir, desk())?;
    let emma = desk_shelf
        .books
        .push("Emma", 474, new::Binding::Cloth, Some("a novel"), Some(5))?;
    desk_shelf.commit(desk())?;
    deliver(&new_dir, &old_dir, desk(), "book");
    phone_shelf.refresh_peers()?;

    phone_shelf.books.set_pages(emma.cast(), 480)?;
    phone_shelf.commit(phone())?;
    phone_shelf
        .books
        .set_title(emma.cast(), "Emma, corrected")?;
    phone_shelf.commit(phone())?;
    deliver(&old_dir, &new_dir, phone(), "book");
    let told = desk_shelf.refresh_peers()?;

    assert_eq!(told.taken, 2);
    assert!(told.is_quiet(), "{told:?}");
    let held = desk_shelf.at(emma)?;
    assert_eq!(held.title()?, "Emma, corrected");
    assert_eq!(held.pages()?, 480);
    assert_eq!(held.subtitle()?.as_deref(), Some("a novel"));
    assert_eq!(held.rating()?, Some(5));
    desk_shelf.verify()?;

    std::fs::remove_dir_all(new_dir.join("local"))?;
    let rebuilt = new::Shelf::open(&new_dir)?;
    assert_eq!(rebuilt.at(emma)?.subtitle()?.as_deref(), Some("a novel"));
    assert_eq!(rebuilt.at(emma)?.rating()?, Some(5));
    Ok(())
}

#[test]
fn a_row_an_older_writer_created_reads_its_missing_fields_as_absent() -> Result<()> {
    let dir = TempDir::new("evolve_absent");
    let (old_dir, new_dir) = (dir.join("phone"), dir.join("desk"));
    let mut phone_shelf = old::Shelf::create(&old_dir, phone())?;
    let mut desk_shelf = new::Shelf::create(&new_dir, desk())?;
    let emma = phone_shelf.books.push("Emma", 474, old::Binding::Paper)?;
    phone_shelf.commit(phone())?;
    deliver(&old_dir, &new_dir, phone(), "book");

    assert_eq!(desk_shelf.refresh_peers()?.taken, 1);

    let held = desk_shelf.at(emma.cast::<new::Book>())?;
    assert_eq!(held.binding()?, new::Binding::Paper);
    assert_eq!(held.subtitle()?, None);
    assert_eq!(held.rating()?, None);
    assert_eq!(held.notes()?.count(), 0);
    Ok(())
}

#[test]
fn an_edit_that_arrives_before_the_revision_it_inherits_from_waits_for_it() -> Result<()> {
    let dir = TempDir::new("evolve_waiting");
    let (old_dir, new_dir, third_dir) = (dir.join("phone"), dir.join("desk"), dir.join("laptop"));
    let laptop = WriterId::from_raw(0x1a9);
    let mut phone_shelf = old::Shelf::create(&old_dir, phone())?;
    let mut desk_shelf = new::Shelf::create(&new_dir, desk())?;
    let mut laptop_shelf = new::Shelf::create(&third_dir, laptop)?;
    let emma = desk_shelf
        .books
        .push("Emma", 474, new::Binding::Cloth, Some("a novel"), None)?;
    desk_shelf.commit(desk())?;
    deliver(&new_dir, &old_dir, desk(), "book");
    phone_shelf.refresh_peers()?;
    phone_shelf.books.set_pages(emma.cast(), 480)?;
    phone_shelf.commit(phone())?;
    let other = phone_shelf
        .books
        .push("Sanditon", 80, old::Binding::Paper)?;
    phone_shelf.commit(phone())?;

    deliver(&old_dir, &third_dir, phone(), "book");
    let early = laptop_shelf.refresh_peers()?;
    assert_eq!((early.taken, early.waiting), (0, 2));
    assert!(laptop_shelf.at(emma).is_err());
    assert!(laptop_shelf.at(other.cast::<new::Book>()).is_err());
    let again = laptop_shelf.refresh_peers()?;
    assert_eq!(
        (again.taken, again.waiting),
        (0, 2),
        "it is asked for again"
    );

    deliver(&new_dir, &third_dir, desk(), "book");
    let late = laptop_shelf.refresh_peers()?;
    assert_eq!((late.taken, late.waiting), (3, 0));
    assert_eq!(laptop_shelf.at(emma)?.pages()?, 480);
    assert_eq!(
        laptop_shelf.at(emma)?.subtitle()?.as_deref(),
        Some("a novel")
    );
    assert_eq!(
        laptop_shelf.at(other.cast::<new::Book>())?.title()?,
        "Sanditon"
    );
    assert_eq!(laptop_shelf.refresh_peers()?.taken, 0);
    Ok(())
}

#[test]
fn a_row_carrying_a_variant_an_older_reader_does_not_know_waits_and_the_rows_before_it_are_read()
-> Result<()> {
    let dir = TempDir::new("evolve_variant");
    let (old_dir, new_dir) = (dir.join("phone"), dir.join("desk"));
    let mut phone_shelf = old::Shelf::create(&old_dir, phone())?;
    let mut desk_shelf = new::Shelf::create(&new_dir, desk())?;
    let emma = desk_shelf
        .books
        .push("Emma", 474, new::Binding::Cloth, None, None)?;
    desk_shelf.commit(desk())?;
    desk_shelf
        .books
        .push("Persuasion", 249, new::Binding::Leather, None, None)?;
    desk_shelf.commit(desk())?;
    phone_shelf
        .books
        .push("Sanditon", 80, old::Binding::Paper)?;
    phone_shelf.commit(phone())?;
    deliver(&new_dir, &old_dir, desk(), "book");
    deliver(&old_dir, &new_dir, phone(), "book");

    let told = phone_shelf.refresh_peers()?;
    assert_eq!((told.taken, told.waiting), (1, 1));
    assert!(told.incompatible.is_empty());
    assert_eq!(phone_shelf.books.title(emma.cast())?, "Emma");
    assert_eq!(phone_shelf.all::<old::Book>().count(), 2);
    phone_shelf.verify()?;

    let heard = desk_shelf.refresh_peers()?;
    assert_eq!(heard.taken, 1, "the wider enum reads the narrower one");
    assert!(heard.incompatible.is_empty());
    Ok(())
}

#[test]
fn a_field_of_another_type_or_a_missing_required_field_is_reported_not_fatal() -> Result<()> {
    let dir = TempDir::new("evolve_incompatible");
    let (old_dir, other_dir, strict_dir) =
        (dir.join("phone"), dir.join("retyped"), dir.join("strict"));
    let mut phone_shelf = old::Shelf::create(&old_dir, phone())?;
    phone_shelf.books.push("Emma", 474, old::Binding::Cloth)?;
    phone_shelf.commit(phone())?;

    let mut retyped = retyped::Shelf::create(&other_dir, desk())?;
    deliver(&old_dir, &other_dir, phone(), "book");
    let told = retyped.refresh_peers()?;
    assert_eq!((told.taken, told.incompatible.len()), (0, 1));
    assert_eq!(Table::rows(&retyped.books), 0);

    let mut strict = demanding::Shelf::create(&strict_dir, desk())?;
    deliver(&old_dir, &strict_dir, phone(), "book");
    let told = strict.refresh_peers()?;
    assert_eq!((told.taken, told.incompatible.len()), (0, 1));
    assert!(demanding::Shelf::open(&strict_dir).is_ok());
    Ok(())
}

#[test]
fn a_schema_that_only_grew_reopens_its_own_directory_and_keeps_writing_the_same_shards()
-> Result<()> {
    let dir = TempDir::new("evolve_grow");
    let path = dir.join("shelf");
    let mut before = old::Shelf::create(&path, phone())?;
    let emma = before.books.push("Emma", 474, old::Binding::Cloth)?;
    before.commit(phone())?;
    drop(before);

    let mut after = new::Shelf::open(&path)?;
    let emma = emma.cast::<new::Book>();
    assert_eq!(after.at(emma)?.title()?, "Emma");
    assert_eq!(after.at(emma)?.subtitle()?, None);
    after.books.set_subtitle(emma, Some("a novel"))?;
    let sanditon = after
        .books
        .push("Sanditon", 80, new::Binding::Leather, None, Some(4))?;
    after.notes.push(sanditon, "unfinished")?;
    after.commit(phone())?;
    drop(after);

    let files: Vec<_> = shared_files(&path).into_keys().collect();
    assert_eq!(
        files,
        [
            Path::new("book/000000000000001d.v1.dbs"),
            Path::new("dbsync.meta/000000000000001d.dbs"),
            Path::new("note/000000000000001d.v1.dbs")
        ]
    );
    std::fs::remove_dir_all(path.join("local"))?;
    let rebuilt = new::Shelf::open(&path)?;
    assert_eq!(rebuilt.at(emma)?.subtitle()?.as_deref(), Some("a novel"));
    assert_eq!(rebuilt.at(sanditon)?.rating()?, Some(4));
    assert_eq!(rebuilt.at(sanditon)?.notes()?.count(), 1);
    assert_eq!(rebuilt.status(emma)?, RowStatus::Visible);
    rebuilt.verify()?;
    Ok(())
}

#[test]
fn a_schema_whose_field_changed_type_refuses_its_own_cache_rather_than_reinterpret_it() -> Result<()>
{
    let dir = TempDir::new("evolve_refuse");
    let path = dir.join("shelf");
    let mut before = old::Shelf::create(&path, phone())?;
    before.books.push("Emma", 474, old::Binding::Cloth)?;
    before.commit(phone())?;
    drop(before);

    assert!(matches!(
        retyped::Shelf::open(&path),
        Err(Error::SchemaMismatch { .. })
    ));
    assert_eq!(Table::rows(&old::Shelf::open(&path)?.books), 1);
    Ok(())
}

#[test]
fn the_description_of_a_schema_names_fields_and_types_and_nothing_about_where_code_lives() {
    let described = String::from_utf8(old::Shelf::default().schema_metadata().descriptor).unwrap();
    assert_eq!(
        described,
        "book\tbinding\tenum binding|Cloth=0,Paper=1\nbook\tpages\tu32\nbook\ttitle\ttext\n"
    );
    assert!(SchemaMetadata::reads(
        "enum binding|Cloth=0,Paper=1,Leather=2",
        "enum binding|Cloth=0,Paper=1"
    ));
    assert!(SchemaMetadata::reads(
        "enum binding|Cloth=0,Paper=1",
        "enum binding|Cloth=0,Paper=1,Leather=2"
    ));
    assert!(!SchemaMetadata::reads(
        "enum binding|Cloth=0,Paper=1",
        "enum binding|Cloth=1,Paper=0"
    ));
    assert!(!SchemaMetadata::reads(
        "enum binding|Cloth=0,Paper=1",
        "enum binding|Cloth=0,Board=1"
    ));
    assert!(!SchemaMetadata::reads(
        "enum binding|Cloth=0,Paper=1",
        "enum cover|Cloth=0,Paper=1"
    ));
    assert!(!SchemaMetadata::reads("u32", "u64"));
    assert!(SchemaMetadata::reads("optional text", "optional text"));
}
