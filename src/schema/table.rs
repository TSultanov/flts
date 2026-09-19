//! An entity's columns as one addressable unit.

use super::clock::VersionVector;
use super::column::{Column, ColumnSpec, Field, FieldMut, Residency, Row};
use super::entity::Entity;
use super::identity::IdColumn;
use super::merge::{Concurrent, Conflict, Taking};
use super::names::qualified;
use super::version::{Dot, Version, VersionColumn};
use super::{Candidate, Claim, Parent, Reference, View};
use crate::error::{Error, Result};
use crate::id::Id;
use crate::storage::{ColumnFile, ColumnFileUpdater, WriterId};
use std::collections::BTreeSet;

pub trait Table: Default + Clone {
    type Of: Entity;

    fn ids(&self) -> &IdColumn<Self::Of>;

    fn ids_mut(&mut self) -> &mut IdColumn<Self::Of>;

    fn versions(&self) -> &VersionColumn<Self::Of>;

    fn versions_mut(&mut self) -> &mut VersionColumn<Self::Of>;

    fn fields(&self) -> Vec<Field<'_>>;

    fn fields_mut(&mut self) -> Vec<FieldMut<'_>>;

    fn rows(&self) -> usize {
        Column::rows(self.ids())
    }

    fn versions_of(&self, id: Id<Self::Of>) -> Result<Vec<Row>> {
        self.ids().rows_of(id)
    }

    fn heads(&self, id: Id<Self::Of>) -> Result<Vec<Row>> {
        let rows = self.versions_of(id)?;
        let mut ancestors = BTreeSet::new();
        for row in &rows {
            for parent in self.versions().parent_rows(*row)? {
                ancestors.insert(parent.index());
            }
        }
        Ok(rows
            .into_iter()
            .filter(|row| !ancestors.contains(&row.index()))
            .collect())
    }

    fn winner(&self, id: Id<Self::Of>) -> Result<Option<Row>> {
        let rows = self.versions_of(id)?;
        let mut deletes = Vec::new();
        for row in &rows {
            if self.versions().raw_at(*row)?.is_removed() {
                deletes.push(*row);
            }
        }
        let mut eligible = Vec::new();
        for row in self.heads(id)? {
            if self.versions().raw_at(row)?.is_removed() {
                continue;
            }
            let ancestors = self.versions().ancestors(row)?;
            if deletes
                .iter()
                .all(|deletion| ancestors.contains(&deletion.index()))
            {
                eligible.push(row);
            }
        }
        if eligible.is_empty() {
            for deletion in &deletes {
                let mut superseded = false;
                for other in &deletes {
                    if other != deletion && self.versions().descends(*other, *deletion)? {
                        superseded = true;
                        break;
                    }
                }
                if !superseded {
                    eligible.push(*deletion);
                }
            }
        }
        let mut ranked = Vec::new();
        for row in eligible {
            let version = self.versions().raw_at(row)?;
            ranked.push(((!version.is_stamped(), version.dot(), row.index()), row));
        }
        Ok(ranked
            .into_iter()
            .max_by_key(|(rank, _)| *rank)
            .map(|(_, row)| row))
    }

    fn current_row_of(&self, id: Id<Self::Of>) -> Result<Option<Row>> {
        self.winner(id)
    }

    fn logical_row_of(&self, id: Id<Self::Of>) -> Result<Row> {
        let row = self
            .winner(id)?
            .ok_or_else(|| Error::invalid(format!("{} has no row with id {id}", Self::Of::NAME)))?;
        if self.versions().raw_at(row)?.is_removed() {
            return Err(Error::invalid("the row is deleted; restore it explicitly"));
        }
        Ok(row)
    }

    fn claims(&self, _row: Row) -> Result<Vec<Claim>> {
        Ok(Vec::new())
    }

    fn references(&self, _row: Row) -> Result<Vec<Reference>> {
        Ok(Vec::new())
    }

    fn payload(&self, row: Row) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        for field in self.fields() {
            if field.name == "id" || field.name == "version" {
                continue;
            }
            let bytes = field.column.value_bytes(row)?;
            super::encode_record(&(bytes.len() as u64), &mut out);
            out.extend(bytes);
        }
        Ok(out)
    }

    fn schema(&self) -> String {
        let fields: Vec<_> = self
            .fields()
            .iter()
            .map(|field| (field.name, field.column.schema_type()))
            .collect();
        format!("{} {fields:?}", Self::Of::NAME)
    }

    fn refresh(&self) -> Result<()> {
        let mut ids = Vec::new();
        let mut unique = BTreeSet::new();
        for index in 0..Table::rows(self) {
            let id = self.id_at(Row::trusted(index))?;
            ids.push(id.uuid());
            unique.insert(id.uuid());
        }
        let mut candidates = Vec::new();
        for uuid in unique {
            let id = Id::from_uuid(uuid);
            if let Some(row) = self.winner(id)? {
                let version = self.versions().raw_at(row)?;
                self.versions().select(&self.versions_of(id)?, row);
                candidates.push(Candidate {
                    entity: Self::Of::NAME,
                    id: uuid,
                    dot: version.dot(),
                    pending: (!version.is_stamped()).then_some(row.index()),
                    deleted: version.is_removed(),
                    claims: self.claims(row)?,
                    references: self.references(row)?,
                });
            }
        }
        self.versions().identify(ids);
        self.versions().view().replace(Self::Of::NAME, candidates);
        Ok(())
    }

    fn refresh_one(&self, id: Id<Self::Of>) -> Result<()> {
        if let Some(row) = self.winner(id)? {
            let rows = self.versions_of(id)?;
            let version = self.versions().raw_at(row)?;
            self.versions().select(&rows, row);
            self.versions().identify_rows(&rows, id.uuid());
            self.versions().view().upsert(Candidate {
                entity: Self::Of::NAME,
                id: id.uuid(),
                dot: version.dot(),
                pending: (!version.is_stamped()).then_some(row.index()),
                deleted: version.is_removed(),
                claims: self.claims(row)?,
                references: self.references(row)?,
            });
        }
        Ok(())
    }

    fn conflicts(&self) -> Result<Vec<Conflict>> {
        let mut conflicts = Vec::new();
        let mut ids = BTreeSet::new();
        for index in 0..Table::rows(self) {
            ids.insert(self.id_at(Row::trusted(index))?.uuid());
        }
        for uuid in ids {
            let id = Id::from_uuid(uuid);
            let Some(winner) = self.winner(id)? else {
                continue;
            };
            let version = self.versions().raw_at(winner)?;
            for row in self.heads(id)? {
                if row == winner
                    || self.versions().descends(winner, row)?
                    || self.versions().descends(row, winner)?
                {
                    continue;
                }
                let other = self.versions().raw_at(row)?;
                if version.is_removed() == other.is_removed()
                    && self.payload(winner)? == self.payload(row)?
                {
                    continue;
                }
                conflicts.push(Conflict::Concurrent(Concurrent {
                    entity: Self::Of::NAME,
                    id: uuid,
                    kept: version.dot(),
                    dropped: other.dot(),
                }));
            }
        }
        conflicts.sort();
        Ok(conflicts)
    }

    fn revision_row(&self, id: Id<Self::Of>, dot: Dot) -> Result<Row> {
        let row = self
            .versions()
            .row_for_dot(dot)
            .ok_or_else(|| Error::invalid("unknown revision"))?;
        if self.id_at(row)? != id {
            return Err(Error::invalid("revision belongs to a different entity id"));
        }
        Ok(row)
    }

    fn live_row_of(&self, id: Id<Self::Of>) -> Result<Option<Row>> {
        match self.current_row_of(id)? {
            Some(row) if self.versions().at(row)?.is_live() => Ok(Some(row)),
            _ => Ok(None),
        }
    }

    fn row_of(&self, id: Id<Self::Of>) -> Result<Row> {
        self.live_row_of(id)?
            .ok_or_else(|| Error::invalid(format!("{} has no row with id {id}", Self::Of::NAME)))
    }

    fn live_among(&self, rows: Vec<Row>) -> Result<Vec<Row>> {
        let mut kept = Vec::new();
        for row in rows {
            if self.versions().at(row)?.is_live() {
                kept.push(row);
            }
        }
        Ok(kept)
    }

    fn live_one(&self, rows: Vec<Row>) -> Result<Option<Row>> {
        for row in rows {
            if self.versions().at(row)?.is_live() {
                return Ok(Some(row));
            }
        }
        Ok(None)
    }

    fn id_at(&self, row: Row) -> Result<Id<Self::Of>> {
        self.ids().at(row)
    }

    fn mint(&mut self) -> Result<Id<Self::Of>> {
        let id = Id::mint();
        self.ids_mut().push(id)?;
        self.versions_mut().push(Version::UNSTAMPED)?;
        Ok(id)
    }

    fn revise(&mut self, id: Id<Self::Of>) -> Result<Row> {
        let held = self.current_row_of(id)?.ok_or_else(|| {
            Error::invalid(format!(
                "{} has no row with id {id} to write over",
                Self::Of::NAME
            ))
        })?;
        if self.versions().raw_at(held)?.is_removed() {
            return Err(Error::invalid("restore deleted rows explicitly"));
        }
        let parents = self
            .heads(id)?
            .into_iter()
            .map(|row| self.versions().parent_of(row))
            .collect::<Result<_>>()?;
        self.versions_mut().supersede(held)?;
        self.ids_mut().push(id)?;
        let row = self.versions_mut().push_with(Version::UNSTAMPED, parents)?;
        Ok(Row::trusted(row))
    }

    fn withdraw(&mut self, id: Id<Self::Of>) -> Result<()> {
        let held = self.current_row_of(id)?.ok_or_else(|| {
            Error::invalid(format!(
                "{} has no row with id {id} to take out",
                Self::Of::NAME
            ))
        })?;
        self.versions_mut().withdraw(held)?;
        self.refresh_one(id)
    }

    fn is_published(&self) -> bool {
        self.versions().unstamped_from() == Table::rows(self)
    }

    fn rows_by_dot(&self) -> Result<Vec<Row>> {
        let rows = Table::rows(self);
        let mut order: Vec<Row> = (0..rows)
            .map(|index| Row::of(index, rows, Self::Of::NAME))
            .collect::<Result<_>>()?;
        let mut said = Ok(());
        order.sort_by_key(|row| match self.versions().at(*row) {
            Ok(version) => version.dot(),
            Err(err) => {
                said = Err(err);
                Dot::new(WriterId::ANONYMOUS, 0)
            }
        });
        said.map(|()| order)
    }

    fn how_to_take(
        &self,
        of: Id<Self::Of>,
        theirs: Version,
        _seen: &VersionVector,
        _told: &VersionVector,
    ) -> Result<Taking> {
        if let Some(row) = self.versions().row_for_dot(theirs.dot()) {
            if self.id_at(row)? != of {
                return Err(Error::corrupt("revision", "dot reused for another id"));
            }
            return Ok(Taking::Known);
        }
        Ok(Taking::Fresh)
    }

    fn adopt_revision(
        &mut self,
        of: Id<Self::Of>,
        version: Version,
        parents: Vec<Parent>,
    ) -> Result<Row> {
        self.ids_mut().push(of)?;
        self.versions_mut()
            .push_with(version, parents)
            .map(Row::trusted)
    }

    fn recover_row(&mut self, _id: Id<Self::Of>, _dot: Dot, _restore: bool) -> Result<()> {
        Err(Error::unsupported(
            "this table does not implement revision recovery",
        ))
    }

    fn begin_recovery(&mut self, id: Id<Self::Of>, restore: bool) -> Result<Row> {
        self.versions().view().check_mutable()?;
        let current = self
            .winner(id)?
            .ok_or_else(|| Error::invalid("unknown entity id"))?;
        if self.versions().raw_at(current)?.is_removed() != restore {
            return Err(Error::invalid(
                "use restore for deleted rows and resolve for live rows",
            ));
        }
        let parents = self
            .heads(id)?
            .into_iter()
            .map(|row| self.versions().parent_of(row))
            .collect::<Result<_>>()?;
        let version = if restore {
            Version::UNSTAMPED.restoration()
        } else {
            Version::UNSTAMPED
        };
        self.ids_mut().push(id)?;
        self.versions_mut()
            .push_with(version, parents)
            .map(Row::trusted)
    }

    fn adopt(&mut self, of: Id<Self::Of>, version: Version) -> Result<Row> {
        self.ids_mut().push(of)?;
        let row = self.versions_mut().push(version)?;
        Ok(Row::trusted(row))
    }

    fn settle(&mut self, of: Id<Self::Of>) -> Result<()> {
        let winner = self
            .winner(of)?
            .ok_or_else(|| Error::invalid("unknown entity id"))?;
        self.versions().select(&self.versions_of(of)?, winner);
        Ok(())
    }

    fn yield_among(&mut self, rows: &[Row]) -> Result<Option<(Row, Vec<Row>)>> {
        let Some(first) = rows.first().copied() else {
            return Ok(None);
        };
        if rows.len() < 2 {
            return Ok(None);
        }

        let mut kept = first;
        for row in rows {
            let held = self.versions().at(*row)?.dot();
            if held > self.versions().at(kept)?.dot() {
                kept = *row;
            }
        }
        let mut given = Vec::new();
        for row in rows {
            if *row != kept {
                self.versions().hide(*row);
                given.push(*row);
            }
        }
        Ok(Some((kept, given)))
    }

    fn stamp_rows(&mut self, clock: &mut VersionVector, writer: WriterId) -> Result<()> {
        self.versions_mut().stamp(clock, writer)?;
        self.refresh()
    }

    fn check_versions(&self) -> Result<()> {
        self.versions().audit_graph()?;
        for index in 0..Table::rows(self) {
            let row = Row::trusted(index);
            let id = self.id_at(row)?;
            let version = self.versions().raw_at(row)?;
            let parents = self.versions().parent_rows(row)?;
            if version.is_superseded() || (version.is_removed() && version.is_restoration()) {
                return Err(Error::corrupt(
                    "revision",
                    "invalid persisted operation flags",
                ));
            }
            let mut seen = BTreeSet::new();
            for parent in &parents {
                if !seen.insert(parent.index()) || self.id_at(*parent)? != id {
                    return Err(Error::corrupt(
                        "revision",
                        "duplicate parent or parent belonging to another id",
                    ));
                }
                if version.is_stamped() && !self.versions().raw_at(*parent)?.is_stamped() {
                    return Err(Error::corrupt(
                        "revision",
                        "published revision has a pending parent",
                    ));
                }
            }
            if !version.is_removed() && !version.is_restoration() {
                for parent in parents {
                    if self.versions().raw_at(parent)?.is_removed() {
                        return Err(Error::corrupt(
                            "revision",
                            "only restoration can follow a deletion",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn specs(&self) -> Vec<ColumnSpec> {
        let mut out = Vec::new();
        for field in self.fields() {
            field.column.describe(Self::Of::NAME, field.name, &mut out);
        }
        out
    }

    fn residency(&self) -> Vec<Residency> {
        let mut out = Vec::new();
        for field in self.fields() {
            field.column.residency(Self::Of::NAME, field.name, &mut out);
        }
        out
    }

    fn check_rows_agree(&self) -> Result<()> {
        let fields = self.fields();
        let Some(first) = fields.first() else {
            return Ok(());
        };
        let want = first.column.rows();

        for field in &fields[1..] {
            let found = field.column.rows();
            if found != want {
                return Err(Error::corrupt(
                    "table",
                    format!(
                        "{} holds {want} rows but {} holds {found}",
                        qualified(Self::Of::NAME, first.name),
                        qualified(Self::Of::NAME, field.name)
                    ),
                ));
            }
        }
        Ok(())
    }

    fn audit(&self) -> Result<()> {
        for field in self.fields() {
            field.column.audit()?;
        }
        self.check_versions()
    }

    fn reserve(&self) -> Result<()> {
        self.versions().view().check_mutable()?;
        let wanted = Table::rows(self) + 1;
        for field in self.fields() {
            field.column.make_room(wanted)?;
        }
        Ok(())
    }

    fn read_from(&mut self, file: &ColumnFile) -> Result<()> {
        for field in self.fields_mut() {
            field.column.read_from(Self::Of::NAME, field.name, file)?;
        }
        self.check_rows_agree()?;
        self.check_versions()?;
        self.refresh()
    }

    fn offer_to(&self, updater: &mut ColumnFileUpdater) -> Result<()> {
        self.check_rows_agree()?;
        for field in self.fields() {
            field.column.offer_to(Self::Of::NAME, field.name, updater)?;
        }
        Ok(())
    }
}

pub trait AnyTable {
    fn identities(&self) -> Result<Vec<crate::Uuid>>;
    fn all_references(&self) -> Result<Vec<Reference>>;
    fn revision_records(&self) -> Result<Vec<super::RevisionRecord>>;

    fn bind_view(&mut self, view: View);
    fn refresh_view(&self) -> Result<()>;
    fn schema_descriptor(&self) -> String;
    fn row_conflicts(&self) -> Result<Vec<Conflict>>;
    fn revision_owners(&self) -> Result<Vec<(Dot, crate::Uuid)>>;

    fn entity(&self) -> &'static str;

    fn row_count(&self) -> usize;

    fn column_specs(&self) -> Vec<ColumnSpec>;

    fn column_residency(&self) -> Vec<Residency>;

    fn rows_agree(&self) -> Result<()>;

    fn published(&self) -> bool;

    fn stamp_unstamped(&mut self, clock: &mut VersionVector, writer: WriterId) -> Result<()>;

    fn read_columns(&mut self, file: &ColumnFile) -> Result<()>;

    fn offer_columns(&self, updater: &mut ColumnFileUpdater) -> Result<()>;
}

impl<T: Table> AnyTable for T {
    fn identities(&self) -> Result<Vec<crate::Uuid>> {
        (0..Table::rows(self))
            .map(|index| self.id_at(Row::trusted(index)).map(|id| id.uuid()))
            .collect()
    }

    fn all_references(&self) -> Result<Vec<Reference>> {
        let mut references = Vec::new();
        for index in 0..Table::rows(self) {
            references.extend(self.references(Row::trusted(index))?);
        }
        Ok(references)
    }

    fn revision_records(&self) -> Result<Vec<super::RevisionRecord>> {
        let mut out = Vec::new();
        for index in 0..Table::rows(self) {
            let row = Row::trusted(index);
            let parents = self
                .versions()
                .parents(row)?
                .iter()
                .map(|parent| match parent {
                    Parent::Published(dot) => Ok(*dot),
                    Parent::Pending(_) => {
                        Err(Error::invalid("pending revision has no published identity"))
                    }
                })
                .collect::<Result<_>>()?;
            out.push(super::RevisionRecord {
                entity: T::Of::NAME,
                id: self.id_at(row)?.uuid(),
                version: self.versions().raw_at(row)?,
                parents,
                payload: self.payload(row)?,
            });
        }
        Ok(out)
    }

    fn bind_view(&mut self, view: View) {
        self.versions_mut().bind(view);
    }

    fn refresh_view(&self) -> Result<()> {
        self.refresh()
    }

    fn schema_descriptor(&self) -> String {
        self.schema()
    }

    fn row_conflicts(&self) -> Result<Vec<Conflict>> {
        self.conflicts()
    }

    fn revision_owners(&self) -> Result<Vec<(Dot, crate::Uuid)>> {
        let mut out = Vec::new();
        for index in 0..Table::rows(self) {
            let row = Row::trusted(index);
            let version = self.versions().raw_at(row)?;
            if version.is_stamped() {
                out.push((version.dot(), self.id_at(row)?.uuid()));
            }
        }
        Ok(out)
    }

    fn entity(&self) -> &'static str {
        T::Of::NAME
    }

    fn row_count(&self) -> usize {
        Table::rows(self)
    }

    fn column_specs(&self) -> Vec<ColumnSpec> {
        Table::specs(self)
    }

    fn column_residency(&self) -> Vec<Residency> {
        Table::residency(self)
    }

    fn rows_agree(&self) -> Result<()> {
        Table::check_rows_agree(self)
    }

    fn published(&self) -> bool {
        Table::is_published(self)
    }

    fn stamp_unstamped(&mut self, clock: &mut VersionVector, writer: WriterId) -> Result<()> {
        Table::stamp_rows(self, clock, writer)
    }

    fn read_columns(&mut self, file: &ColumnFile) -> Result<()> {
        Table::read_from(self, file)
    }

    fn offer_columns(&self, updater: &mut ColumnFileUpdater) -> Result<()> {
        self.versions().view().check_mutable()?;
        Table::offer_to(self, updater)
    }
}

#[cfg(test)]
#[path = "table_test.rs"]
mod table_test;
