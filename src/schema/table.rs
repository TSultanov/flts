//! An entity's columns as one addressable unit.

use super::column::{Column, ColumnSpec, Field, FieldMut, Residency, Row};
use super::entity::Entity;
use super::identity::IdColumn;
use super::names::qualified;
use crate::error::{Error, Result};
use crate::id::Id;
use crate::storage::{ColumnFile, ColumnFileUpdater};

pub trait Table: Default + Clone {
    type Of: Entity;

    fn ids(&self) -> &IdColumn<Self::Of>;

    fn ids_mut(&mut self) -> &mut IdColumn<Self::Of>;

    fn fields(&self) -> Vec<Field<'_>>;

    fn fields_mut(&mut self) -> Vec<FieldMut<'_>>;

    fn rows(&self) -> usize {
        Column::rows(self.ids())
    }

    fn row_of(&self, id: Id<Self::Of>) -> Result<Row> {
        self.ids().row_of(id)
    }

    fn id_at(&self, row: Row) -> Result<Id<Self::Of>> {
        self.ids().at(row)
    }

    fn mint(&mut self) -> Result<Id<Self::Of>> {
        let id = Id::mint();
        self.ids_mut().push(id)?;
        Ok(id)
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
        Ok(())
    }

    fn reserve(&self) -> Result<()> {
        for field in self.fields() {
            field.column.touch_tail()?;
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

    fn audit_columns(&self) -> Result<()>;

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

    fn audit_columns(&self) -> Result<()> {
        Table::audit(self)
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
