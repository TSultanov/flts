//! What every entity knows about itself: its name, its columns, and whose child it is.

use super::column::Row;
use super::entry::{Entry, EntryMut};
use super::reference::RefColumn;
use super::table::Table;
use crate::error::Result;
use crate::id::Id;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Retention {
    Newest,
    History,
}

impl Retention {
    pub const fn keeps_what_was_written_over(self) -> bool {
        matches!(self, Self::History)
    }
}

pub trait Entity: Sized + 'static {
    const NAME: &'static str;

    const KEEPS: Retention = Retention::Newest;

    type Value;

    type Table: Table<Of = Self>;

    type Row<'a, S>: From<Entry<'a, S, Self>>
    where
        S: Holds<Self> + 'a;

    type RowMut<'a, S>: From<EntryMut<'a, S, Self>>
    where
        S: Holds<Self> + 'a;
}

#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a child of `{P}`",
    label = "`{Self}` has no `One<{P}>` field",
    note = "`Many<{Self}>` groups `{Self}` rows by the column that points back at `{P}`",
    note = "give `{Self}` a single `One<{P}>` field, or drop the `Many<{Self}>` from `{P}`"
)]
pub trait Child<P: Entity>: Entity + Sized {
    fn parents(table: &Self::Table) -> &RefColumn<P, Self>;

    fn parent_at(table: &Self::Table, row: Row, _historical: bool) -> Result<Id<P>> {
        Self::parents(table).get(row)
    }

    fn child_rows(table: &Self::Table, parent: Id<P>) -> Result<Vec<Row>> {
        let mut rows = table.live_among(Self::parents(table).child_rows_of(parent)?)?;
        table.in_creation_order(&mut rows)?;
        Ok(rows)
    }
}

pub trait Holds<E: Entity> {
    fn table(&self) -> &E::Table;

    fn table_mut(&mut self) -> &mut E::Table;
}

pub trait Audited<S>: Entity {
    fn audit_in(store: &S) -> Result<()>;
}

pub trait Removed<S>: Entity {
    fn remove_in(store: &mut S, of: Id<Self>) -> Result<()>;
}

pub fn children_of<S, P, C>(store: &S, parent: Id<P>) -> Result<Vec<Id<C>>>
where
    S: Holds<P> + Holds<C>,
    P: Entity,
    C: Child<P>,
{
    let table = Holds::<C>::table(store);
    let mine = Table::ids(table);
    child_rows_of::<S, P, C>(store, parent)?
        .into_iter()
        .map(|row| mine.at(row))
        .collect()
}

pub fn child_rows_of<S, P, C>(store: &S, parent: Id<P>) -> Result<Vec<Row>>
where
    S: Holds<P> + Holds<C>,
    P: Entity,
    C: Child<P>,
{
    let table = Holds::<C>::table(store);
    C::child_rows(table, parent)
}

pub fn across_rows_of<S, A, J, B>(store: &S, from: Id<A>) -> Result<Vec<Row>>
where
    S: Holds<J> + Holds<B>,
    A: Entity,
    B: Entity,
    J: Child<A> + Child<B>,
{
    let joins = Holds::<J>::table(store);
    let far = Holds::<B>::table(store);
    let mut reached = Vec::new();

    let links = <J as Child<A>>::child_rows(joins, from)?;
    for link in links {
        let beyond = <J as Child<B>>::parent_at(joins, link, false)?;
        if let Some(row) = Table::live_row_of(far, beyond)? {
            reached.push(row);
        }
    }
    Ok(reached)
}

pub fn audit_reference<S, P, C>(store: &S) -> Result<()>
where
    S: Holds<P> + Holds<C>,
    P: Entity,
    C: Child<P>,
{
    let parents = Holds::<P>::table(store);
    let mine = Holds::<C>::table(store);
    let column = C::parents(mine);
    column.check_parents(Table::ids(parents))?;

    Ok(())
}

#[cfg(test)]
#[path = "entity_test.rs"]
mod entity_test;
