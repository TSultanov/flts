//! The write that made a row, and whether a later write has replaced it.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::marker::PhantomData;
use std::sync::RwLock;

use super::clock::VersionVector;
use super::column::{Column, ColumnKind, ColumnSpec, Residency, Row};
use super::entity::Entity;
use super::names::qualified;
use crate::error::{Error, Result};
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater, WriterId};

const SUPERSEDED: u64 = 1;
const REMOVED: u64 = 1 << 1;
const RESTORED: u64 = 1 << 2;
const HIDDEN: u64 = 1 << 3;
const KNOWN: u64 = SUPERSEDED | REMOVED | RESTORED;

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
        if self.is_restoration() {
            f.write_str(" restored")?;
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
    parents: Vec<Vec<Parent>>,
    held_parents: Arena<Dot>,
    held_ends: Arena<u64>,
    by_dot: HashMap<Dot, usize>,
    superseded: RwLock<BTreeSet<usize>>,
    row_ids: RwLock<Vec<crate::Uuid>>,
    view: super::View,
    stamped: usize,
    marker: PhantomData<fn() -> E>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Parent {
    Published(Dot),
    Pending(usize),
}

impl<E: Entity> Default for VersionColumn<E> {
    fn default() -> Self {
        Self {
            versions: Arena::new(),
            parents: Vec::new(),
            held_parents: Arena::new(),
            held_ends: Arena::new(),
            by_dot: HashMap::new(),
            superseded: RwLock::new(BTreeSet::new()),
            row_ids: RwLock::new(Vec::new()),
            view: super::View::default(),
            stamped: 0,
            marker: PhantomData,
        }
    }
}

impl<E: Entity> Clone for VersionColumn<E> {
    fn clone(&self) -> Self {
        Self {
            versions: self.versions.clone(),
            parents: self.parents.clone(),
            held_parents: self.held_parents.clone(),
            held_ends: self.held_ends.clone(),
            by_dot: self.by_dot.clone(),
            superseded: RwLock::new(self.superseded.read().expect("versions lock").clone()),
            row_ids: RwLock::new(self.row_ids.read().expect("versions lock").clone()),
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
        if version.is_stamped() && self.by_dot.contains_key(&version.dot()) {
            return Err(Error::corrupt("revision", "duplicate revision dot"));
        }
        let row = self.versions.push(version)?;
        self.parents.push(parents);
        if version.is_stamped() {
            self.by_dot.insert(version.dot(), row);
        }
        if version.is_stamped() && self.stamped == row {
            self.stamped = row + 1;
        }
        self.settle_parents()?;
        Ok(row)
    }

    fn settle_parents(&mut self) -> Result<()> {
        while self.held_ends.len() < self.versions.len() {
            let row = self.held_ends.len();
            if !self.versions.row(row)?.is_stamped() {
                break;
            }
            let mut dots = Vec::with_capacity(self.parents[row].len());
            for parent in &self.parents[row] {
                match parent {
                    Parent::Published(dot) => dots.push(*dot),
                    Parent::Pending(_) => return Ok(()),
                }
            }
            self.held_parents.extend_from_slice(&dots)?;
            self.held_ends.push(self.held_parents.len() as u64)?;
        }
        Ok(())
    }

    pub fn at(&self, row: Row) -> Result<Version> {
        let mut version = self.raw_at(row)?;
        if self
            .superseded
            .read()
            .expect("versions lock")
            .contains(&row.index())
        {
            version = version.supersede();
        }
        if let Some(id) = self.row_ids.read().expect("versions lock").get(row.index())
            && self
                .view
                .status(E::NAME, *id)
                .is_some_and(|s| s != super::RowStatus::Visible)
        {
            version.flags |= HIDDEN;
        }
        Ok(version)
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

    pub fn identify(&self, ids: Vec<crate::Uuid>) {
        *self.row_ids.write().expect("versions lock") = ids;
    }

    pub fn identify_rows(&self, rows: &[Row], id: crate::Uuid) {
        let mut ids = self.row_ids.write().expect("versions lock");
        ids.resize(self.versions.len(), crate::Uuid::NIL);
        for row in rows {
            ids[row.index()] = id;
        }
    }

    pub fn row_for_dot(&self, dot: Dot) -> Option<Row> {
        self.by_dot.get(&dot).copied().map(Row::trusted)
    }

    pub fn parents(&self, row: Row) -> Result<&[Parent]> {
        self.parents
            .get(row.index())
            .map(Vec::as_slice)
            .ok_or_else(|| Error::corrupt("revision", "missing parent list"))
    }

    pub fn parent_rows(&self, row: Row) -> Result<Vec<Row>> {
        self.parents(row)?
            .iter()
            .filter_map(|parent| match parent {
                Parent::Published(dot) => self.row_for_dot(*dot).map(Ok),
                Parent::Pending(at) if *at < self.versions.len() => Some(Ok(Row::trusted(*at))),
                Parent::Pending(_) => {
                    Some(Err(Error::corrupt("revision", "missing pending parent")))
                }
            })
            .collect()
    }

    pub fn lacks_a_parent(&self, row: Row) -> Result<bool> {
        Ok(self.parents(row)?.iter().any(|parent| match parent {
            Parent::Published(dot) => !self.by_dot.contains_key(dot),
            Parent::Pending(_) => false,
        }))
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

    pub fn select(&self, rows: &[Row], winner: Row) {
        let mut superseded = self.superseded.write().expect("versions lock");
        for row in rows {
            if *row == winner {
                superseded.remove(&row.index());
            } else {
                superseded.insert(row.index());
            }
        }
    }

    pub fn supersede(&mut self, row: Row) -> Result<()> {
        self.raw_at(row)?;
        self.superseded
            .write()
            .expect("versions lock")
            .insert(row.index());
        Ok(())
    }

    pub fn restore(&mut self, row: Row) -> Result<()> {
        self.raw_at(row)?;
        self.superseded
            .write()
            .expect("versions lock")
            .remove(&row.index());
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

    pub fn unstamped_from(&self) -> usize {
        self.stamped
    }

    pub fn stamp(&mut self, clock: &mut VersionVector, writer: WriterId) -> Result<()> {
        self.view.check_mutable()?;
        for row in self.stamped..self.versions.len() {
            let held = *self.versions.row(row)?;
            if !held.is_stamped() {
                let dot = clock.tick(writer)?;
                self.versions.set(row, held.stamped_with(dot))?;
                self.by_dot.insert(dot, row);
            }
            let parents = self.parents[row].clone();
            let mut published = Vec::new();
            for parent in parents {
                published.push(match parent {
                    Parent::Published(dot) => Parent::Published(dot),
                    Parent::Pending(at) if at < row => {
                        let dot = self.versions.row(at)?.dot();
                        if dot.counter() == 0 {
                            return Err(Error::corrupt("revision", "unstamped parent"));
                        }
                        Parent::Published(dot)
                    }
                    _ => {
                        return Err(Error::corrupt(
                            "revision",
                            "pending ancestry is not ordered",
                        ));
                    }
                });
            }
            published.sort_by_key(|parent| match parent {
                Parent::Published(dot) => *dot,
                _ => unreachable!(),
            });
            published.dedup();
            self.parents[row] = published;
        }
        self.stamped = self.versions.len();
        self.settle_parents()
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
        for (suffix, rows, width) in [
            ("parent_ends", self.parents.len(), 8),
            (
                "parents",
                self.parents.iter().map(Vec::len).sum(),
                Dot::SIZE,
            ),
        ] {
            let chunks = rows.div_ceil(crate::storage::default_chunk_rows(width));
            out.push(Residency {
                name: qualified(entity, &format!("{field}.{suffix}")),
                resident: chunks,
                chunks,
            });
        }
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
        self.parents.clear();
        self.by_dot.clear();
        self.superseded.write().expect("versions lock").clear();
        self.row_ids.write().expect("versions lock").clear();
        let mut start = 0;
        for row in 0..ends.len() {
            let end = usize::try_from(*ends.row(row)?)
                .map_err(|_| Error::corrupt("revision", "parent offset overflow"))?;
            if end < start || end > parents.len() {
                return Err(Error::corrupt("revision", "invalid parent span"));
            }
            self.parents.push(
                (start..end)
                    .map(|at| parents.row(at).copied().map(Parent::Published))
                    .collect::<Result<_>>()?,
            );
            start = end;
            let version = *self.versions.row(row)?;
            if version.is_stamped() && self.by_dot.insert(version.dot(), row).is_some() {
                return Err(Error::corrupt("revision", "duplicated persisted dot"));
            }
        }
        if start != parents.len() {
            return Err(Error::corrupt("revision", "unclaimed parent entries"));
        }
        self.held_ends = ends;
        self.held_parents = parents;
        self.stamped = (0..self.versions.len())
            .find(|row| self.versions.row(*row).is_ok_and(|v| !v.is_stamped()))
            .unwrap_or(self.versions.len());
        Ok(())
    }

    fn offer_to(&self, entity: &str, field: &str, updater: &mut ColumnFileUpdater) -> Result<()> {
        self.versions.offer_to(entity, field, updater)?;
        let ends_name = qualified(entity, &format!("{field}.parent_ends"));
        let parents_name = qualified(entity, &format!("{field}.parents"));
        if self.held_ends.len() == self.versions.len() {
            updater.column(&ends_name, &self.held_ends)?;
            return updater.column(&parents_name, &self.held_parents);
        }
        let mut ends = self.held_ends.clone();
        let mut parents = self.held_parents.clone();
        for row in &self.parents[ends.len()..] {
            for parent in row {
                let Parent::Published(dot) = parent else {
                    return Err(Error::invalid("cannot persist pending parents"));
                };
                parents.push(*dot)?;
            }
            ends.push(parents.len() as u64)?;
        }
        updater.column(&ends_name, &ends)?;
        updater.column(&parents_name, &parents)
    }

    fn rebase(&mut self, entity: &str, field: &str, file: &ColumnFile) -> Result<()> {
        file.rebase(&qualified(entity, field), &mut self.versions)?;
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
