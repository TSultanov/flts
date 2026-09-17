//! Typed tables over named columns: the runtime a generated schema wires itself into.

mod child_index;
mod column;
mod entity;
mod enums;
mod names;
mod nullable;
mod reference;
mod table;
mod text;

pub use child_index::ChildIndex;
pub use column::{Column, ColumnKind, ColumnSpec, Field, FieldMut, Row};
pub use entity::Entity;
pub use enums::{EnumColumn, UnitEnum};
pub use names::{heap_name, presence_name, qualified};
pub use nullable::NullableColumn;
pub use reference::RefColumn;
pub use table::{AnyTable, Table};
pub use text::{NullableTextColumn, TextColumn};
