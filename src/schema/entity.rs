//! What every entity knows about itself: its name, its columns, and whose child it is.

use super::entry::Entry;
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
}

#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a child of `{P}`",
    label = "`{Self}` has no `Ref<{P}>` field",
    note = "`Many<{Self}>` groups `{Self}` rows by the column that points back at `{P}`",
    note = "give `{Self}` exactly one `Ref<{P}>` field, or drop the `Many<{Self}>` from `{P}`"
)]
pub trait Child<P: Entity>: Entity + Sized {
    fn parents(table: &Self::Table) -> &RefColumn<P, Self>;
}

pub trait Holds<E: Entity> {
    fn table(&self) -> &E::Table;

    fn table_mut(&mut self) -> &mut E::Table;
}

pub fn children_of<S, P, C>(store: &S, parent: Id<P>) -> Result<&[Id<C>]>
where
    S: Holds<P> + Holds<C>,
    P: Entity,
    C: Child<P>,
{
    let parents = Table::rows(Holds::<P>::table(store));
    C::parents(Holds::<C>::table(store)).children_of(parents, parent)
}

#[cfg(test)]
#[path = "entity_test.rs"]
mod entity_test;
