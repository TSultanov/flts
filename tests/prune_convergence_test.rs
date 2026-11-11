#![cfg(feature = "macros")]

use dbsync::schema::{Dot, RowStatus, Store};
use dbsync::{ColumnFile, Id, Result, Uuid, WriterId};
use proptest::prelude::*;
use std::path::{Path, PathBuf};

#[dbsync::entity]
struct Page {
    title: String,
    position: u32,
    notes: Many<Note>,
}

#[dbsync::entity]
struct Note {
    page: One<Page>,
    text: String,
}

#[dbsync::container(schema = "journal", version = 1)]
struct Journal {
    pages: Set<Page>,
    notes: Set<Note>,
}

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("dbsync-prune-{}", Uuid::now_v7()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn writer(actor: usize) -> WriterId {
    WriterId::from_raw(actor as u64 + 0x70)
}

fn everyone() -> [WriterId; 3] {
    [writer(0), writer(1), writer(2)]
}

fn newer(source: &Path, held: &Path, authoritative: &Path) -> bool {
    let Ok(told) = ColumnFile::read_commit_token(source) else {
        return false;
    };
    match ColumnFile::read_commit_token(held) {
        Err(_) => true,
        Ok(ours) if ours.file_uuid == told.file_uuid => ours.generation < told.generation,
        Ok(_) => ColumnFile::read_commit_token(authoritative)
            .is_ok_and(|current| current.file_uuid == told.file_uuid),
    }
}

fn carry(from: &Path, to: &Path, origin: &Path, author: WriterId) {
    let shard = format!("{:016x}.v1.dbs", author.raw());
    let said = format!("{:016x}.dbs", author.raw());
    for (folder, name) in [("page", &shard), ("note", &shard), ("dbsync.meta", &said)] {
        let source = from.join(folder).join(name);
        let held = to.join(folder).join(name);
        if source.exists() && newer(&source, &held, &origin.join(folder).join(name)) {
            std::fs::create_dir_all(to.join(folder)).unwrap();
            let arriving = to.join(folder).join(format!(".{name}.tmp"));
            std::fs::copy(source, &arriving).unwrap();
            std::fs::rename(arriving, held).unwrap();
        }
    }
}

fn seen(journal: &Journal) -> Result<String> {
    let mut pages = Vec::new();
    for page in journal.all::<Page>() {
        let mut notes = Vec::new();
        for note in page.notes()? {
            notes.push((note.id()?.uuid(), note.text()?.into_owned()));
        }
        notes.sort();
        pages.push((
            page.id()?.uuid(),
            page.title()?.into_owned(),
            page.position()?,
            notes,
        ));
    }
    pages.sort();
    Ok(format!(
        "{pages:?} {:?} {:?}",
        journal.statuses()?,
        journal.conflicts()?
    ))
}

struct World {
    dirs: Vec<PathBuf>,
    journals: Vec<Journal>,
    witness_dir: PathBuf,
    witness: Journal,
    pages: Vec<(Id<Page>, Dot)>,
}

impl World {
    fn committed(&mut self, actor: usize) -> Result<()> {
        self.journals[actor].commit(writer(actor))?;
        carry(
            &self.dirs[actor],
            &self.witness_dir,
            &self.dirs[actor],
            writer(actor),
        );
        self.witness.refresh_peers()?;
        Ok(())
    }

    fn hears(&mut self, actor: usize, other: usize) -> Result<()> {
        for author in 0..self.dirs.len() {
            if author != actor {
                carry(
                    &self.dirs[other],
                    &self.dirs[actor],
                    &self.dirs[author],
                    writer(author),
                );
            }
        }
        self.journals[actor].refresh_peers()?;
        self.journals[actor].acknowledge(writer(actor))?;
        Ok(())
    }
}

fn exercise(operations: &[(usize, usize, u8, usize, u32)]) -> Result<()> {
    let dir = Directory::new();
    let dirs: Vec<PathBuf> = (0..3).map(|actor| dir.0.join(actor.to_string())).collect();
    let mut journals = Vec::new();
    for (actor, path) in dirs.iter().enumerate() {
        journals.push(Journal::create(path, writer(actor))?);
    }
    let witness_dir = dir.0.join("witness");
    let witness = Journal::create(&witness_dir, WriterId::from_raw(0x7f))?;
    let mut world = World {
        dirs,
        journals,
        witness_dir,
        witness,
        pages: Vec::new(),
    };

    let first = world.journals[0].pages.push("first", 0)?;
    world.committed(0)?;
    let dot = world.journals[0].at(first)?.version()?.dot();
    world.pages.push((first, dot));
    for actor in 1..3 {
        world.hears(actor, 0)?;
    }

    for &(actor, other, action, selected, value) in operations {
        let (page, original) = world.pages[selected % world.pages.len()];
        let known = world.journals[actor].status(page).ok();
        match action {
            0 | 1 if known == Some(RowStatus::Visible) => {
                world.journals[actor].pages.set_position(page, value)?;
                world.committed(actor)?;
            }
            2 if known == Some(RowStatus::Visible) => {
                world.journals[actor]
                    .pages
                    .set_title(page, &format!("title {value}"))?;
                world.committed(actor)?;
            }
            3 if world.pages.len() < 4 => {
                let made = world.journals[actor].pages.push("made", value)?;
                world.committed(actor)?;
                let dot = world.journals[actor].at(made)?.version()?.dot();
                world.pages.push((made, dot));
            }
            4 if known == Some(RowStatus::Visible) => {
                world.journals[actor]
                    .notes
                    .push(page, &format!("note {value}"))?;
                world.committed(actor)?;
            }
            5 if known == Some(RowStatus::Visible) => {
                world.journals[actor].remove(page)?;
                world.committed(actor)?;
            }
            6 if known == Some(RowStatus::Deleted) => {
                if world.journals[actor].restore(page, original).is_ok() {
                    world.committed(actor)?;
                }
            }
            7 | 8 if other != actor => world.hears(actor, other)?,
            9 | 10 => {
                world.journals[actor].prune(writer(actor), &everyone())?;
                world.journals[actor].verify()?;
            }
            11 => {
                world.journals[actor] = Journal::open(&world.dirs[actor])?;
            }
            _ => {}
        }
    }

    for _ in 0..3 {
        for actor in 0..3 {
            for other in 0..3 {
                if other != actor {
                    world.hears(actor, other)?;
                }
            }
        }
    }
    for actor in 0..3 {
        carry(
            &world.dirs[actor],
            &world.witness_dir,
            &world.dirs[actor],
            writer(actor),
        );
    }
    world.witness.refresh_peers()?;

    let expected = seen(&world.witness)?;
    for actor in 0..3 {
        world.journals[actor].verify()?;
        assert_eq!(seen(&world.journals[actor])?, expected, "writer {actor}");
    }
    std::fs::remove_dir_all(world.dirs[0].join("local"))?;
    let rebuilt = Journal::open(&world.dirs[0])?;
    rebuilt.verify()?;
    assert_eq!(
        seen(&rebuilt)?,
        expected,
        "a cache rebuilt from pruned shards"
    );

    for actor in 0..3 {
        world.journals[actor].prune(writer(actor), &everyone())?;
        assert_eq!(
            seen(&world.journals[actor])?,
            expected,
            "writer {actor} pruned"
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn pruning_never_changes_what_any_replica_ends_up_seeing(
        operations in prop::collection::vec(
            (0usize..3, 0usize..3, 0u8..12, 0usize..4, 0u32..5),
            0..40,
        )
    ) {
        exercise(&operations).unwrap();
    }
}
