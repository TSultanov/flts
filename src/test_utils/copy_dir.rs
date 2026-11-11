//! Copies of a store directory, and what its shared half holds.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else if entry.file_name() != "lock" {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

pub fn shared_files(store: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut held = BTreeMap::new();
    let mut pending = vec![store.to_owned()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                if path != store.join("local") {
                    pending.push(path);
                }
            } else {
                held.insert(
                    path.strip_prefix(store).unwrap().to_owned(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    held
}

pub fn copy_store(from: impl AsRef<Path>, to: impl AsRef<Path>) -> std::io::Result<u64> {
    if from.as_ref().is_dir() {
        copy_dir(from.as_ref(), to.as_ref());
        return Ok(0);
    }
    std::fs::copy(from, to)
}

pub fn cache(store: impl AsRef<Path>) -> PathBuf {
    store.as_ref().join("local").join("cache.v1.r2.dbs")
}
