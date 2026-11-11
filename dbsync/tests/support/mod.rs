//! What every integration test needs to treat a store directory as one thing.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn copy_store(from: impl AsRef<Path>, to: impl AsRef<Path>) -> std::io::Result<u64> {
    let (from, to) = (from.as_ref(), to.as_ref());
    if !from.is_dir() {
        return std::fs::copy(from, to);
    }
    std::fs::create_dir_all(to)?;
    let mut copied = 0;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copied += copy_store(entry.path(), target)?;
        } else if entry.file_name() != "lock" {
            copied += std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(copied)
}

pub fn cache(store: impl AsRef<Path>) -> PathBuf {
    store.as_ref().join("local").join("cache.v1.r2.dbs")
}

pub fn shared_len(store: impl AsRef<Path>) -> u64 {
    let store = store.as_ref();
    let mut bytes = 0;
    let mut pending = vec![store.to_owned()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                if entry.path() != store.join("local") {
                    pending.push(entry.path());
                }
            } else {
                bytes += entry.metadata().unwrap().len();
            }
        }
    }
    bytes
}
