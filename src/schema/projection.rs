//! Which row speaks for each id and whether it can be seen, kept in columns every table shares.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, RwLock};

use super::column::{ColumnKind, ColumnSpec, Residency};
use super::index::HashIndex;
use super::names::qualified;
use super::{Conflict, ReferenceConflict, RowStatus};
use crate::record::Record;
use crate::storage::{Arena, ColumnFile, ColumnFileUpdater};
use crate::{Error, Result, Uuid};

const DELETED: u8 = 1;
const SUPPRESSED: u8 = 1 << 1;
const COMBINED: u8 = 1 << 2;
const HIDDEN: u8 = DELETED | SUPPRESSED;
const PREFIX: &str = "dbsync.view";

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Reference {
    pub column: &'static str,
    pub entity: &'static str,
    pub id: Uuid,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Candidate {
    pub entity: &'static str,
    pub id: Uuid,
    pub winner: usize,
    pub deleted: bool,
    pub combined: bool,
    pub references: Vec<Reference>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Standing {
    pub winner: usize,
    pub deleted: bool,
    pub suppressed: bool,
    pub combined: bool,
}

impl Standing {
    pub const fn is_visible(self) -> bool {
        !self.deleted && !self.suppressed
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StatusChange {
    pub entity: &'static str,
    pub id: Uuid,
    pub before: Option<RowStatus>,
}

#[derive(Clone)]
struct Named {
    column: &'static str,
    parent: &'static str,
    ids: Arena<u128>,
    index: HashIndex<u128>,
}

impl Named {
    fn field(&self) -> String {
        let field = self.column.rsplit('.').next().unwrap_or(self.column);
        format!("ref.{field}")
    }
}

#[derive(Clone)]
struct Shelf {
    entity: &'static str,
    ids: Arena<u128>,
    index: HashIndex<u128>,
    winners: Arena<u64>,
    states: Arena<u8>,
    refs: Vec<Named>,
}

fn family(entity: &str) -> String {
    format!("{PREFIX}.{entity}")
}

fn unnamed(slot: usize) -> u128 {
    u128::MAX - slot as u128
}

fn filed(slot: usize, parent: u128) -> u128 {
    match parent {
        0 => unnamed(slot),
        parent => parent,
    }
}

impl Shelf {
    fn new(entity: &'static str) -> Self {
        Self {
            entity,
            ids: Arena::new(),
            index: HashIndex::new(qualified(&family(entity), "id")),
            winners: Arena::new(),
            states: Arena::new(),
            refs: Vec::new(),
        }
    }

    fn names(&mut self, column: &'static str, parent: &'static str) -> Result<usize> {
        if let Some(at) = self.refs.iter().position(|held| held.column == column) {
            return Ok(at);
        }
        let mut named = Named {
            column,
            parent,
            ids: Arena::new(),
            index: HashIndex::new(String::new()),
        };
        named.index = HashIndex::new(qualified(&family(self.entity), &named.field()));
        for slot in 0..self.ids.len() {
            named.ids.push(0)?;
            named.index.append(unnamed(slot))?;
        }
        self.refs.push(named);
        Ok(self.refs.len() - 1)
    }

    fn slot_of(&self, id: Uuid) -> Result<Option<usize>> {
        self.index.row_of(id.to_u128())
    }

    fn state(&self, slot: usize) -> Result<u8> {
        self.states.row(slot).copied()
    }

    fn standing(&self, slot: usize) -> Result<Standing> {
        let state = self.state(slot)?;
        Ok(Standing {
            winner: *self.winners.row(slot)? as usize,
            deleted: state & DELETED != 0,
            suppressed: state & SUPPRESSED != 0,
            combined: state & COMBINED != 0,
        })
    }
}

#[derive(Clone, Default)]
struct Projection {
    shelves: Vec<Shelf>,
    journal: Option<Vec<StatusChange>>,
    blocked: bool,
}

impl Projection {
    fn shelf(&self, entity: &str) -> Option<usize> {
        self.shelves.iter().position(|shelf| shelf.entity == entity)
    }

    fn shelf_for(&mut self, entity: &'static str) -> usize {
        match self.shelf(entity) {
            Some(at) => at,
            None => {
                self.shelves.push(Shelf::new(entity));
                self.shelves.len() - 1
            }
        }
    }

    fn hides(&self, parent: &str, id: u128) -> Result<bool> {
        if id == 0 {
            return Ok(false);
        }
        let Some(at) = self.shelf(parent) else {
            return Ok(false);
        };
        let shelf = &self.shelves[at];
        match shelf.index.row_of(id)? {
            Some(slot) => Ok(shelf.state(slot)? & HIDDEN != 0),
            None => Ok(true),
        }
    }

    fn reasons(&self, at: usize, slot: usize) -> Result<Vec<ReferenceConflict>> {
        let shelf = &self.shelves[at];
        let id = Uuid::from_u128(*shelf.ids.row(slot)?);
        let mut reasons = Vec::new();
        for named in &shelf.refs {
            let parent = *named.ids.row(slot)?;
            if self.hides(named.parent, parent)? {
                reasons.push(ReferenceConflict {
                    entity: shelf.entity,
                    id,
                    column: named.column,
                    parent_entity: named.parent,
                    parent: Uuid::from_u128(parent),
                });
            }
        }
        reasons.sort();
        reasons.dedup();
        Ok(reasons)
    }

    fn status(&self, at: usize, slot: usize) -> Result<RowStatus> {
        let state = self.shelves[at].state(slot)?;
        Ok(if state & DELETED != 0 {
            RowStatus::Deleted
        } else if state & SUPPRESSED != 0 {
            RowStatus::Suppressed(self.reasons(at, slot)?)
        } else {
            RowStatus::Visible
        })
    }

    fn dependants(&self, at: usize, slot: usize) -> Result<Vec<(usize, usize)>> {
        let entity = self.shelves[at].entity;
        let id = *self.shelves[at].ids.row(slot)?;
        let mut found = Vec::new();
        for (child, shelf) in self.shelves.iter().enumerate() {
            for named in &shelf.refs {
                if named.parent == entity {
                    for held in named.index.rows_of(id)? {
                        found.push((child, held));
                    }
                }
            }
        }
        Ok(found)
    }

    fn noted(&mut self, at: usize, slot: usize, before: Option<RowStatus>) -> Result<()> {
        if self.journal.is_none() {
            return Ok(());
        }
        let change = StatusChange {
            entity: self.shelves[at].entity,
            id: Uuid::from_u128(*self.shelves[at].ids.row(slot)?),
            before,
        };
        if let Some(journal) = &mut self.journal {
            journal.push(change);
        }
        Ok(())
    }

    fn settle_from(&mut self, at: usize, slot: usize) -> Result<()> {
        let mut reached = BTreeSet::from([(at, slot)]);
        let mut walk = vec![(at, slot)];
        while let Some((shelf, held)) = walk.pop() {
            for next in self.dependants(shelf, held)? {
                if reached.insert(next) {
                    walk.push(next);
                }
            }
        }
        let mut before = BTreeMap::new();
        if self.journal.is_some() {
            for &(shelf, held) in &reached {
                before.insert((shelf, held), self.status(shelf, held)?);
            }
        }
        for &(shelf, held) in &reached {
            let state = self.shelves[shelf].state(held)?;
            if state & SUPPRESSED != 0 {
                self.shelves[shelf].states.set(held, state & !SUPPRESSED)?;
            }
        }
        let mut queue: VecDeque<(usize, usize)> = reached.iter().copied().collect();
        while let Some((shelf, held)) = queue.pop_front() {
            let state = self.shelves[shelf].state(held)?;
            if state & HIDDEN != 0 {
                continue;
            }
            let mut hidden = false;
            for named in &self.shelves[shelf].refs {
                if self.hides(named.parent, *named.ids.row(held)?)? {
                    hidden = true;
                    break;
                }
            }
            if hidden {
                self.shelves[shelf].states.set(held, state | SUPPRESSED)?;
                queue.extend(self.dependants(shelf, held)?);
            }
        }
        for ((shelf, held), was) in before {
            if self.status(shelf, held)? != was {
                self.noted(shelf, held, Some(was))?;
            }
        }
        Ok(())
    }

    fn upsert(&mut self, candidate: &Candidate) -> Result<()> {
        let at = self.shelf_for(candidate.entity);
        let mut wanted = Vec::new();
        for reference in &candidate.references {
            let named = self.shelves[at].names(reference.column, reference.entity)?;
            wanted.push((named, reference.id.to_u128()));
        }
        let flags = u8::from(candidate.deleted) * DELETED + u8::from(candidate.combined) * COMBINED;
        let id = candidate.id.to_u128();

        let Some(slot) = self.shelves[at].index.row_of(id)? else {
            let shelf = &mut self.shelves[at];
            let slot = shelf.ids.push(id)?;
            shelf.index.append(id)?;
            shelf.winners.push(candidate.winner as u64)?;
            shelf.states.push(flags)?;
            for (named, held) in shelf.refs.iter_mut().enumerate() {
                let parent = wanted
                    .iter()
                    .find(|(wanted, _)| *wanted == named)
                    .map_or(0, |(_, parent)| *parent);
                held.ids.push(parent)?;
                held.index.append(filed(slot, parent))?;
            }
            self.settle_from(at, slot)?;
            return self.noted(at, slot, None);
        };

        let before = match self.journal.is_some() {
            true => Some(self.status(at, slot)?),
            false => None,
        };
        let shelf = &mut self.shelves[at];
        let state = shelf.state(slot)?;
        if *shelf.winners.row(slot)? != candidate.winner as u64 {
            shelf.winners.set(slot, candidate.winner as u64)?;
        }
        let mut moved = false;
        for (named, held) in shelf.refs.iter_mut().enumerate() {
            let parent = wanted
                .iter()
                .find(|(wanted, _)| *wanted == named)
                .map_or(0, |(_, parent)| *parent);
            if *held.ids.row(slot)? != parent {
                held.ids.set(slot, parent)?;
                held.index.repoint_row(slot, filed(slot, parent))?;
                moved = true;
            }
        }
        let kept = state & SUPPRESSED;
        if state != flags | kept {
            shelf.states.set(slot, flags | kept)?;
        }
        if moved || (state & DELETED) != (flags & DELETED) {
            self.settle_from(at, slot)?;
        }
        if let Some(before) = before
            && self.status(at, slot)? != before
            && !self.journal.as_ref().is_some_and(|journal| {
                journal
                    .iter()
                    .any(|change| change.entity == candidate.entity && change.id == candidate.id)
            })
        {
            self.noted(at, slot, Some(before))?;
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct View(Arc<RwLock<Projection>>);

impl View {
    pub fn register(
        &self,
        entity: &'static str,
        references: &[(&'static str, &'static str)],
    ) -> Result<()> {
        let mut held = self.0.write().expect("view lock");
        let at = held.shelf_for(entity);
        for (column, parent) in references {
            held.shelves[at].names(column, parent)?;
        }
        Ok(())
    }

    pub fn fork(&self) -> Self {
        Self(Arc::new(RwLock::new(
            self.0.read().expect("view lock").clone(),
        )))
    }

    pub fn check_mutable(&self) -> Result<()> {
        if self.0.read().expect("view lock").blocked {
            return Err(Error::invalid(
                "retry commit or merge_from to finish the pending publication before changing this store",
            ));
        }
        Ok(())
    }

    pub fn block(&self, blocked: bool) {
        self.0.write().expect("view lock").blocked = blocked;
    }

    pub fn upsert(&self, candidate: Candidate) -> Result<()> {
        self.0.write().expect("view lock").upsert(&candidate)
    }

    pub fn standing(&self, entity: &str, id: Uuid) -> Result<Option<Standing>> {
        let held = self.0.read().expect("view lock");
        let Some(at) = held.shelf(entity) else {
            return Ok(None);
        };
        match held.shelves[at].slot_of(id)? {
            Some(slot) => held.shelves[at].standing(slot).map(Some),
            None => Ok(None),
        }
    }

    pub fn status(&self, entity: &str, id: Uuid) -> Result<Option<RowStatus>> {
        let held = self.0.read().expect("view lock");
        let Some(at) = held.shelf(entity) else {
            return Ok(None);
        };
        match held.shelves[at].slot_of(id)? {
            Some(slot) => held.status(at, slot).map(Some),
            None => Ok(None),
        }
    }

    pub fn combined(&self, entity: &str) -> Result<Vec<Uuid>> {
        let held = self.0.read().expect("view lock");
        let Some(at) = held.shelf(entity) else {
            return Ok(Vec::new());
        };
        let shelf = &held.shelves[at];
        let mut ids = Vec::new();
        for slot in 0..shelf.ids.len() {
            if shelf.state(slot)? & COMBINED != 0 {
                ids.push(Uuid::from_u128(*shelf.ids.row(slot)?));
            }
        }
        Ok(ids)
    }

    pub fn statuses(&self) -> Result<BTreeMap<(&'static str, Uuid), RowStatus>> {
        let held = self.0.read().expect("view lock");
        let mut statuses = BTreeMap::new();
        for (at, shelf) in held.shelves.iter().enumerate() {
            for slot in 0..shelf.ids.len() {
                statuses.insert(
                    (shelf.entity, Uuid::from_u128(*shelf.ids.row(slot)?)),
                    held.status(at, slot)?,
                );
            }
        }
        Ok(statuses)
    }

    pub fn conflicts(&self) -> Result<Vec<Conflict>> {
        let held = self.0.read().expect("view lock");
        let mut conflicts = Vec::new();
        for (at, shelf) in held.shelves.iter().enumerate() {
            for slot in 0..shelf.ids.len() {
                let state = shelf.state(slot)?;
                if state & SUPPRESSED != 0 && state & DELETED == 0 {
                    conflicts.extend(held.reasons(at, slot)?.into_iter().map(Conflict::Reference));
                }
            }
        }
        Ok(conflicts)
    }

    pub fn watch(&self) {
        self.0.write().expect("view lock").journal = Some(Vec::new());
    }

    pub fn watched(&self) -> Vec<StatusChange> {
        self.0
            .write()
            .expect("view lock")
            .journal
            .take()
            .unwrap_or_default()
    }

    pub fn describe(&self, out: &mut Vec<ColumnSpec>) {
        let held = self.0.read().expect("view lock");
        for shelf in &held.shelves {
            let family = family(shelf.entity);
            out.push(ColumnSpec {
                name: qualified(&family, "id"),
                elem_size: <u128 as Record>::SIZE,
                kind: ColumnKind::Identity,
                counts_rows: false,
            });
            shelf.index.describe(&family, "id", out);
            out.push(ColumnSpec {
                name: qualified(&family, "winner"),
                elem_size: <u64 as Record>::SIZE,
                kind: ColumnKind::Version,
                counts_rows: false,
            });
            out.push(ColumnSpec {
                name: qualified(&family, "state"),
                elem_size: <u8 as Record>::SIZE,
                kind: ColumnKind::Version,
                counts_rows: false,
            });
            for named in &shelf.refs {
                out.push(ColumnSpec {
                    name: qualified(&family, &named.field()),
                    elem_size: <u128 as Record>::SIZE,
                    kind: ColumnKind::Ref {
                        parent: named.parent,
                    },
                    counts_rows: false,
                });
                named.index.describe(&family, &named.field(), out);
            }
        }
    }

    pub fn residency(&self, out: &mut Vec<Residency>) {
        let held = self.0.read().expect("view lock");
        let told = |name: String, resident: usize, chunks: usize| Residency {
            name,
            resident,
            chunks,
        };
        for shelf in &held.shelves {
            let family = family(shelf.entity);
            out.push(told(
                qualified(&family, "id"),
                shelf.ids.resident_chunks(),
                shelf.ids.chunk_count(),
            ));
            shelf.index.residency(&family, "id", out);
            out.push(told(
                qualified(&family, "winner"),
                shelf.winners.resident_chunks(),
                shelf.winners.chunk_count(),
            ));
            out.push(told(
                qualified(&family, "state"),
                shelf.states.resident_chunks(),
                shelf.states.chunk_count(),
            ));
            for named in &shelf.refs {
                out.push(told(
                    qualified(&family, &named.field()),
                    named.ids.resident_chunks(),
                    named.ids.chunk_count(),
                ));
                named.index.residency(&family, &named.field(), out);
            }
        }
    }

    pub fn read_from(&self, file: &ColumnFile) -> Result<()> {
        let mut held = self.0.write().expect("view lock");
        for shelf in &mut held.shelves {
            let family = family(shelf.entity);
            shelf.ids = file.column(&qualified(&family, "id"))?;
            shelf.index.read_from(&family, "id", file)?;
            shelf.winners = file.column(&qualified(&family, "winner"))?;
            shelf.states = file.column(&qualified(&family, "state"))?;
            let slots = shelf.ids.len();
            if shelf.winners.len() != slots
                || shelf.states.len() != slots
                || !shelf.index.agrees_with(slots)
            {
                return Err(Error::corrupt(
                    "view",
                    "columns disagree about how many ids there are",
                ));
            }
            for named in &mut shelf.refs {
                let field = named.field();
                named.ids = file.column(&qualified(&family, &field))?;
                named.index.read_from(&family, &field, file)?;
                if named.ids.len() != slots || !named.index.agrees_with(slots) {
                    return Err(Error::corrupt(
                        "view",
                        "a reference column disagrees with its ids",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn offer_to(&self, updater: &mut ColumnFileUpdater) -> Result<()> {
        let held = self.0.read().expect("view lock");
        for shelf in &held.shelves {
            let family = family(shelf.entity);
            updater.column(&qualified(&family, "id"), &shelf.ids)?;
            shelf.index.offer_to(&family, "id", updater)?;
            updater.column(&qualified(&family, "winner"), &shelf.winners)?;
            updater.column(&qualified(&family, "state"), &shelf.states)?;
            for named in &shelf.refs {
                updater.column(&qualified(&family, &named.field()), &named.ids)?;
                named.index.offer_to(&family, &named.field(), updater)?;
            }
        }
        Ok(())
    }

    pub fn rebase(&self, file: &ColumnFile) -> Result<()> {
        let mut held = self.0.write().expect("view lock");
        for shelf in &mut held.shelves {
            let family = family(shelf.entity);
            file.rebase(&qualified(&family, "id"), &mut shelf.ids)?;
            shelf.index.rebase(&family, "id", file)?;
            file.rebase(&qualified(&family, "winner"), &mut shelf.winners)?;
            file.rebase(&qualified(&family, "state"), &mut shelf.states)?;
            for named in &mut shelf.refs {
                let field = named.field();
                file.rebase(&qualified(&family, &field), &mut named.ids)?;
                named.index.rebase(&family, &field, file)?;
            }
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.0
            .read()
            .expect("view lock")
            .shelves
            .iter()
            .all(|shelf| shelf.ids.is_empty())
    }
}

pub fn encode_record<T: crate::Record>(value: &T, out: &mut Vec<u8>) {
    let at = out.len();
    out.resize(at + T::SIZE, 0);
    value.encode(&mut out[at..]);
}

pub fn encode_text(value: &str, out: &mut Vec<u8>) {
    encode_bytes(value.as_bytes(), out);
}

pub fn encode_bytes(value: &[u8], out: &mut Vec<u8>) {
    encode_record(&(value.len() as u64), out);
    out.extend_from_slice(value);
}
