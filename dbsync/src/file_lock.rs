//! Advisory whole-file locks. std's `File::lock` family is not implemented
//! for Android, where `flock` itself works.

use std::fs::File;
use std::io;

pub trait FileLock {
    /// Blocks until this process holds the file's exclusive lock.
    fn lock_exclusive(&self) -> io::Result<()>;
    /// False when another holder has it.
    fn try_lock_exclusive(&self) -> io::Result<bool>;
    fn release_lock(&self) -> io::Result<()>;
}

#[cfg(not(target_os = "android"))]
impl FileLock for File {
    fn lock_exclusive(&self) -> io::Result<()> {
        self.lock()
    }

    fn try_lock_exclusive(&self) -> io::Result<bool> {
        match self.try_lock() {
            Ok(()) => Ok(true),
            Err(std::fs::TryLockError::WouldBlock) => Ok(false),
            Err(std::fs::TryLockError::Error(err)) => Err(err),
        }
    }

    fn release_lock(&self) -> io::Result<()> {
        self.unlock()
    }
}

#[cfg(target_os = "android")]
impl FileLock for File {
    fn lock_exclusive(&self) -> io::Result<()> {
        use rustix::fs::{FlockOperation, flock};
        loop {
            match flock(self, FlockOperation::LockExclusive) {
                Err(rustix::io::Errno::INTR) => continue,
                result => return Ok(result?),
            }
        }
    }

    fn try_lock_exclusive(&self) -> io::Result<bool> {
        use rustix::fs::{FlockOperation, flock};
        match flock(self, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(true),
            Err(rustix::io::Errno::WOULDBLOCK) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    fn release_lock(&self) -> io::Result<()> {
        use rustix::fs::{FlockOperation, flock};
        Ok(flock(self, FlockOperation::Unlock)?)
    }
}
