use crate::error::Result;
use crate::id::{Id, Uuid};
use crate::schema::{RowStatus, Store, Table};
use crate::storage::WriterId;
use crate::test_utils::TempDir;

#[crate::field_enum]
#[derive(PartialEq, Eq, Debug)]
enum Binding {
    Cloth = 0,
    Paper = 1,
}

#[crate::entity]
struct Series {
    name: String,
    volumes: Many<Volume>,
}

#[crate::entity]
struct Volume {
    series: Option<One<Series>>,
    title: String,
    cover: Vec<u8>,
    scan: Option<Vec<u8>>,
    binding: Option<Binding>,
}

#[crate::container(schema = "kinds", version = 1)]
struct Shelf {
    series: Set<Series>,
    volumes: Set<Volume>,
}

fn here() -> WriterId {
    WriterId::from_raw(7)
}

type Stocked = (Shelf, Id<Series>, Id<Volume>, Id<Volume>);

fn stocked(dir: &TempDir) -> Result<Stocked> {
    let mut shelf = Shelf::create(dir.join("shelf.dbs"), here())?;
    let discworld = shelf.series.push("Discworld")?;
    let mort = shelf.volumes.push(
        Some(discworld),
        "Mort",
        &[0xff, 0x00, 0x01],
        Some(&[9, 9]),
        Some(Binding::Paper),
    )?;
    let loose = shelf.volumes.push(None, "Loose leaves", &[], None, None)?;
    shelf.commit(here())?;
    Ok((shelf, discworld, mort, loose))
}

#[test]
fn bytes_and_optional_fields_survive_a_commit_and_a_reopen() -> Result<()> {
    let dir = TempDir::new("kinds_reopen");
    let (_, discworld, mort, loose) = stocked(&dir)?;

    let shelf = Shelf::open(dir.join("shelf.dbs"))?;
    shelf.verify()?;
    let held = shelf.at(mort)?;
    assert_eq!(*held.cover()?, [0xff, 0x00, 0x01]);
    assert_eq!(held.scan()?.as_deref(), Some(&[9u8, 9][..]));
    assert_eq!(held.binding()?, Some(Binding::Paper));
    assert_eq!(held.series()?.unwrap().id()?, discworld);

    let alone = shelf.at(loose)?;
    assert!(alone.cover()?.is_empty());
    assert!(alone.scan()?.is_none());
    assert_eq!(alone.binding()?, None);
    assert!(alone.series()?.is_none());
    Ok(())
}

#[test]
fn a_many_only_groups_the_children_that_name_a_parent() -> Result<()> {
    let dir = TempDir::new("kinds_many");
    let (shelf, discworld, mort, _) = stocked(&dir)?;

    let grouped: Vec<_> = shelf
        .at(discworld)?
        .volumes()?
        .map(|volume| volume.id())
        .collect::<Result<_>>()?;
    assert_eq!(grouped, [mort]);
    Ok(())
}

#[test]
fn optional_fields_can_be_set_and_cleared() -> Result<()> {
    let dir = TempDir::new("kinds_clear");
    let (mut shelf, discworld, mort, loose) = stocked(&dir)?;

    shelf.volumes.set_series(mort, None)?;
    shelf.volumes.set_scan(mort, None)?;
    shelf.volumes.set_binding(mort, None)?;
    shelf.volumes.set_series(loose, Some(discworld))?;
    shelf.volumes.set_cover(loose, &[1, 2, 3, 4])?;
    shelf.commit(here())?;

    let shelf = Shelf::open(dir.join("shelf.dbs"))?;
    assert!(shelf.at(mort)?.series()?.is_none());
    assert!(shelf.at(mort)?.scan()?.is_none());
    assert_eq!(shelf.at(mort)?.binding()?, None);
    assert_eq!(*shelf.at(mort)?.cover()?, [0xff, 0x00, 0x01]);
    assert_eq!(*shelf.at(loose)?.cover()?, [1, 2, 3, 4]);
    assert_eq!(shelf.children_of::<Series, Volume>(discworld)?, [loose]);
    Ok(())
}

#[test]
fn removing_a_parent_hides_only_the_children_that_name_it() -> Result<()> {
    let dir = TempDir::new("kinds_remove");
    let (mut shelf, discworld, mort, loose) = stocked(&dir)?;

    shelf.remove(discworld)?;

    assert!(matches!(shelf.status(mort)?, RowStatus::Suppressed(_)));
    assert_eq!(shelf.status(loose)?, RowStatus::Visible);
    Ok(())
}

#[test]
fn the_new_kinds_merge_between_copies() -> Result<()> {
    let dir = TempDir::new("kinds_merge");
    let (mut shelf, _, mort, _) = stocked(&dir)?;
    crate::test_utils::copy_store(dir.join("shelf.dbs"), dir.join("peer.dbs"))?;

    let mut peer = Shelf::open(dir.join("peer.dbs"))?;
    peer.volumes.set_cover(mort, &[0xaa; 40])?;
    peer.volumes.set_binding(mort, Some(Binding::Cloth))?;
    peer.commit(WriterId::from_raw(8))?;

    let report = shelf.merge_from(dir.join("peer.dbs"), here())?;
    assert!(report.is_quiet());
    assert_eq!(*shelf.at(mort)?.cover()?, [0xaa; 40]);
    assert_eq!(shelf.at(mort)?.binding()?, Some(Binding::Cloth));
    shelf.verify()?;
    Ok(())
}

#[test]
fn a_row_can_be_created_under_an_id_the_caller_chose() -> Result<()> {
    let dir = TempDir::new("kinds_insert");
    let (mut shelf, _, _, _) = stocked(&dir)?;
    let namespace = Uuid::from_u128(0x6ba7b810_9dad_11d1_80b4_00c04fd430c8);
    let chosen: Id<Series> = Id::derived(namespace, b"series/culture");

    shelf.series.insert(chosen, "Culture")?;
    assert_eq!(shelf.at(chosen)?.name()?, "Culture");

    let again = shelf.series.insert(chosen, "Culture, again");
    assert!(again.is_err());
    assert_eq!(Table::versions_of(&shelf.series, chosen)?.len(), 1);

    shelf.commit(here())?;
    assert_eq!(
        Shelf::open(dir.join("shelf.dbs"))?.at(chosen)?.name()?,
        "Culture"
    );
    Ok(())
}

#[test]
fn two_copies_that_derive_the_same_id_converge_on_one_row() -> Result<()> {
    let dir = TempDir::new("kinds_converge");
    let (mut shelf, _, _, _) = stocked(&dir)?;
    crate::test_utils::copy_store(dir.join("shelf.dbs"), dir.join("peer.dbs"))?;
    let namespace = Uuid::from_u128(0x6ba7b810_9dad_11d1_80b4_00c04fd430c8);
    let chosen: Id<Series> = Id::derived(namespace, b"series/culture");

    let mut peer = Shelf::open(dir.join("peer.dbs"))?;
    peer.series.insert(chosen, "Culture")?;
    peer.commit(WriterId::from_raw(8))?;
    shelf.series.insert(chosen, "Culture")?;
    shelf.commit(here())?;

    let report = shelf.merge_from(dir.join("peer.dbs"), here())?;
    assert!(
        report.is_quiet(),
        "equal values under one id are not a conflict"
    );
    assert_eq!(shelf.all::<Series>().count(), 2);
    assert_eq!(shelf.at(chosen)?.name()?, "Culture");
    Ok(())
}

#[test]
fn a_nil_id_is_refused() -> Result<()> {
    let dir = TempDir::new("kinds_nil");
    let (mut shelf, _, _, _) = stocked(&dir)?;
    assert!(shelf.series.insert(Id::NIL, "nobody").is_err());
    assert_eq!(shelf.all::<Series>().count(), 1);
    Ok(())
}

mod resolved {
    use super::Binding;

    #[crate::entity]
    pub struct Folder {
        name: String,
        notes: Many<Note>,
    }

    #[crate::entity]
    #[merge(with = longest)]
    pub struct Note {
        folder: Option<One<Folder>>,
        body: Vec<u8>,
        draft: Option<Vec<u8>>,
        binding: Option<Binding>,
    }

    fn longest(input: &crate::schema::MergeInput<'_, NoteValue>) -> NoteValue {
        let mut kept = input.candidates[0].value.clone();
        for candidate in input.candidates {
            if candidate.value.body.len() > kept.body.len() {
                kept.body = candidate.value.body.clone();
            }
            kept.folder = kept.folder.or(candidate.value.folder);
            kept.draft = kept.draft.or(candidate.value.draft.clone());
            kept.binding = kept.binding.or(candidate.value.binding);
        }
        kept
    }

    #[crate::container(schema = "notes", version = 1)]
    pub struct Notes {
        pub folders: Set<Folder>,
        pub notes: Set<Note>,
    }
}

#[test]
fn a_resolver_combines_the_new_kinds_from_both_copies() -> Result<()> {
    let dir = TempDir::new("kinds_resolved");
    let mut ours = resolved::Notes::create(dir.join("ours.dbs"), here())?;
    let folder = ours.folders.push("inbox")?;
    let note = ours.notes.push(None, &[1], None, None)?;
    ours.commit(here())?;
    crate::test_utils::copy_store(dir.join("ours.dbs"), dir.join("theirs.dbs"))?;

    let mut theirs = resolved::Notes::open(dir.join("theirs.dbs"))?;
    theirs.notes.set_folder(note, Some(folder))?;
    theirs.notes.set_binding(note, Some(Binding::Cloth))?;
    theirs.commit(WriterId::from_raw(8))?;
    ours.notes.set_body(note, &[1, 2, 3])?;
    ours.notes.set_draft(note, Some(&[4]))?;

    let report = ours.merge_from(dir.join("theirs.dbs"), here())?;

    assert_eq!(report.combined.len(), 1);
    let merged = ours.at(note)?;
    assert_eq!(*merged.body()?, [1, 2, 3]);
    assert_eq!(merged.draft()?.as_deref(), Some(&[4u8][..]));
    assert_eq!(merged.binding()?, Some(Binding::Cloth));
    assert_eq!(merged.folder()?.unwrap().id()?, folder);
    assert_eq!(
        ours.children_of::<resolved::Folder, resolved::Note>(folder)?,
        [note]
    );
    Ok(())
}
