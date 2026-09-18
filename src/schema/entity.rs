//! What every entity knows about itself: its name, its columns, and whose child it is.

use super::column::Row;
use super::entry::{Entry, EntryMut};
use super::reference::RefColumn;
use super::table::Table;
use crate::error::{Error, Result};
use crate::id::Id;

pub trait Entity: Sized + 'static {
    const NAME: &'static str;

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
    Table::live_among(table, C::parents(table).child_rows_of(parent)?)
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

    let links = Table::live_among(joins, <J as Child<A>>::parents(joins).child_rows_of(from)?)?;
    for link in links {
        let beyond = <J as Child<B>>::parents(joins).get(link)?;
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

    let rows = Table::rows(mine);
    for index in 0..rows {
        let row = Row::of(index, rows, C::NAME)?;
        let parent = column.get(row)?;
        if parent.is_nil() || !Table::versions(mine).at(row)?.is_live() {
            continue;
        }
        if Table::live_row_of(parents, parent)?.is_none() {
            return Err(Error::corrupt(
                "reference",
                format!(
                    "{} row {index} names parent {parent}, which was taken out of {}",
                    column.name(),
                    P::NAME
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "entity_test.rs"]
mod entity_test;
