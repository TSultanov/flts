//! What every entity knows about itself: its name, its columns, and whose child it is.

use super::column::Row;
use super::entry::{Entry, EntryMut};
use super::reference::RefColumn;
use super::table::Table;
use crate::error::Result;
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

pub fn children_of<S, P, C>(store: &S, parent: Id<P>) -> Result<Vec<Id<C>>>
where
    S: Holds<P> + Holds<C>,
    P: Entity,
    C: Child<P>,
{
    let mine = Table::ids(Holds::<C>::table(store));
    C::parents(Holds::<C>::table(store)).children_of(mine, parent)
}

pub fn child_rows_of<S, P, C>(store: &S, parent: Id<P>) -> Result<Vec<Row>>
where
    S: Holds<P> + Holds<C>,
    P: Entity,
    C: Child<P>,
{
    C::parents(Holds::<C>::table(store)).child_rows_of(parent)
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

    for link in <J as Child<A>>::parents(joins).child_rows_of(from)? {
        let beyond = <J as Child<B>>::parents(joins).get(link)?;
        if beyond.is_nil() {
            continue;
        }
        reached.push(Table::row_of(far, beyond)?);
    }
    Ok(reached)
}

pub fn audit_reference<S, P, C>(store: &S) -> Result<()>
where
    S: Holds<P> + Holds<C>,
    P: Entity,
    C: Child<P>,
{
    let parents = Table::ids(Holds::<P>::table(store));
    C::parents(Holds::<C>::table(store)).check_parents(parents)
}

#[cfg(test)]
#[path = "entity_test.rs"]
mod entity_test;
