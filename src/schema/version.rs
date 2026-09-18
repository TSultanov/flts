//! The write that made a row, and whether a later write has replaced it.

use std::cmp::Ordering;
use std::fmt;
use std::marker::PhantomData;

use super::clock::VersionVector;
use super::column::{Column, ColumnKind, ColumnSpec, Residency, Row};
use super::entity::Entity;
use super::names::qualified;
use crate::error::{Error, Result};
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater, WriterId};

const SUPERSEDED: u64 = 1;
const REMOVED: u64 = 1 << 1;
const KNOWN: u64 = SUPERSEDED | REMOVED;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dot {
    writer: WriterId,
    counter: u64,
}

impl Dot {
    pub const fn new(writer: WriterId, counter: u64) -> Self {
        Self { writer, counter }
    }

    pub const fn writer(self) -> WriterId {
        self.writer
    }

    pub const fn counter(self) -> u64 {
        self.counter
    }
}

impl PartialOrd for Dot {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Dot {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.counter, self.writer).cmp(&(other.counter, other.writer))
    }
}

impl fmt::Debug for Dot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#018x}@{}", self.writer.raw(), self.counter)
    }
}

impl Record for Dot {
    const SIZE: usize = 2 * size_of::<u64>();

    fn encode(&self, out: &mut [u8]) {
        out[..8].copy_from_slice(&self.writer.raw().to_le_bytes());
        out[8..].copy_from_slice(&self.counter.to_le_bytes());
    }

    fn decode(src: &[u8]) -> Self {
        Self {
            writer: WriterId::from_raw(u64::from_le_bytes(src[..8].try_into().expect("8 bytes"))),
            counter: u64::from_le_bytes(src[8..].try_into().expect("8 bytes")),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Version {
    dot: Dot,
    flags: u64,
}

impl Version {
    pub const UNSTAMPED: Self = Self::of(Dot::new(WriterId::ANONYMOUS, 0));

    pub const fn of(dot: Dot) -> Self {
        Self { dot, flags: 0 }
    }

    pub const fn is_stamped(self) -> bool {
        self.dot.counter() != 0
    }

    pub const fn stamped_with(self, dot: Dot) -> Self {
        Self {
            dot,
            flags: self.flags,
        }
    }

    pub const fn dot(self) -> Dot {
        self.dot
    }

    pub const fn is_superseded(self) -> bool {
        self.flags & SUPERSEDED != 0
    }

    pub const fn is_removed(self) -> bool {
        self.flags & REMOVED != 0
    }

    pub const fn is_current(self) -> bool {
        !self.is_superseded()
    }

    pub const fn is_live(self) -> bool {
        self.is_current() && !self.is_removed()
    }

    pub const fn supersede(self) -> Self {
        Self {
            dot: self.dot,
            flags: self.flags | SUPERSEDED,
        }
    }

    pub const fn restore(self) -> Self {
        Self {
            dot: self.dot,
            flags: self.flags & !SUPERSEDED,
        }
    }

    pub const fn withdraw(self) -> Self {
        Self {
            dot: self.dot,
            flags: self.flags | REMOVED,
        }
    }

    pub fn check(self, column: &str, row: usize) -> Result<()> {
        let unknown = self.flags & !KNOWN;
        if unknown != 0 {
            return Err(Error::unsupported(format!(
                "{column} row {row} is marked {unknown:#x}, which this build does not know how to read"
            )));
        }
        Ok(())
    }
}

impl fmt::Debug for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.dot)?;
        if self.is_superseded() {
            f.write_str(" superseded")?;
        }
        if self.is_removed() {
            f.write_str(" removed")?;
        }
        Ok(())
    }
}

impl Record for Version {
    const SIZE: usize = <Dot as Record>::SIZE + size_of::<u64>();

    fn encode(&self, out: &mut [u8]) {
        self.dot.encode(&mut out[..<Dot as Record>::SIZE]);
        out[<Dot as Record>::SIZE..].copy_from_slice(&self.flags.to_le_bytes());
    }

    fn decode(src: &[u8]) -> Self {
        Self {
            dot: Dot::decode(&src[..<Dot as Record>::SIZE]),
            flags: u64::from_le_bytes(src[<Dot as Record>::SIZE..].try_into().expect("8 bytes")),
        }
    }
}

pub struct VersionColumn<E: Entity> {
    versions: Arena<Version>,
    stamped: usize,
    marker: PhantomData<fn() -> E>,
}

impl<E: Entity> Default for VersionColumn<E> {
    fn default() -> Self {
        Self {
            versions: Arena::new(),
            stamped: 0,
            marker: PhantomData,
        }
    }
}

impl<E: Entity> Clone for VersionColumn<E> {
    fn clone(&self) -> Self {
        Self {
            versions: self.versions.clone(),
            stamped: self.stamped,
            marker: PhantomData,
        }
    }
}

impl<E: Entity> VersionColumn<E> {
    pub fn push(&mut self, version: Version) -> Result<usize> {
        let row = self.versions.push(version)?;
        if version.is_stamped() && self.stamped == row {
            self.stamped = row + 1;
        }
        Ok(row)
    }

    pub fn at(&self, row: Row) -> Result<Version> {
        self.versions.row(row.index()).copied()
    }

    pub fn supersede(&mut self, row: Row) -> Result<()> {
        let held = self.at(row)?;
        self.versions.set(row.index(), held.supersede())
    }

    pub fn restore(&mut self, row: Row) -> Result<()> {
        let held = self.at(row)?;
        self.versions.set(row.index(), held.restore())
    }

    pub fn withdraw(&mut self, row: Row) -> Result<()> {
        let held = self.at(row)?;
        self.versions.set(row.index(), held.withdraw())
    }

    pub fn unstamped_from(&self) -> usize {
        self.stamped
    }

    pub fn stamp(&mut self, clock: &mut VersionVector, writer: WriterId) -> Result<()> {
        for row in self.stamped..self.versions.len() {
            let held = *self.versions.row(row)?;
            if !held.is_stamped() {
                let dot = clock.tick(writer)?;
                self.versions.set(row, held.stamped_with(dot))?;
            }
        }
        self.stamped = self.versions.len();
        Ok(())
    }
}

impl<E: Entity> Column for VersionColumn<E> {
    fn rows(&self) -> usize {
        self.versions.len()
    }

    fn touch_tail(&self) -> Result<()> {
        self.versions.touch_tail()
    }

    fn audit(&self) -> Result<()> {
        let column = qualified(E::NAME, "version");
        for row in 0..self.versions.len() {
            self.versions.row(row)?.check(&column, row)?;
        }
        Ok(())
    }

    fn describe(&self, entity: &str, field: &str, out: &mut Vec<ColumnSpec>) {
        out.push(ColumnSpec {
            name: qualified(entity, field),
            elem_size: <Version as Record>::SIZE,
            kind: ColumnKind::Version,
            counts_rows: true,
        });
    }

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        self.versions.residency(entity, field, out);
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.versions.read_from(entity, field, file)?;
        self.stamped = self.versions.len();
        Ok(())
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        self.versions.offer_to(entity, field, updater)
    }
}

#[cfg(test)]
#[path = "version_test.rs"]
mod version_test;
