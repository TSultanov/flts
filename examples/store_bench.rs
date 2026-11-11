use dbsync::schema::Store;
use dbsync::{CompactionPolicy, Result, WriterId};
use std::time::Instant;

#[dbsync::entity]
struct Note {
    #[index]
    title: String,
    body: String,
    score: u64,
}
#[dbsync::container(schema = "database", version = 1)]
struct Database {
    notes: Set<Note>,
}

fn copy_store(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_store(&entry.path(), &target)?;
        } else if entry.file_name() != "lock" {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn bytes_in(dir: &std::path::Path) -> std::io::Result<u64> {
    let mut bytes = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        bytes += match entry.file_type()?.is_dir() {
            true => bytes_in(&entry.path())?,
            false => entry.metadata()?.len(),
        };
    }
    Ok(bytes)
}

fn main() -> Result<()> {
    println!(
        "workload,commits,initial_bytes,added_bytes,update_us,read_us,compacted_bytes,compact_us"
    );
    let writer = WriterId::from_raw(1);
    for workload in ["scalar", "text", "bulk", "merge"] {
        let path = std::env::temp_dir().join(format!(
            "dbsync-store-bench-{}-{workload}",
            std::process::id()
        ));
        let peer_path = path.with_extension("peer");
        let mut db = Database::create(&path, writer)?;
        db.set_compaction_policy(CompactionPolicy::Disabled)?;
        let mut ids = Vec::new();
        for n in 0..128 {
            ids.push(db.notes.push(&format!("note {n}"), &"x".repeat(512), 0)?);
        }
        db.commit(writer)?;
        copy_store(&path, &peer_path)?;
        let mut peer = Database::open(&peer_path)?;
        peer.set_compaction_policy(CompactionPolicy::Disabled)?;
        let initial = bytes_in(&path)?;
        let rounds = if workload == "bulk" || workload == "merge" {
            8
        } else {
            32
        };
        let start = Instant::now();
        for round in 0..rounds {
            match workload {
                "scalar" => db.notes.set_score(ids[0], round + 1)?,
                "text" => db
                    .notes
                    .set_body(ids[0], &format!("{round:04}{}", "y".repeat(508)))?,
                "bulk" => {
                    for id in &ids[..32] {
                        db.notes.set_score(*id, round + 1)?;
                    }
                }
                "merge" => {
                    for id in &ids[..16] {
                        peer.notes.set_score(*id, round + 1)?;
                    }
                    peer.commit(WriterId::from_raw(2))?;
                    db.merge_from(&peer_path, writer)?;
                    continue;
                }
                _ => unreachable!(),
            }
            db.commit(writer)?;
        }
        let update_us = start.elapsed().as_micros();
        let size = bytes_in(&path)?;
        let start = Instant::now();
        Database::open(&path)?.verify()?;
        let read_us = start.elapsed().as_micros();
        let start = Instant::now();
        let compacted_bytes = db.compact()?.after_bytes;
        let compact_us = start.elapsed().as_micros();
        println!(
            "{workload},{rounds},{initial},{},{update_us},{read_us},{compacted_bytes},{compact_us}",
            size - initial
        );
        std::fs::remove_dir_all(path)?;
        std::fs::remove_dir_all(peer_path)?;
    }
    Ok(())
}
