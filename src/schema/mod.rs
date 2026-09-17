//! Typed tables over named columns: the runtime a generated schema wires itself into.

mod column;
mod enums;
mod names;
mod nullable;
mod text;

pub use column::{Column, ColumnKind, ColumnSpec, Field, FieldMut, Row};
pub use enums::{EnumColumn, UnitEnum};
pub use names::{heap_name, presence_name, qualified};
pub use nullable::NullableColumn;
pub use text::{NullableTextColumn, TextColumn};
