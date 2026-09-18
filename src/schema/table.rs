//! An entity's columns as one addressable unit.

use super::clock::VersionVector;
use super::column::{Column, ColumnSpec, Field, FieldMut, Residency, Row};
use super::entity::Entity;
use super::identity::IdColumn;
use super::names::qualified;
use super::version::{Version, VersionColumn};
use crate::error::{Error, Result};
use crate::id::Id;
use crate::storage::{ColumnFile, ColumnFileUpdater, WriterId};

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

    fn current_row_of(&self, id: Id<Self::Of>) -> Result<Option<Row>> {
        for row in self.versions_of(id)? {
            if self.versions().at(row)?.is_current() {
                return Ok(Some(row));
            }
        }
        Ok(None)
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
        self.versions_mut().supersede(held)?;
        self.ids_mut().push(id)?;
        let row = self.versions_mut().push(Version::UNSTAMPED)?;
        Ok(Row::trusted(row))
    }

    fn stamp_rows(&mut self, clock: &mut VersionVector, writer: WriterId) -> Result<()> {
        self.versions_mut().stamp(clock, writer)
    }

    fn check_versions(&self) -> Result<()> {
        let column = qualified(Self::Of::NAME, "version");
        let rows = Table::rows(self);

        for index in 0..rows {
            let row = Row::of(index, rows, Self::Of::NAME)?;
            let id = self.ids().at(row)?;

            let mut current = None;
            let mut highest = None;
            for other in self.versions_of(id)? {
                let version = self.versions().at(other)?;
                if version.is_current() && current.replace(other).is_some() {
                    return Err(Error::corrupt(
                        "version",
                        format!("{column} has two rows that are both the newest {id}"),
                    ));
                }
                if version.is_stamped() && highest.is_none_or(|(dot, _)| dot < version.dot()) {
                    highest = Some((version.dot(), other));
                }
            }

            let current = current.ok_or_else(|| {
                Error::corrupt(
                    "version",
                    format!("{column} row {index} holds {id}, and no row holds the newest one"),
                )
            })?;
            if self.versions().at(current)?.is_stamped()
                && let Some((dot, at)) = highest
                && at != current
            {
                return Err(Error::corrupt(
                    "version",
                    format!(
                        "{column} row {} is the newest {id}, and row {} counted further at {dot:?}",
                        current.index(),
                        at.index()
                    ),
                ));
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
        self.check_rows_agree()
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
    fn entity(&self) -> &'static str;

    fn row_count(&self) -> usize;

    fn column_specs(&self) -> Vec<ColumnSpec>;

    fn column_residency(&self) -> Vec<Residency>;

    fn rows_agree(&self) -> Result<()>;

    fn stamp_unstamped(&mut self, clock: &mut VersionVector, writer: WriterId) -> Result<()>;

    fn read_columns(&mut self, file: &ColumnFile) -> Result<()>;

    fn offer_columns(&self, updater: &mut ColumnFileUpdater) -> Result<()>;
}

impl<T: Table> AnyTable for T {
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

    fn stamp_unstamped(&mut self, clock: &mut VersionVector, writer: WriterId) -> Result<()> {
        Table::stamp_rows(self, clock, writer)
    }

    fn read_columns(&mut self, file: &ColumnFile) -> Result<()> {
        Table::read_from(self, file)
    }

    fn offer_columns(&self, updater: &mut ColumnFileUpdater) -> Result<()> {
        Table::offer_to(self, updater)
    }
}

#[cfg(test)]
#[path = "table_test.rs"]
mod table_test;
