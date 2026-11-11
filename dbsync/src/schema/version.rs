//! The write that made a row, and whether a later write has replaced it.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::marker::PhantomData;

use super::clock::VersionVector;
use super::column::{Column, ColumnKind, ColumnSpec, Residency, Row};
use super::entity::Entity;
use super::index::{HashIndex, IndexKey};
use super::names::qualified;
use crate::error::{Error, Result};
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater, WriterId};

const SUPERSEDED: u64 = 1;
const REMOVED: u64 = 1 << 1;
const RESTORED: u64 = 1 << 2;
const HIDDEN: u64 = 1 << 3;
const PURGED: u64 = 1 << 4;
const KNOWN: u64 = SUPERSEDED | REMOVED | RESTORED | PURGED;

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

impl IndexKey for Dot {
    fn raw(self) -> u128 {
        (u128::from(self.writer.raw()) << 64) | u128::from(self.counter)
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

    pub const fn is_purged(self) -> bool {
        self.flags & PURGED != 0
    }

    pub const fn is_current(self) -> bool {
        !self.is_superseded()
    }

    pub const fn is_live(self) -> bool {
        self.is_current() && !self.is_removed() && self.flags & HIDDEN == 0
    }

    pub const fn is_restoration(self) -> bool {
        self.flags & RESTORED != 0
    }

    pub const fn restoration(self) -> Self {
        Self {
            dot: self.dot,
            flags: RESTORED,
        }
    }

    // Also removed, so a build that predates purging still hides the row.
    pub const fn purge(self) -> Self {
        Self {
            dot: self.dot,
            flags: REMOVED | PURGED,
        }
    }

    pub const fn supersede(self) -> Self {
        Self {
            dot: self.dot,
            flags: self.flags | SUPERSEDED,
        }
    }

    pub const fn hide(self) -> Self {
        Self {
            dot: self.dot,
            flags: self.flags | HIDDEN,
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
        if self.is_restoration() {
            f.write_str(" restored")?;
        }
        if self.is_purged() {
            f.write_str(" purged")?;
        }
        if self.flags & HIDDEN != 0 {
            f.write_str(" hidden")?;
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
    held_parents: Arena<Dot>,
    held_ends: Arena<u64>,
    unsettled: BTreeMap<usize, Vec<Parent>>,
    by_dot: HashIndex<Dot>,
    view: super::View,
    stamped: Option<usize>,
    marker: PhantomData<fn() -> E>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Parent {
    Published(Dot),
    Pending(usize),
}

fn filed(dot: Dot, row: usize) -> Dot {
    match dot.counter() {
        0 => Dot::new(WriterId::ANONYMOUS, u64::MAX - row as u64),
        _ => dot,
    }
}

impl<E: Entity> Default for VersionColumn<E> {
    fn default() -> Self {
        Self {
            versions: Arena::new(),
            held_parents: Arena::new(),
            held_ends: Arena::new(),
            unsettled: BTreeMap::new(),
            by_dot: HashIndex::new(qualified(E::NAME, "version")),
            view: super::View::default(),
            stamped: Some(0),
            marker: PhantomData,
        }
    }
}

impl<E: Entity> Clone for VersionColumn<E> {
    fn clone(&self) -> Self {
        Self {
            versions: self.versions.clone(),
            held_parents: self.held_parents.clone(),
            held_ends: self.held_ends.clone(),
            unsettled: self.unsettled.clone(),
            by_dot: self.by_dot.clone(),
            view: self.view.fork(),
            stamped: self.stamped,
            marker: PhantomData,
        }
    }
}

impl<E: Entity> VersionColumn<E> {
    pub fn push(&mut self, version: Version) -> Result<usize> {
        self.push_with(version, Vec::new())
    }

    pub fn push_with(&mut self, version: Version, parents: Vec<Parent>) -> Result<usize> {
        self.view.check_mutable()?;
        if self.row_for_dot(version.dot())?.is_some() {
            return Err(Error::corrupt("revision", "duplicate revision dot"));
        }
        let stamped = self.unstamped_from()?;
        self.by_dot
            .append(filed(version.dot(), self.versions.len()))?;
        let row = self.versions.push(version)?;
        self.unsettled.insert(row, parents);
        self.stamped = Some(match version.is_stamped() && stamped == row {
            true => row + 1,
            false => stamped,
        });
        self.settle_parents()?;
        Ok(row)
    }

    fn settle_parents(&mut self) -> Result<()> {
        while self.held_ends.len() < self.versions.len() {
            let row = self.held_ends.len();
            if !self.versions.row(row)?.is_stamped() {
                break;
            }
            let Some(parents) = self.unsettled.get(&row) else {
                return Err(Error::corrupt("revision", "missing parent list"));
            };
            let mut dots = Vec::with_capacity(parents.len());
            for parent in parents {
                match parent {
                    Parent::Published(dot) => dots.push(*dot),
                    Parent::Pending(_) => return Ok(()),
                }
            }
            self.held_parents.extend_from_slice(&dots)?;
            self.held_ends.push(self.held_parents.len() as u64)?;
            self.unsettled.remove(&row);
        }
        Ok(())
    }

    pub fn raw_at(&self, row: Row) -> Result<Version> {
        self.versions.row(row.index()).copied()
    }

    pub fn view(&self) -> &super::View {
        &self.view
    }

    pub fn bind(&mut self, view: super::View) {
        self.view = view;
    }

    pub fn row_for_dot(&self, dot: Dot) -> Result<Option<Row>> {
        if dot.counter() == 0 || dot.writer().is_anonymous() {
            return Ok(None);
        }
        Ok(self.by_dot.row_of(dot)?.map(Row::trusted))
    }

    pub fn parents(&self, row: Row) -> Result<Vec<Parent>> {
        let at = row.index();
        if at >= self.held_ends.len() {
            return self
                .unsettled
                .get(&at)
                .cloned()
                .ok_or_else(|| Error::corrupt("revision", "missing parent list"));
        }
        let start = match at {
            0 => 0,
            _ => *self.held_ends.row(at - 1)?,
        };
        let end = *self.held_ends.row(at)?;
        let (start, end) = (start as usize, end as usize);
        if end < start || end > self.held_parents.len() {
            return Err(Error::corrupt("revision", "invalid parent span"));
        }
        Ok(self
            .held_parents
            .range(start, end - start)?
            .iter()
            .copied()
            .map(Parent::Published)
            .collect())
    }

    pub fn parent_rows(&self, row: Row) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        for parent in self.parents(row)? {
            match parent {
                Parent::Published(dot) => rows.extend(self.row_for_dot(dot)?),
                Parent::Pending(at) if at < self.versions.len() => rows.push(Row::trusted(at)),
                Parent::Pending(_) => {
                    return Err(Error::corrupt("revision", "missing pending parent"));
                }
            }
        }
        Ok(rows)
    }

    pub fn lacks_a_parent(&self, row: Row) -> Result<bool> {
        for parent in self.parents(row)? {
            if let Parent::Published(dot) = parent
                && self.row_for_dot(dot)?.is_none()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn parent_of(&self, row: Row) -> Result<Parent> {
        let version = self.raw_at(row)?;
        Ok(if version.is_stamped() {
            Parent::Published(version.dot())
        } else {
            Parent::Pending(row.index())
        })
    }

    pub fn ancestors(&self, row: Row) -> Result<BTreeSet<usize>> {
        let mut pending = self.parent_rows(row)?;
        let mut visited = BTreeSet::new();
        while let Some(parent) = pending.pop() {
            if parent == row {
                return Err(Error::corrupt("revision", "ancestry cycle"));
            }
            if visited.insert(parent.index()) {
                pending.extend(self.parent_rows(parent)?);
            }
        }
        Ok(visited)
    }

    pub fn descends(&self, row: Row, ancestor: Row) -> Result<bool> {
        Ok(self.ancestors(row)?.contains(&ancestor.index()))
    }

    pub fn audit_graph(&self) -> Result<()> {
        let mut marks = vec![0u8; self.versions.len()];
        for root in 0..marks.len() {
            let mut stack = vec![(root, false)];
            while let Some((row, exiting)) = stack.pop() {
                if exiting {
                    marks[row] = 2;
                    continue;
                }
                if marks[row] == 2 {
                    continue;
                }
                if marks[row] == 1 {
                    return Err(Error::corrupt("revision", "ancestry cycle"));
                }
                marks[row] = 1;
                stack.push((row, true));
                for parent in self.parent_rows(Row::trusted(row))? {
                    stack.push((parent.index(), false));
                }
            }
        }
        Ok(())
    }

    pub fn withdraw(&mut self, row: Row) -> Result<()> {
        self.view.check_mutable()?;
        let held = self.raw_at(row)?;
        if held.is_stamped() {
            return Err(Error::invalid("a published revision is immutable"));
        }
        self.versions.set(row.index(), held.withdraw())
    }

    pub fn unstamped_from(&self) -> Result<usize> {
        if let Some(stamped) = self.stamped {
            return Ok(stamped);
        }
        let rows = self.versions.len();
        match rows.checked_sub(1) {
            Some(last) if !self.versions.row(last)?.is_stamped() => {
                for row in 0..rows {
                    if !self.versions.row(row)?.is_stamped() {
                        return Ok(row);
                    }
                }
                Ok(rows)
            }
            _ => Ok(rows),
        }
    }

    pub fn stamp(&mut self, clock: &mut VersionVector, writer: WriterId) -> Result<()> {
        self.view.check_mutable()?;
        for row in self.unstamped_from()?..self.versions.len() {
            let held = *self.versions.row(row)?;
            if !held.is_stamped() {
                let dot = clock.tick(writer)?;
                self.versions.set(row, held.stamped_with(dot))?;
                self.by_dot.repoint_row(row, dot)?;
            }
            let Some(parents) = self.unsettled.get(&row).cloned() else {
                continue;
            };
            let mut published = Vec::new();
            for parent in parents {
                published.push(match parent {
                    Parent::Published(dot) => dot,
                    Parent::Pending(at) if at < row => {
                        let dot = self.versions.row(at)?.dot();
                        if dot.counter() == 0 {
                            return Err(Error::corrupt("revision", "unstamped parent"));
                        }
                        dot
                    }
                    _ => {
                        return Err(Error::corrupt(
                            "revision",
                            "pending ancestry is not ordered",
                        ));
                    }
                });
            }
            published.sort();
            published.dedup();
            self.unsettled
                .insert(row, published.into_iter().map(Parent::Published).collect());
        }
        self.stamped = Some(self.versions.len());
        self.settle_parents()
    }

    fn index_every_dot(&mut self) -> Result<()> {
        let mut index = HashIndex::new(qualified(E::NAME, "version"));
        for row in 0..self.versions.len() {
            index.append(filed(self.versions.row(row)?.dot(), row))?;
        }
        self.by_dot = index;
        Ok(())
    }
}

impl<E: Entity> Column for VersionColumn<E> {
    fn schema_type(&self) -> String {
        "version".to_owned()
    }

    fn rows(&self) -> usize {
        self.versions.len()
    }

    fn touch_tail(&self) -> Result<()> {
        self.versions.touch_tail()?;
        self.by_dot.touch_tail()
    }

    fn make_room(&self, rows: usize) -> Result<()> {
        self.versions.touch_tail()?;
        self.by_dot.make_room(rows)
    }

    fn audit(&self) -> Result<()> {
        let column = qualified(E::NAME, "version");
        if self.by_dot.rows() != self.versions.len() {
            return Err(Error::corrupt(
                "revision",
                "dot index disagrees with revisions",
            ));
        }
        for row in 0..self.versions.len() {
            let held = self.versions.row(row)?;
            held.check(&column, row)?;
            if held.is_stamped() && self.row_for_dot(held.dot())? != Some(Row::trusted(row)) {
                return Err(Error::corrupt("revision", "duplicated persisted dot"));
            }
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
        self.by_dot.describe(entity, field, out);
        out.push(ColumnSpec {
            name: qualified(entity, &format!("{field}.parent_ends")),
            elem_size: 8,
            kind: ColumnKind::Version,
            counts_rows: false,
        });
        out.push(ColumnSpec {
            name: qualified(entity, &format!("{field}.parents")),
            elem_size: Dot::SIZE,
            kind: ColumnKind::Version,
            counts_rows: false,
        });
    }

    fn residency(&self, entity: &str, field: &str, out: &mut Vec<Residency>) {
        self.versions.residency(entity, field, out);
        self.by_dot.residency(entity, field, out);
        out.push(Residency {
            name: qualified(entity, &format!("{field}.parent_ends")),
            resident: self.held_ends.resident_chunks(),
            chunks: self.held_ends.chunk_count(),
        });
        out.push(Residency {
            name: qualified(entity, &format!("{field}.parents")),
            resident: self.held_parents.resident_chunks(),
            chunks: self.held_parents.chunk_count(),
        });
    }

    fn read_from(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        self.versions.read_from(entity, field, file)?;
        let ends: Arena<u64> = file.column(&qualified(entity, &format!("{field}.parent_ends")))?;
        let parents: Arena<Dot> = file.column(&qualified(entity, &format!("{field}.parents")))?;
        if ends.len() != self.versions.len() {
            return Err(Error::corrupt(
                "revision",
                "parent spans disagree with revisions",
            ));
        }
        self.held_ends = ends;
        self.held_parents = parents;
        self.unsettled.clear();
        self.by_dot.read_from(entity, field, file)?;
        if !self.by_dot.agrees_with(self.versions.len()) {
            self.index_every_dot()?;
        }
        self.stamped = None;
        Ok(())
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        self.versions.offer_to(entity, field, updater)?;
        self.by_dot.offer_to(entity, field, updater)?;
        let ends_name = qualified(entity, &format!("{field}.parent_ends"));
        let parents_name = qualified(entity, &format!("{field}.parents"));
        if self.held_ends.len() == self.versions.len() {
            updater.column(&ends_name, &self.held_ends)?;
            return updater.column(&parents_name, &self.held_parents);
        }
        let mut ends = self.held_ends.clone();
        let mut parents = self.held_parents.clone();
        for row in ends.len()..self.versions.len() {
            for parent in self.parents(Row::trusted(row))? {
                let Parent::Published(dot) = parent else {
                    return Err(Error::invalid("cannot persist pending parents"));
                };
                parents.push(dot)?;
            }
            ends.push(parents.len() as u64)?;
        }
        updater.column(&ends_name, &ends)?;
        updater.column(&parents_name, &parents)
    }

    fn rebase(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        file.rebase(&qualified(entity, field), &mut self.versions)?;
        self.by_dot.rebase(entity, field, file)?;
        file.rebase(
            &qualified(entity, &format!("{field}.parent_ends")),
            &mut self.held_ends,
        )?;
        file.rebase(
            &qualified(entity, &format!("{field}.parents")),
            &mut self.held_parents,
        )
    }
}

#[cfg(test)]
#[path = "version_test.rs"]
mod version_test;
