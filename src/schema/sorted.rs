//! A column kept beside a run of its keys in order, so rows can be found by a range of values.

use std::ops::{Bound, RangeBounds};

use super::column::{Column, ColumnKind, ColumnSpec, Residency, Row};
use super::entity::Entity;
use super::indexed::Keyed;
use super::names::sorted_name;
use super::nullable::Maybe;
use super::reference::RefColumn;
use crate::error::{Error, Result};
use crate::id::Id;
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater};

const PRESENT: u128 = 1 << 64;
const FOLD_AT_LEAST: usize = 256;
const FOLD_FRACTION: usize = 16;

pub trait SortKey: Copy {
    fn sort_key(self) -> u128;
}

macro_rules! unsigned_sort_by_their_value {
    ($($number:ty),*) => {
        $(
            impl SortKey for $number {
                fn sort_key(self) -> u128 {
                    PRESENT | u128::from(self)
                }
            }
        )*
    };
}

unsigned_sort_by_their_value!(u8, u16, u32, u64, bool);

macro_rules! signed_sort_with_their_sign_flipped {
    ($($number:ty),*) => {
        $(
            impl SortKey for $number {
                fn sort_key(self) -> u128 {
                    PRESENT | u128::from((i64::from(self) as u64) ^ (1 << 63))
                }
            }
        )*
    };
}

signed_sort_with_their_sign_flipped!(i8, i16, i32, i64);

impl<T: SortKey> SortKey for Maybe<T> {
    fn sort_key(self) -> u128 {
        self.0.map_or(0, SortKey::sort_key)
    }
}

pub fn key_bounds<T: SortKey>(range: &impl RangeBounds<T>) -> Option<(u128, u128)> {
    let low = match range.start_bound() {
        Bound::Included(value) => value.sort_key(),
        Bound::Excluded(value) => value.sort_key().checked_add(1)?,
        Bound::Unbounded => PRESENT,
    };
    let high = match range.end_bound() {
        Bound::Included(value) => value.sort_key(),
        Bound::Excluded(value) => value.sort_key().checked_sub(1)?,
        Bound::Unbounded => PRESENT | u128::from(u64::MAX),
    };
    (low <= high).then_some((low, high))
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SortEntry<P> {
    pub prefix: P,
    pub key: u128,
    pub row: u64,
}

impl<P: Record> Record for SortEntry<P> {
    const SIZE: usize = P::SIZE + size_of::<u128>() + size_of::<u64>();

    fn encode(&self, out: &mut [u8]) {
        self.prefix.encode(&mut out[..P::SIZE]);
        self.key.encode(&mut out[P::SIZE..P::SIZE + 16]);
        self.row.encode(&mut out[P::SIZE + 16..]);
    }

    fn decode(src: &[u8]) -> Self {
        Self {
            prefix: P::decode(&src[..P::SIZE]),
            key: u128::decode(&src[P::SIZE..P::SIZE + 16]),
            row: u64::decode(&src[P::SIZE + 16..]),
        }
    }
}

pub trait Prefix: Record + Ord + std::fmt::Debug {}

impl Prefix for () {}

impl<T: ?Sized + 'static> Prefix for Id<T> {}

pub struct SortedRun<P> {
    run: Arena<SortEntry<P>>,
    column: String,
}

impl<P: Prefix> Clone for SortedRun<P> {
    fn clone(&self) -> Self {
        Self {
            run: self.run.clone(),
            column: self.column.clone(),
        }
    }
}

impl<P: Prefix> SortedRun<P> {
    pub fn new(column: String) -> Self {
        Self {
            run: Arena::new(),
            column,
        }
    }

    pub fn len(&self) -> usize {
        self.run.len()
    }

    pub fn is_empty(&self) -> bool {
        self.run.is_empty()
    }

    pub fn range(&self, prefix: P, low: u128, high: u128) -> Result<Vec<SortEntry<P>>> {
        let mut at = self.first_at_least(&prefix, low)?;
        let mut found = Vec::new();
        while at < self.run.len() {
            let entry = *self.run.row(at)?;
            if (&entry.prefix, entry.key) > (&prefix, high) {
                break;
            }
            found.push(entry);
            at += 1;
        }
        Ok(found)
    }

    pub fn wants_fold(&self, rows: usize, force: bool) -> bool {
        let tail = rows.saturating_sub(self.run.len());
        tail > 0 && (force || tail >= FOLD_AT_LEAST.max(self.run.len() / FOLD_FRACTION))
    }

    pub fn fold(&mut self, mut tail: Vec<SortEntry<P>>) -> Result<()> {
        let held = self.run.len();
        let rows = held + tail.len();
        tail.sort_unstable();
        let mut merged = Arena::new();
        let mut seen = vec![false; rows];
        let mut last: Option<SortEntry<P>> = None;
        let mut from_run = 0;
        let mut from_tail = tail.into_iter().peekable();
        loop {
            let next_run = match from_run < held {
                true => Some(*self.run.row(from_run)?),
                false => None,
            };
            let entry = match (next_run, from_tail.peek()) {
                (Some(run), Some(tail)) if tail < &run => from_tail.next(),
                (Some(run), _) => {
                    from_run += 1;
                    Some(run)
                }
                (None, Some(_)) => from_tail.next(),
                (None, None) => None,
            };
            let Some(entry) = entry else { break };
            let row = entry.row as usize;
            if row >= rows || seen[row] || last.is_some_and(|last| last >= entry) {
                return Err(self.damaged(format!("would file row {row} out of order or twice")));
            }
            seen[row] = true;
            last = Some(entry);
            merged.push(entry)?;
        }
        self.run = merged;
        Ok(())
    }

    pub fn audit(
        &self,
        rows: usize,
        mut expected: impl FnMut(usize) -> Result<(Option<P>, u128)>,
    ) -> Result<()> {
        let held = self.run.len();
        if held > rows {
            return Err(self.damaged(format!("files {held} rows of a column holding {rows}")));
        }
        let mut seen = vec![false; held];
        let mut last: Option<SortEntry<P>> = None;
        for at in 0..held {
            let entry = *self.run.row(at)?;
            let row = entry.row as usize;
            if row >= held || seen[row] {
                return Err(self.damaged(format!("files row {row} twice or out of reach")));
            }
            if last.is_some_and(|last| last >= entry) {
                return Err(self.damaged(format!("files row {row} out of order")));
            }
            let (prefix, key) = expected(row)?;
            if key != entry.key || prefix.is_some_and(|prefix| prefix != entry.prefix) {
                return Err(self.damaged(format!("files row {row} under a key it does not hold")));
            }
            seen[row] = true;
            last = Some(entry);
        }
        Ok(())
    }

    pub fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: sorted_name(entity, field),
            elem_size: <SortEntry<P> as Record>::SIZE,
            kind: ColumnKind::Sorted,
            counts_rows: false,
        });
    }

    pub fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        out.push(Residency {
            name: sorted_name(entity, field),
            resident: self.run.resident_chunks(),
            chunks: self.run.chunk_count(),
        });
    }

    pub fn read_from(
        &mut self,
        entity: &str,
        field: &str,
        file: &ColumnFile,
        rows: usize,
    ) -> Result<()> {
        let name = sorted_name(entity, field);
        self.run = match file.column_meta(&name) {
            Some(held)
                if held.elem_size as usize == <SortEntry<P> as Record>::SIZE
                    && held.rows as usize <= rows =>
            {
                file.column(&name)?
            }
            _ => Arena::new(),
        };
        Ok(())
    }

    pub fn offer_to(
        &self,
        entity: &str,
        field: &str,
        updater: &mut ColumnFileUpdater,
    ) -> Result<()> {
        updater.column(&sorted_name(entity, field), &self.run)
    }

    fn first_at_least(&self, prefix: &P, low: u128) -> Result<usize> {
        let (mut from, mut to) = (0, self.run.len());
        while from < to {
            let mid = from + (to - from) / 2;
            let entry = self.run.row(mid)?;
            if (&entry.prefix, entry.key) < (prefix, low) {
                from = mid + 1;
            } else {
                to = mid;
            }
        }
        Ok(from)
    }

    fn damaged(&self, detail: String) -> Error {
        Error::corrupt("sorted index", format!("{} {detail}", self.column))
    }
}

pub struct Sorted<C: Keyed, P = ()> {
    column: C,
    run: SortedRun<P>,
}

impl<C: Keyed, P: Prefix> Clone for Sorted<C, P> {
    fn clone(&self) -> Self {
        Self {
            column: self.column.clone(),
            run: self.run.clone(),
        }
    }
}

impl<C: Keyed, P: Prefix> Sorted<C, P>
where
    C::Key: SortKey,
{
    pub fn named(column: &str) -> Self {
        Self {
            column: C::default(),
            run: SortedRun::new(column.to_owned()),
        }
    }

    pub fn column(&self) -> &C {
        &self.column
    }

    pub fn run(&self) -> &SortedRun<P> {
        &self.run
    }

    pub fn push(&mut self, value: C::Seek<'_>) -> Result<usize> {
        self.column.append(value, None)?;
        Ok(Column::rows(&self.column) - 1)
    }

    pub fn key_at(&self, row: usize) -> Result<u128> {
        Ok(self.column.key_at(row)?.sort_key())
    }

    fn fold_with(&mut self, force: bool, prefix_at: impl Fn(usize) -> Result<P>) -> Result<()> {
        let rows = Column::rows(&self.column);
        if !self.run.wants_fold(rows, force) {
            return Ok(());
        }
        let mut tail = Vec::with_capacity(rows - self.run.len());
        for row in self.run.len()..rows {
            tail.push(SortEntry {
                prefix: prefix_at(row)?,
                key: self.key_at(row)?,
                row: row as u64,
            });
        }
        self.run.fold(tail)
    }
}

impl<C: Keyed> Sorted<C, ()>
where
    C::Key: SortKey,
{
    pub fn fold(&mut self, force: bool) -> Result<()> {
        self.fold_with(force, |_| Ok(()))
    }

    pub fn between(&self, low: u128, high: u128) -> Result<Vec<(u128, Row)>> {
        let mut found: Vec<(u128, Row)> = self
            .run
            .range((), low, high)?
            .into_iter()
            .map(|entry| (entry.key, Row::trusted(entry.row as usize)))
            .collect();
        for row in self.run.len()..Column::rows(&self.column) {
            let key = self.key_at(row)?;
            if (low..=high).contains(&key) {
                found.push((key, Row::trusted(row)));
            }
        }
        Ok(found)
    }
}

impl<C: Keyed, Parent: Entity> Sorted<C, Id<Parent>>
where
    C::Key: SortKey,
{
    pub fn fold_within<X: Entity>(
        &mut self,
        parents: &RefColumn<Parent, X>,
        force: bool,
    ) -> Result<()> {
        self.fold_with(force, |row| parents.get(Row::trusted(row)))
    }

    pub fn audit_within<X: Entity>(&self, parents: &RefColumn<Parent, X>) -> Result<()> {
        self.run.audit(Column::rows(&self.column), |row| {
            Ok((Some(parents.get(Row::trusted(row))?), self.key_at(row)?))
        })
    }

    pub fn children_between<X: Entity>(
        &self,
        parent: Id<Parent>,
        low: u128,
        high: u128,
        parents: &RefColumn<Parent, X>,
    ) -> Result<Vec<(u128, Row)>> {
        let mut found: Vec<(u128, Row)> = self
            .run
            .range(parent, low, high)?
            .into_iter()
            .map(|entry| (entry.key, Row::trusted(entry.row as usize)))
            .collect();
        for row in parents.child_rows_of(parent)? {
            if row.index() < self.run.len() {
                continue;
            }
            let key = self.key_at(row.index())?;
            if (low..=high).contains(&key) {
                found.push((key, row));
            }
        }
        Ok(found)
    }

    pub fn children<X: Entity>(
        &self,
        parent: Id<Parent>,
        parents: &RefColumn<Parent, X>,
    ) -> Result<Vec<(u128, Row)>> {
        self.children_between(parent, 0, u128::MAX, parents)
    }
}

impl<C: Keyed, P: Prefix> Column for Sorted<C, P>
where
    C::Key: SortKey,
{
    fn unused_heap_bytes(&self) -> Result<u64> {
        self.column.unused_heap_bytes()
    }

    fn compact_storage(&mut self) -> Result<bool> {
        self.column.compact_storage()
    }

    fn append_value_bytes(&mut self, bytes: &[u8]) -> Result<usize> {
        self.column.append_value_bytes(bytes)
    }

    fn value_bytes(&self, row: Row) -> Result<Vec<u8>> {
        self.column.value_bytes(row)
    }

    fn schema_type(&self) -> String {
        self.column.schema_type()
    }

    fn rows(&self) -> usize {
        Column::rows(&self.column)
    }

    fn touch_tail(&self) -> Result<()> {
        self.column.touch_tail()
    }

    fn make_room(&self, rows: usize) -> Result<()> {
        self.column.make_room(rows)
    }

    fn audit(&self) -> Result<()> {
        self.run.audit(Column::rows(&self.column), |row| {
            Ok((None, self.key_at(row)?))
        })
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        self.column.describe(entity, field, out);
        self.run.describe(entity, field, out);
    }

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        self.column.residency(entity, field, out);
        self.run.residency(entity, field, out);
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.column.read_from(entity, field, file)?;
        self.run
            .read_from(entity, field, file, Column::rows(&self.column))
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        self.column.offer_to(entity, field, updater)?;
        self.run.offer_to(entity, field, updater)
    }
}

#[cfg(test)]
#[path = "sorted_test.rs"]
mod sorted_test;
