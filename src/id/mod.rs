//! Opaque typed row identifier: the join key between tables.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

pub struct Id<T: ?Sized> {
    raw: u64,
    marker: PhantomData<fn() -> T>,
}

impl<T: ?Sized> Id<T> {
    pub const NULL: u64 = u64::MAX;

    pub const fn from_raw(raw: u64) -> Self {
        Self {
            raw,
            marker: PhantomData,
        }
    }

    pub const fn from_index(index: usize) -> Self {
        Self::from_raw(index as u64)
    }

    pub const fn raw(self) -> u64 {
        self.raw
    }

    pub const fn index(self) -> usize {
        self.raw as usize
    }

    pub const fn cast<U: ?Sized>(self) -> Id<U> {
        Id {
            raw: self.raw,
            marker: PhantomData,
        }
    }
}

impl<T: ?Sized> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for Id<T> {}

impl<T: ?Sized> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<T: ?Sized> Eq for Id<T> {}

impl<T: ?Sized> PartialOrd for Id<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: ?Sized> Ord for Id<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.raw.cmp(&other.raw)
    }
}

impl<T: ?Sized> Hash for Id<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

impl<T: ?Sized> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let full = std::any::type_name::<T>();
        let short = full.rsplit("::").next().unwrap_or(full);
        write!(f, "Id<{short}>({})", self.raw)
    }
}

#[cfg(test)]
#[path = "id_test.rs"]
mod id_test;
