//! What every entity knows about itself: its name, its columns, and whose child it is.

use super::reference::RefColumn;
use super::table::Table;

pub trait Entity: 'static {
    const NAME: &'static str;

    type Table: Table<Of = Self>;
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
