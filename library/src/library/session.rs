use dbsync::file_lock::FileLock;
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

/// An exclusive local lease, held until every writer in the session exits.
pub(crate) struct LibraryLease {
    _file: File,
    path: PathBuf,
    pub(crate) root: PathBuf,
}

impl Drop for LibraryLease {
    fn drop(&mut self) {
        // Unlink while still locked; `acquire` re-checks identity, so an
        // opener racing this cannot end up holding the orphaned file.
        #[cfg(unix)]
        if names_same_file(&self._file, &self.path) {
            let _ = std::fs::remove_file(&self.path);
        }
        let _ = self._file.release_lock();
    }
}

/// Whether `path` still names the file we hold open.
#[cfg(unix)]
fn names_same_file(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (file.metadata(), std::fs::metadata(path)) {
        (Ok(held), Ok(current)) => held.dev() == current.dev() && held.ino() == current.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn names_same_file(_file: &File, _path: &Path) -> bool {
    true
}

impl LibraryLease {
    /// The lock file is a sibling of the root: every process derives the same
    /// path from the root alone, and it stays outside the synced folder.
    pub(crate) fn acquire(root: &Path) -> anyhow::Result<Self> {
        let root = root.canonicalize()?;
        let (Some(parent), Some(name)) = (root.parent(), root.file_name()) else {
            anyhow::bail!("library root has no parent: {}", root.display());
        };
        let mut lock_name = std::ffi::OsString::from(".");
        lock_name.push(name);
        lock_name.push(".flts-lock");
        let mut path = parent.join(lock_name);
        let open = |path: &Path| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
        };
        loop {
            let file = match open(&path) {
                Ok(file) => file,
                // Unwritable parent (mount point, read-only dir): every
                // process falls back to the same in-root path.
                Err(err)
                    if path.parent() != Some(root.as_path())
                        && matches!(
                            err.kind(),
                            std::io::ErrorKind::PermissionDenied
                                | std::io::ErrorKind::ReadOnlyFilesystem
                        ) =>
                {
                    path = root.join(".flts-lock");
                    continue;
                }
                Err(err) => return Err(err.into()),
            };
            if !file.try_lock_exclusive()? {
                anyhow::bail!("library already open: {}", root.display())
            }
            // A previous holder may have unlinked the file after we opened it.
            if names_same_file(&file, &path) {
                return Ok(Self {
                    _file: file,
                    path,
                    root,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_competing_sessions_and_releases_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library");
        std::fs::create_dir(&root).unwrap();
        let lease = LibraryLease::acquire(&root).unwrap();
        assert!(LibraryLease::acquire(&root).is_err());
        let lock = dir.path().join(".library.flts-lock");
        assert!(lock.exists());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        drop(lease);
        #[cfg(unix)]
        assert!(!lock.exists());
        assert!(LibraryLease::acquire(&root).is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn library_dropped_without_close_leaves_no_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library");
        let library = crate::library::Library::open(root).await.unwrap();
        drop(library);
        let lock = dir.path().join(".library.flts-lock");
        for _ in 0..200 {
            if !lock.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!lock.exists());
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_parent_falls_back_to_a_lock_inside_the_root() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("mount");
        let root = parent.join("library");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).unwrap();
        let lease = LibraryLease::acquire(&root).unwrap();
        assert!(root.join(".flts-lock").exists());
        assert!(LibraryLease::acquire(&root).is_err());
        drop(lease);
        assert!(!root.join(".flts-lock").exists());
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn independent_libraries_do_not_block_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let _first = LibraryLease::acquire(&first).unwrap();
        let _second = LibraryLease::acquire(&second).unwrap();
    }
}
