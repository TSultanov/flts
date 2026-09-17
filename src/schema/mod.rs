//! Typed tables over named columns: the runtime a generated schema wires itself into.

mod child_index;
mod column;
mod diff;
mod entity;
mod enums;
mod names;
mod nullable;
mod reference;
mod store;
mod table;
mod text;

pub use child_index::ChildIndex;
pub use column::{Column, ColumnKind, ColumnSpec, Field, FieldMut, Residency, Row};
pub use diff::SchemaDiff;
pub use entity::{Child, Entity, Holds, children_of};
pub use enums::{EnumColumn, UnitEnum};
pub use names::{heap_name, presence_name, qualified};
pub use nullable::NullableColumn;
pub use reference::RefColumn;
pub use store::{Store, StoreState};
pub use table::{AnyTable, Table};
pub use text::{NullableTextColumn, TextColumn};

#[cfg(all(test, feature = "derive"))]
#[path = "generated_test.rs"]
mod generated_test;

#[cfg(all(test, feature = "derive"))]
#[path = "declared_test.rs"]
mod declared_test;
