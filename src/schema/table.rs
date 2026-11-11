//! An entity's columns as one addressable unit.

use super::clock::VersionVector;
use super::column::{Column, ColumnSpec, Field, FieldMut, Residency, Row};
use super::entity::Entity;
use super::identity::IdColumn;
use super::merge::{Concurrent, Conflict};
use super::names::qualified;
use super::shard::ShardRow;
use super::version::{Dot, Version, VersionColumn};
use super::{Candidate, Parent, Reference, View};
use crate::error::{Error, Result};
use crate::id::Id;
use crate::storage::{ColumnFile, ColumnFileUpdater, WriterId};
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Adoption {
    Taken,
    Known,
    Waiting,
}

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
        let mut cut: Option<Dot> = None;
        for row in &rows {
            for parent in self.versions().parent_rows(*row)? {
                ancestors.insert(parent.index());
            }
            if self.versions().lacks_a_parent(*row)? {
                let dot = self.versions().raw_at(*row)?.dot();
                cut = Some(cut.map_or(dot, |held| held.max(dot)));
            }
        }
        let mut heads = Vec::new();
        for row in rows {
            let version = self.versions().raw_at(row)?;
            let below = cut.is_some_and(|cut| version.is_stamped() && version.dot() < cut);
            if !ancestors.contains(&row.index()) && !below {
                heads.push(row);
            }
        }
        Ok(heads)
    }

    fn covers(&self, row: Row, other: Row) -> Result<bool> {
        let ancestors = self.versions().ancestors(row)?;
        if ancestors.contains(&other.index()) {
            return Ok(true);
        }
        let sought = self.versions().raw_at(other)?;
        if !sought.is_stamped() {
            return Ok(false);
        }
        for held in ancestors.into_iter().chain([row.index()]) {
            let held = Row::trusted(held);
            if self.versions().lacks_a_parent(held)?
                && sought.dot() < self.versions().raw_at(held)?.dot()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn eligible_heads(&self, id: Id<Self::Of>) -> Result<Vec<Row>> {
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
            let mut covered = true;
            for deletion in &deletes {
                if !self.covers(row, *deletion)? {
                    covered = false;
                    break;
                }
            }
            if covered {
                eligible.push(row);
            }
        }
        eligible.sort_by_key(|row| {
            self.versions()
                .raw_at(*row)
                .map(|v| (v.dot(), row.index()))
                .ok()
        });
        Ok(eligible)
    }

    fn winner(&self, id: Id<Self::Of>) -> Result<Option<Row>> {
        let mut eligible = self.eligible_heads(id)?;
        let mut deletes = Vec::new();
        for row in self.versions_of(id)? {
            if self.versions().raw_at(row)?.is_removed() {
                deletes.push(row);
            }
        }
        if eligible.is_empty() {
            for deletion in &deletes {
                let mut superseded = false;
                for other in &deletes {
                    if other != deletion && self.covers(*other, *deletion)? {
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

    fn resolve(&self, _id: Id<Self::Of>) -> Result<()> {
        Ok(())
    }

    fn invalidate_resolved(&self) {}

    fn is_derived(&self, _id: Id<Self::Of>) -> bool {
        false
    }

    fn source_dots(&self, id: Id<Self::Of>) -> Result<Vec<Dot>> {
        self.winner(id)?
            .map(|row| self.versions().raw_at(row).map(|v| vec![v.dot()]))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    fn current_references(&self, _id: Id<Self::Of>, row: Row) -> Result<Vec<Reference>> {
        self.references(row)
    }

    fn additional_references(&self) -> Result<Vec<Reference>> {
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

    fn field_types(&self) -> Vec<(&'static str, String)> {
        self.fields()
            .iter()
            .filter(|field| field.name != "id" && field.name != "version")
            .map(|field| (field.name, field.column.schema_type()))
            .collect()
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
                self.resolve(id)?;
                let version = self.versions().raw_at(row)?;
                self.versions().select(&self.versions_of(id)?, row);
                candidates.push(Candidate {
                    entity: Self::Of::NAME,
                    id: uuid,
                    deleted: version.is_removed(),
                    references: self.current_references(id, row)?,
                });
            }
        }
        self.versions().identify(ids);
        self.versions().view().replace(Self::Of::NAME, candidates);
        Ok(())
    }

    fn refresh_one(&self, id: Id<Self::Of>) -> Result<()> {
        if let Some(row) = self.winner(id)? {
            self.resolve(id)?;
            let rows = self.versions_of(id)?;
            let version = self.versions().raw_at(row)?;
            self.versions().select(&rows, row);
            self.versions().identify_rows(&rows, id.uuid());
            self.versions().view().upsert(Candidate {
                entity: Self::Of::NAME,
                id: id.uuid(),
                deleted: version.is_removed(),
                references: self.current_references(id, row)?,
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
            conflicts.extend(self.conflicts_of(Id::from_uuid(uuid))?);
        }
        conflicts.sort();
        Ok(conflicts)
    }

    fn conflicts_of(&self, id: Id<Self::Of>) -> Result<Vec<Conflict>> {
        let uuid = id.uuid();
        let mut conflicts = Vec::new();
        let Some(winner) = self.winner(id)? else {
            return Ok(conflicts);
        };
        let version = self.versions().raw_at(winner)?;
        if self.is_derived(id) {
            conflicts.push(Conflict::Combined(super::Combined {
                entity: Self::Of::NAME,
                id: uuid,
                sources: self.source_dots(id)?,
            }));
            return Ok(conflicts);
        }
        for row in self.heads(id)? {
            if row == winner || self.covers(winner, row)? || self.covers(row, winner)? {
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

    fn in_creation_order(&self, rows: &mut [Row]) -> Result<()> {
        let mut keyed = Vec::with_capacity(rows.len());
        for row in rows.iter() {
            keyed.push((self.id_at(*row)?, *row));
        }
        keyed.sort_unstable();
        for (at, (_, row)) in keyed.into_iter().enumerate() {
            rows[at] = row;
        }
        Ok(())
    }

    fn live_in_key_order(&self, keyed: Vec<(u128, Row)>) -> Result<Vec<Row>> {
        let mut kept = Vec::with_capacity(keyed.len());
        for (key, row) in keyed {
            if self.versions().at(row)?.is_live() {
                kept.push((key, self.id_at(row)?, row));
            }
        }
        kept.sort_unstable();
        Ok(kept.into_iter().map(|(_, _, row)| row).collect())
    }

    fn settle_indexes(&mut self, _force: bool) -> Result<()> {
        Ok(())
    }

    fn audit_indexes(&self) -> Result<()> {
        Ok(())
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
        self.mint_as(id)?;
        Ok(id)
    }

    fn mint_as(&mut self, id: Id<Self::Of>) -> Result<()> {
        self.ids_mut().push(id)?;
        self.versions_mut().push(Version::UNSTAMPED)?;
        Ok(())
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

    fn merge_rows(&mut self, theirs: &Self, report: &mut super::MergeReport) -> Result<()> {
        for row in theirs.rows_by_dot()? {
            let version = theirs.versions().raw_at(row)?;
            let id = theirs.id_at(row)?;
            if let Some(held) = self.versions().row_for_dot(version.dot()) {
                if self.id_at(held)? != id
                    || self.versions().raw_at(held)? != version
                    || self.versions().parents(held)? != theirs.versions().parents(row)?
                    || self.payload(held)? != theirs.payload(row)?
                {
                    return Err(Error::corrupt(
                        "revision",
                        "a dot identifies different revisions",
                    ));
                }
                report.known += 1;
                continue;
            }
            let mut values = std::collections::BTreeMap::new();
            for field in theirs.fields() {
                if field.name != "id" && field.name != "version" {
                    values.insert(field.name, field.column.value_bytes(row)?);
                }
            }
            self.reserve()?;
            self.adopt_revision(id, version, theirs.versions().parents(row)?.to_vec())?;
            for field in self.fields_mut() {
                if field.name != "id" && field.name != "version" {
                    let bytes = values
                        .remove(field.name)
                        .ok_or_else(|| Error::corrupt("schema", "missing field during merge"))?;
                    field.column.append_value_bytes(&bytes)?;
                }
            }
            if !values.is_empty() {
                return Err(Error::corrupt("schema", "extra fields during merge"));
            }
            report.taken += 1;
        }
        self.check_versions()?;
        self.refresh()
    }

    fn prunable(
        &self,
        writer: WriterId,
        settled: &dyn Fn(Dot) -> bool,
        whole: &dyn Fn(WriterId) -> bool,
    ) -> Result<Vec<Row>> {
        let mut ids = BTreeSet::new();
        for index in 0..Table::rows(self) {
            ids.insert(self.id_at(Row::trusted(index))?);
        }
        let mut dropped = Vec::new();
        for id in ids {
            let heads = self.heads(id)?;
            let [head] = heads.as_slice() else {
                continue;
            };
            let kept = self.versions().raw_at(*head)?;
            if !kept.is_stamped() || !settled(kept.dot()) || !whole(kept.dot().writer()) {
                continue;
            }
            for row in self.versions_of(id)? {
                let held = self.versions().raw_at(row)?;
                if row != *head && held.is_stamped() && held.dot().writer() == writer {
                    dropped.push(row);
                }
            }
        }
        dropped.sort();
        Ok(dropped)
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

    fn stamp_rows(&mut self, clock: &mut VersionVector, writer: WriterId) -> Result<()> {
        self.versions_mut().stamp(clock, writer)?;
        self.refresh()
    }

    fn pending(&self) -> Result<Vec<Row>> {
        let mut pending = Vec::new();
        for index in self.versions().unstamped_from()..Table::rows(self) {
            let row = Row::trusted(index);
            if !self.versions().raw_at(row)?.is_stamped() {
                pending.push(row);
            }
        }
        Ok(pending)
    }

    fn publish(&mut self, clock: &mut VersionVector, writer: WriterId) -> Result<Vec<Row>> {
        let pending = self.pending()?;
        if pending.is_empty() {
            return Ok(pending);
        }
        self.versions_mut().stamp(clock, writer)?;
        let mut touched = BTreeSet::new();
        for row in &pending {
            touched.insert(self.id_at(*row)?);
        }
        for id in touched {
            self.refresh_one(id)?;
        }
        Ok(pending)
    }

    fn export(&self, row: Row) -> Result<ShardRow> {
        let mut values = Vec::new();
        for field in self.fields() {
            if field.name != "id" && field.name != "version" {
                values.push((field.name.to_owned(), field.column.value_bytes(row)?));
            }
        }
        let mut parents = Vec::new();
        for parent in self.versions().parents(row)? {
            match parent {
                Parent::Published(dot) => parents.push(*dot),
                Parent::Pending(_) => {
                    return Err(Error::invalid("pending revision has no published identity"));
                }
            }
        }
        Ok(ShardRow {
            id: self.id_at(row)?.uuid(),
            version: self.versions().raw_at(row)?,
            parents,
            values,
        })
    }

    fn adopt_shard_row(
        &mut self,
        theirs: &ShardRow,
        gone: &dyn Fn(Dot) -> bool,
    ) -> Result<Adoption> {
        let id = Id::from_uuid(theirs.id);
        let parents: Vec<Parent> = theirs
            .parents
            .iter()
            .copied()
            .map(Parent::Published)
            .collect();
        if !theirs.version.is_stamped() {
            return Err(Error::corrupt(
                "revision",
                "a shard holds an unpublished row",
            ));
        }
        if let Some(held) = self.versions().row_for_dot(theirs.version.dot()) {
            let mut same = self.id_at(held)? == id
                && self.versions().raw_at(held)? == theirs.version
                && self.versions().parents(held)? == parents.as_slice();
            for field in self.fields() {
                if same
                    && field.name != "id"
                    && field.name != "version"
                    && let Some(told) = theirs.value(field.name)
                {
                    same = told == field.column.value_bytes(held)?;
                }
            }
            if !same {
                return Err(Error::corrupt(
                    "revision",
                    "a dot identifies different revisions",
                ));
            }
            return Ok(Adoption::Known);
        }

        let foreign = self.fields().iter().any(|field| {
            theirs
                .value(field.name)
                .is_some_and(|told| !field.column.understands(told))
        });
        if foreign {
            return Ok(Adoption::Waiting);
        }
        let mut inherited: Option<Row> = None;
        let lacking = self.fields().iter().any(|field| {
            field.name != "id" && field.name != "version" && theirs.value(field.name).is_none()
        });
        if lacking {
            let mut candidates = Vec::new();
            let mut orphaned = false;
            for dot in &theirs.parents {
                match self.versions().row_for_dot(*dot) {
                    Some(row) => candidates.push(row),
                    None if gone(*dot) => orphaned = true,
                    None => return Ok(Adoption::Waiting),
                }
            }
            if orphaned {
                for row in self.versions_of(id)? {
                    let held = self.versions().raw_at(row)?;
                    if held.is_stamped() && held.dot() < theirs.version.dot() {
                        candidates.push(row);
                    }
                }
            }
            for row in candidates {
                let newer = match inherited {
                    Some(held) => {
                        self.versions().raw_at(held)?.dot() < self.versions().raw_at(row)?.dot()
                    }
                    None => true,
                };
                if newer {
                    inherited = Some(row);
                }
            }
        }
        let mut values = Vec::new();
        for field in self.fields() {
            if field.name == "id" || field.name == "version" {
                continue;
            }
            values.push(match (theirs.value(field.name), inherited) {
                (Some(told), _) => told.to_vec(),
                (None, Some(from)) => field.column.value_bytes(from)?,
                (None, None) => field.column.absent_value_bytes().ok_or_else(|| {
                    Error::unsupported(format!(
                        "{}.{} was not written and has no absent value",
                        Self::Of::NAME,
                        field.name
                    ))
                })?,
            });
        }
        self.reserve()?;
        self.adopt_revision(id, theirs.version, parents)?;
        let mut values = values.into_iter();
        for field in self.fields_mut() {
            if field.name != "id" && field.name != "version" {
                let bytes = values
                    .next()
                    .ok_or_else(|| Error::corrupt("schema", "missing field during merge"))?;
                field.column.append_value_bytes(&bytes)?;
            }
        }
        Ok(Adoption::Taken)
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
        self.audit_indexes()?;
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
        self.invalidate_resolved();
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
    fn unused_heap_bytes(&self) -> Result<u64>;
    fn compact_columns(&mut self) -> Result<()>;
    fn identities(&self) -> Result<Vec<crate::Uuid>>;
    fn all_references(&self) -> Result<Vec<Reference>>;
    fn revision_records(&self) -> Result<Vec<super::RevisionRecord>>;

    fn bind_view(&mut self, view: View);
    fn refresh_view(&self) -> Result<()>;
    fn described(&self) -> Vec<(String, String, String)>;

    fn optional_fields(&self) -> Vec<&'static str>;
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

    fn fold_indexes(&mut self, force: bool) -> Result<()>;

    fn offer_columns(&self, updater: &mut ColumnFileUpdater) -> Result<()>;

    fn rebase_columns(&mut self, file: &ColumnFile) -> Result<()>;

    fn field_names(&self) -> Vec<&'static str>;

    fn publish_rows(
        &mut self,
        clock: &mut VersionVector,
        writer: WriterId,
    ) -> Result<Vec<ShardRow>>;

    fn adopt_row(&mut self, row: &ShardRow, gone: &dyn Fn(Dot) -> bool) -> Result<Adoption>;

    fn keeps_history(&self) -> bool;

    fn prunable_rows(
        &self,
        writer: WriterId,
        settled: &dyn Fn(Dot) -> bool,
        whole: &dyn Fn(WriterId) -> bool,
    ) -> Result<Vec<usize>>;

    fn authored_rows(&self, writer: WriterId) -> Result<Vec<(usize, ShardRow)>>;

    fn refresh_rows(&self, ids: &[crate::Uuid]) -> Result<()>;

    fn conflicts_among(&self, ids: &[crate::Uuid]) -> Result<Vec<Conflict>>;

    fn export_rows(&self) -> Result<Vec<ShardRow>>;
}

impl<T: Table> AnyTable for T {
    fn unused_heap_bytes(&self) -> Result<u64> {
        self.fields()
            .iter()
            .try_fold(0, |sum, field| Ok(sum + field.column.unused_heap_bytes()?))
    }

    fn compact_columns(&mut self) -> Result<()> {
        for field in self.fields_mut() {
            field.column.compact_storage()?;
        }
        self.invalidate_resolved();
        Ok(())
    }

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
        references.extend(self.additional_references()?);
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

    fn described(&self) -> Vec<(String, String, String)> {
        Table::field_types(self)
            .into_iter()
            .map(|(field, held)| (T::Of::NAME.to_owned(), field.to_owned(), held))
            .collect()
    }

    fn optional_fields(&self) -> Vec<&'static str> {
        self.fields()
            .iter()
            .filter(|field| field.column.absent_value_bytes().is_some())
            .map(|field| field.name)
            .collect()
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

    fn fold_indexes(&mut self, force: bool) -> Result<()> {
        Table::settle_indexes(self, force)
    }

    fn offer_columns(&self, updater: &mut ColumnFileUpdater) -> Result<()> {
        self.versions().view().check_mutable()?;
        Table::offer_to(self, updater)
    }

    fn rebase_columns(&mut self, file: &ColumnFile) -> Result<()> {
        for field in self.fields_mut() {
            field.column.rebase(T::Of::NAME, field.name, file)?;
        }
        Ok(())
    }

    fn field_names(&self) -> Vec<&'static str> {
        self.fields()
            .iter()
            .map(|field| field.name)
            .filter(|name| *name != "id" && *name != "version")
            .collect()
    }

    fn publish_rows(
        &mut self,
        clock: &mut VersionVector,
        writer: WriterId,
    ) -> Result<Vec<ShardRow>> {
        Table::publish(self, clock, writer)?
            .into_iter()
            .map(|row| Table::export(self, row))
            .collect()
    }

    fn adopt_row(&mut self, row: &ShardRow, gone: &dyn Fn(Dot) -> bool) -> Result<Adoption> {
        Table::adopt_shard_row(self, row, gone)
    }

    fn keeps_history(&self) -> bool {
        T::Of::KEEPS.keeps_what_was_written_over()
    }

    fn prunable_rows(
        &self,
        writer: WriterId,
        settled: &dyn Fn(Dot) -> bool,
        whole: &dyn Fn(WriterId) -> bool,
    ) -> Result<Vec<usize>> {
        Ok(Table::prunable(self, writer, settled, whole)?
            .into_iter()
            .map(Row::index)
            .collect())
    }

    fn authored_rows(&self, writer: WriterId) -> Result<Vec<(usize, ShardRow)>> {
        let mut authored = Vec::new();
        for row in Table::rows_by_dot(self)? {
            let held = self.versions().raw_at(row)?;
            if held.is_stamped() && held.dot().writer() == writer {
                authored.push((row.index(), Table::export(self, row)?));
            }
        }
        Ok(authored)
    }

    fn refresh_rows(&self, ids: &[crate::Uuid]) -> Result<()> {
        for id in ids {
            Table::refresh_one(self, Id::from_uuid(*id))?;
        }
        Ok(())
    }

    fn conflicts_among(&self, ids: &[crate::Uuid]) -> Result<Vec<Conflict>> {
        let mut conflicts = Vec::new();
        for id in ids {
            conflicts.extend(Table::conflicts_of(self, Id::from_uuid(*id))?);
        }
        conflicts.sort();
        Ok(conflicts)
    }

    fn export_rows(&self) -> Result<Vec<ShardRow>> {
        (0..Table::rows(self))
            .map(|index| Table::export(self, Row::trusted(index)))
            .collect()
    }
}

#[cfg(test)]
#[path = "table_test.rs"]
mod table_test;
