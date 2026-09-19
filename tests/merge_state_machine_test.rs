#![cfg(feature = "derive")]

use dbsync::schema::{Parent, RowStatus, Store, Table};
use dbsync::{Id, Result, Uuid, WriterId};
use proptest::prelude::*;

#[dbsync::entity]
struct Node {
    #[index(unique)]
    key: Option<u32>,
    parent: One<Node>,
    value: u32,
}

#[dbsync::container]
struct Database {
    nodes: Set<Node>,
}

struct Directory(std::path::PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("dbsync-state-{}", Uuid::now_v7()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, actor: usize) -> std::path::PathBuf {
        self.0.join(actor.to_string())
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn publish(db: &mut Database, actor: usize) -> Result<()> {
    let mut clock = db.clock().clone();
    db.nodes
        .stamp_rows(&mut clock, WriterId::from_raw(actor as u64 + 10))?;
    db.absorb(&clock)
}

fn state(db: &Database) -> Result<String> {
    let mut revisions = Vec::new();
    for row in db.nodes.rows_by_dot()? {
        let version = db.nodes.versions().raw_at(row)?;
        let parents: Vec<_> = db
            .nodes
            .versions()
            .parents(row)?
            .iter()
            .map(|p| match p {
                Parent::Published(dot) => *dot,
                Parent::Pending(_) => panic!("all simulated mutations must be published"),
            })
            .collect();
        revisions.push((
            db.nodes.id_at(row)?.uuid(),
            version.dot(),
            version.is_removed(),
            version.is_restoration(),
            parents,
            db.nodes.payload(row)?,
        ));
    }
    revisions.sort();
    let mut visible = Vec::new();
    for row in db.all::<Node>() {
        visible.push((row.id()?.uuid(), row.key()?, row.value()?));
    }
    visible.sort();
    Ok(format!(
        "{revisions:?} {:?} {:?} {visible:?}",
        db.statuses()?,
        db.conflicts()?
    ))
}

fn exercise(operations: &[(usize, usize, u8, usize, u32)]) -> Result<()> {
    let dir = Directory::new();
    let mut first = Database::create(dir.path(0), WriterId::from_raw(10))?;
    let root = first.nodes.push(None, Id::NIL, 0)?;
    let child = first.nodes.push(None, root, 1)?;
    first.commit(WriterId::from_raw(10))?;
    let ids = [root, child];
    let originals = [
        first.at(root)?.version()?.dot(),
        first.at(child)?.version()?.dot(),
    ];
    for actor in 1..3 {
        std::fs::copy(dir.path(0), dir.path(actor))?;
    }
    let mut replicas = [
        first,
        Database::open(dir.path(1))?,
        Database::open(dir.path(2))?,
    ];
    for &(actor, other, action, selected, value) in operations {
        let id = ids[selected];
        let source = replicas[other].clone();
        let db = &mut replicas[actor];
        match action {
            0 => {
                let _ = db.nodes.set_value(id, value);
            }
            1 => {
                let _ = db.nodes.set_key(id, Some(value));
            }
            2 => {
                let _ = db.nodes.set_parent(id, ids[1 - selected]);
            }
            3 => {
                let _ = db.remove(id);
            }
            4 if db.status(id)? == RowStatus::Deleted => {
                db.restore(id, originals[selected])?;
            }
            5 => {
                db.merge(&source)?;
            }
            6 => {
                db.commit(WriterId::from_raw(actor as u64 + 10))?;
                *db = Database::open(dir.path(actor))?;
            }
            7 => {
                let _ = db.nodes.push(Some(value), id, value);
            }
            _ => {}
        }
        publish(db, actor)?;
        db.audit()?;
    }
    let permutations = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let mut expected = None;
    for [a, b, c] in permutations {
        let mut left = replicas[a].clone();
        left.merge(&replicas[b])?;
        left.merge(&replicas[c])?;
        left.audit()?;
        let mut bc = replicas[b].clone();
        bc.merge(&replicas[c])?;
        let mut right = replicas[a].clone();
        right.merge(&bc)?;
        let actual = state(&left)?;
        assert_eq!(actual, state(&right)?);
        if let Some(expected) = &expected {
            assert_eq!(&actual, expected);
        } else {
            expected = Some(actual);
        }
        for replica in &replicas {
            let report = left.merge(replica)?;
            assert!(!report.changed_anything(), "{report:?}");
            assert!(report.is_quiet(), "{report:?}");
        }
        assert_eq!(state(&left)?, expected.as_ref().unwrap().as_str());
    }
    let mut merged = replicas[0].clone();
    merged.merge(&replicas[1])?;
    merged.merge(&replicas[2])?;
    merged.commit(WriterId::from_raw(10))?;
    assert_eq!(state(&Database::open(dir.path(0))?)?, expected.unwrap());
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn replicas_converge_through_mutations_partitions_and_reopens(
        operations in prop::collection::vec((0usize..3, 0usize..3, 0u8..8, 0usize..2, 0u32..4), 0..30)
    ) {
        exercise(&operations).unwrap();
    }
}
