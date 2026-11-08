use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

/// An exclusive local lease, held until every writer in the session exits.
pub(crate) struct LibraryLease {
    _file: File,
    pub(crate) root: PathBuf,
}

impl LibraryLease {
    pub(crate) fn acquire(root: &Path, locks_dir: &Path) -> anyhow::Result<Self> {
        let root = root.canonicalize()?;
        std::fs::create_dir_all(locks_dir)?;
        // FNV collisions conservatively refuse a second session; they cannot
        // admit two writers for the same canonical root.
        use std::hash::Hasher;
        let mut hash = fnv::FnvHasher::default();
        hash.write(root.as_os_str().as_encoded_bytes());
        let path = locks_dir.join(format!("{:016x}.lock", hash.finish()));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file, root }),
            Err(std::fs::TryLockError::WouldBlock) => {
                anyhow::bail!("library already open: {}", root.display())
            }
            Err(std::fs::TryLockError::Error(err)) => Err(err.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_competing_sessions_and_releases_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let locks = tempfile::tempdir().unwrap();
        let lease = LibraryLease::acquire(root.path(), locks.path()).unwrap();
        assert!(LibraryLease::acquire(root.path(), locks.path()).is_err());
        drop(lease);
        assert!(LibraryLease::acquire(root.path(), locks.path()).is_ok());
    }

    #[test]
    fn independent_libraries_do_not_block_each_other() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let locks = tempfile::tempdir().unwrap();
        let _first = LibraryLease::acquire(first.path(), locks.path()).unwrap();
        let _second = LibraryLease::acquire(second.path(), locks.path()).unwrap();
    }
}
