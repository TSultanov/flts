//! Typed tables over named columns: the runtime a generated schema wires itself into.

mod clock;
mod column;
mod diff;
mod entity;
mod entry;
mod enums;
mod identity;
mod index;
mod indexed;
mod names;
mod nullable;
mod pair;
mod reference;
mod store;
mod table;
mod text;
mod version;

pub use clock::VersionVector;
pub use column::{Column, ColumnKind, ColumnSpec, Field, FieldMut, Residency, Row};
pub use diff::SchemaDiff;
pub use entity::{
    Audited, Child, Entity, Holds, Removed, across_rows_of, audit_reference, child_rows_of,
    children_of,
};
pub use entry::{Across, Children, Entry, EntryMut, Rows};
pub use enums::{EnumColumn, UnitEnum};
pub use identity::IdColumn;
pub use index::{HashIndex, IndexKey, Keys};
pub use indexed::{Indexed, Keyed, Unique};
pub use names::{bucket_name, heap_name, link_name, presence_name, qualified};
pub use nullable::{Maybe, NullableColumn};
pub use pair::{Distinct, Pair, PairColumn, check_pairs};
pub use reference::RefColumn;
pub use store::{CLOCK, Store, StoreState};
pub use table::{AnyTable, Table};
pub use text::{NullableTextColumn, TextColumn, TextKey};
pub use version::{Dot, Version, VersionColumn};

#[cfg(all(test, feature = "derive"))]
#[path = "generated_test.rs"]
mod generated_test;

#[cfg(all(test, feature = "derive"))]
#[path = "declared_test.rs"]
mod declared_test;
