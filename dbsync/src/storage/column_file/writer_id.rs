//! Stable identity of whatever wrote a commit.

use crate::storage::hash::fnv1a;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WriterId(u64);

impl WriterId {
    pub const ANONYMOUS: WriterId = WriterId(0);

    pub fn from_device_id(device_id: &str) -> Self {
        Self(fnv1a(device_id.as_bytes()))
    }

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn is_anonymous(self) -> bool {
        self.0 == 0
    }
}

impl std::fmt::Debug for WriterId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_anonymous() {
            f.write_str("WriterId(anonymous)")
        } else {
            write!(f, "WriterId({:#018x})", self.0)
        }
    }
}

#[cfg(test)]
#[path = "writer_id_test.rs"]
mod writer_id_test;
