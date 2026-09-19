//! Two parents held as one key, so no two rows of a join may name the same pair.

use std::fmt;

use super::column::{Column, ColumnKind, ColumnSpec, Residency, Row};
use super::entity::Entity;
use super::index::{IndexKey, Keys};
use super::indexed::{Keyed, Unique};
use super::names::qualified;
use super::reference::RefColumn;
use crate::error::{Error, Result};
use crate::id::Id;
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater, fnv1a};

pub type Distinct<A, B> = Unique<PairColumn<A, B>>;

pub struct Pair<A: Entity, B: Entity> {
    left: Id<A>,
    right: Id<B>,
}

impl<A: Entity, B: Entity> Pair<A, B> {
    const HALF: usize = <Id<A> as Record>::SIZE;

    pub fn of(left: Id<A>, right: Id<B>) -> Self {
        Self { left, right }
    }

    pub fn left(self) -> Id<A> {
        self.left
    }

    pub fn right(self) -> Id<B> {
        self.right
    }

    pub fn is_whole(self) -> bool {
        !self.left.is_nil() && !self.right.is_nil()
    }
}

impl<A: Entity, B: Entity> Clone for Pair<A, B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A: Entity, B: Entity> Copy for Pair<A, B> {}

impl<A: Entity, B: Entity> PartialEq for Pair<A, B> {
    fn eq(&self, other: &Self) -> bool {
        self.left == other.left && self.right == other.right
    }
}

impl<A: Entity, B: Entity> Eq for Pair<A, B> {}

impl<A: Entity, B: Entity> fmt::Debug for Pair<A, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({} {}, {} {})", A::NAME, self.left, B::NAME, self.right)
    }
}

impl<A: Entity, B: Entity> Record for Pair<A, B> {
    const SIZE: usize = <Id<A> as Record>::SIZE + <Id<B> as Record>::SIZE;

    fn encode(&self, out: &mut [u8]) {
        self.left.encode(&mut out[..Self::HALF]);
        self.right.encode(&mut out[Self::HALF..]);
    }

    fn decode(src: &[u8]) -> Self {
        Self {
            left: Id::decode(&src[..Self::HALF]),
            right: Id::decode(&src[Self::HALF..]),
        }
    }
}

impl<A: Entity, B: Entity> IndexKey for Pair<A, B> {
    fn raw(self) -> u128 {
        let mut both = [0u8; 2 * size_of::<u128>()];
        self.encode(&mut both);
        u128::from(fnv1a(&both))
    }
}

pub struct PairColumn<A: Entity, B: Entity> {
    pairs: Arena<Pair<A, B>>,
}

impl<A: Entity, B: Entity> Default for PairColumn<A, B> {
    fn default() -> Self {
        Self {
            pairs: Arena::new(),
        }
    }
}

impl<A: Entity, B: Entity> Clone for PairColumn<A, B> {
    fn clone(&self) -> Self {
        Self {
            pairs: self.pairs.clone(),
        }
    }
}

impl<A: Entity, B: Entity> Column for PairColumn<A, B> {
    fn value_bytes(&self, row: Row) -> Result<Vec<u8>> {
        self.pairs.value_bytes(row)
    }

    fn rows(&self) -> usize {
        self.pairs.len()
    }

    fn touch_tail(&self) -> Result<()> {
        self.pairs.touch_tail()
    }

    fn make_room(&self, rows: usize) -> Result<()> {
        self.pairs.make_room(rows)
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: qualified(entity, field),
            elem_size: <Pair<A, B> as Record>::SIZE,
            kind: ColumnKind::Pair {
                left: A::NAME,
                right: B::NAME,
            },
            counts_rows: true,
        });
    }

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        self.pairs.residency(entity, field, out);
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.pairs.read_from(entity, field, file)
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        self.pairs.offer_to(entity, field, updater)
    }
}

impl<A: Entity, B: Entity> Keys for PairColumn<A, B> {
    type Key = Pair<A, B>;

    fn key_at(&self, row: usize) -> Result<Self::Key> {
        self.pairs.row(row).copied()
    }
}

impl<A: Entity, B: Entity> Keyed for PairColumn<A, B> {
    type Seek<'a> = Pair<A, B>;

    fn key_for(value: Self::Seek<'_>) -> Self::Key {
        value
    }

    fn absent(value: Self::Seek<'_>) -> bool {
        !value.is_whole()
    }

    fn append(&mut self, value: Self::Seek<'_>, _shared: Option<Self::Key>) -> Result<Self::Key> {
        self.pairs.push(value)?;
        Ok(value)
    }

    fn write(
        &mut self,
        row: Row,
        value: Self::Seek<'_>,
        _shared: Option<Self::Key>,
    ) -> Result<Self::Key> {
        self.pairs.set(row.index(), value)?;
        Ok(value)
    }
}

pub fn check_pairs<A: Entity, B: Entity, J: Entity>(
    filed: &Distinct<A, B>,
    left: &RefColumn<A, J>,
    right: &RefColumn<B, J>,
) -> Result<()> {
    let rows = Column::rows(filed);

    for row in 0..rows {
        let at = Row::of(row, Column::rows(left), J::NAME)?;
        let held = Pair::of(left.get(at)?, right.get(at)?);
        let pair = filed.column().key_at(row)?;
        if pair != held {
            return Err(Error::corrupt(
                "pair",
                format!(
                    "{} row {row} is filed under {pair:?}, and the row names {held:?}",
                    filed.name()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "pair_test.rs"]
mod pair_test;
