//! What each writer says it has seen, published beside its shards for the others to read.

use std::path::{Path, PathBuf};

use super::clock::VersionVector;
use super::version::Dot;
use crate::error::{Error, Result};
use crate::storage::{Arena, ColumnFile, ColumnFileWriter, WriterId};

pub const FOLDER: &str = "dbsync.meta";
const SEEN: &str = "seen";
const RETIRED: &str = "retired";

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Peer {
    pub writer: WriterId,
    pub seen: VersionVector,
    pub retired: Vec<WriterId>,
}

pub fn peer_path(shared: &Path, writer: WriterId) -> PathBuf {
    shared
        .join(FOLDER)
        .join(format!("{:016x}.dbs", writer.raw()))
}

fn named(path: &Path) -> Option<WriterId> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".dbs")?;
    let raw = u64::from_str_radix(stem, 16).ok()?;
    (raw != 0 && name == format!("{raw:016x}.dbs")).then(|| WriterId::from_raw(raw))
}

pub fn read(shared: &Path, writer: WriterId) -> Result<Option<Peer>> {
    let file = match ColumnFile::open(peer_path(shared, writer)) {
        Ok(file) => file,
        Err(Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if file.column_meta(SEEN).is_none() {
        return Ok(None);
    }
    let dots: Arena<Dot> = file.column(SEEN)?;
    let mut seen = VersionVector::default();
    seen.read_from(&dots)?;
    let retired = file
        .column::<u64>(RETIRED)?
        .iter()
        .map(|raw| raw.map(|raw| WriterId::from_raw(*raw)))
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(Peer {
        writer,
        seen,
        retired,
    }))
}

pub fn read_all(shared: &Path) -> Result<Vec<Peer>> {
    let mut peers = Vec::new();
    let listing = match std::fs::read_dir(shared.join(FOLDER)) {
        Ok(listing) => listing,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(peers),
        Err(err) => return Err(err.into()),
    };
    for entry in listing {
        let Some(writer) = named(&entry?.path()) else {
            continue;
        };
        match read(shared, writer) {
            Ok(Some(peer)) => peers.push(peer),
            Ok(None) | Err(Error::Io(_) | Error::Corrupt { .. }) => {}
            Err(err) => return Err(err),
        }
    }
    peers.sort_by_key(|peer| peer.writer);
    Ok(peers)
}

pub fn publish(shared: &Path, peer: &Peer) -> Result<()> {
    let path = peer_path(shared, peer.writer);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let file = match ColumnFile::open(&path) {
        Ok(file) => file,
        Err(Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            ColumnFileWriter::create(&path, peer.writer)?.finish()?;
            ColumnFile::open(&path)?
        }
        Err(err) => return Err(err),
    };
    let mut seen = Arena::<Dot>::new();
    seen.extend_from_slice(peer.seen.dots())?;
    let mut retired = Arena::<u64>::new();
    for writer in &peer.retired {
        retired.push(writer.raw())?;
    }
    let mut updater = file.update(peer.writer)?;
    updater.column(SEEN, &seen)?;
    updater.column(RETIRED, &retired)?;
    updater.commit()?;
    Ok(())
}

#[cfg(test)]
#[path = "peers_test.rs"]
mod peers_test;
