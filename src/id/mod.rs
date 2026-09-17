//! Opaque typed row identifier: minted once, stored in a column, never changed.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::str::FromStr;

mod uuid;

pub use uuid::Uuid;

use crate::error::{Error, Result};

pub struct Id<T: ?Sized> {
    uuid: Uuid,
    marker: PhantomData<fn() -> T>,
}

impl<T: ?Sized> Id<T> {
    pub const NIL: Self = Self::from_uuid(Uuid::NIL);

    pub fn mint() -> Self {
        Self::from_uuid(Uuid::now_v7())
    }

    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self {
            uuid,
            marker: PhantomData,
        }
    }

    pub const fn uuid(self) -> Uuid {
        self.uuid
    }

    pub const fn is_nil(self) -> bool {
        self.uuid.is_nil()
    }

    pub const fn cast<U: ?Sized>(self) -> Id<U> {
        Id {
            uuid: self.uuid,
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
        self.uuid == other.uuid
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
        self.uuid.cmp(&other.uuid)
    }
}

impl<T: ?Sized> Hash for Id<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uuid.hash(state);
    }
}

impl<T: ?Sized> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.uuid, f)
    }
}

impl<T: ?Sized> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let full = std::any::type_name::<T>();
        let short = full.rsplit("::").next().unwrap_or(full);
        write!(f, "Id<{short}>({})", self.uuid)
    }
}

impl<T: ?Sized> FromStr for Id<T> {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        Ok(Self::from_uuid(text.parse()?))
    }
}

#[cfg(test)]
#[path = "id_test.rs"]
mod id_test;
