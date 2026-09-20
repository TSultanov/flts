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
mod merge;
mod metadata;
mod migration;
mod names;
mod nullable;
mod projection;
mod reference;
mod resolved;
mod store;
mod table;
mod text;
mod version;

pub use clock::VersionVector;
pub use column::{Column, ColumnKind, ColumnSpec, Field, FieldMut, Residency, Row};
pub use diff::SchemaDiff;
pub use entity::{
    Audited, Child, Entity, Holds, Removed, Retention, across_rows_of, audit_reference,
    child_rows_of, children_of,
};
pub use entry::{Across, Children, Entry, EntryMut, Rows};
pub use enums::{EnumColumn, UnitEnum};
pub use identity::IdColumn;
pub use index::{HashIndex, IndexKey, Keys};
pub use indexed::{Indexed, Keyed};
pub use merge::{
    Combined, Concurrent, Conflict, MergeReport, ReferenceConflict, RevisionRecord, RowStatus,
    VisibilityChange,
};
pub use metadata::SchemaMetadata;
pub use migration::{Migration, MigrationReport, MigrationStepReport, MigrationTable, Migrations};
pub use names::{bucket_name, heap_name, link_name, presence_name, qualified};
pub use nullable::{Maybe, NullableColumn};
pub use projection::{Candidate, Reference, View, encode_record, encode_text};
pub use reference::RefColumn;
pub use resolved::{MergeCandidate, MergeInput, Resolved, RevisionAncestry};
pub use store::{CLOCK, Recovery, SCHEMA, Store, StoreState};
pub use table::{AnyTable, Table};
pub use text::{NullableTextColumn, TextColumn, TextKey};
pub use version::{Dot, Parent, Version, VersionColumn};

#[cfg(all(test, feature = "macros"))]
#[path = "generated_test.rs"]
mod generated_test;

#[cfg(all(test, feature = "macros"))]
#[path = "declared_test.rs"]
mod declared_test;

#[cfg(all(test, feature = "macros"))]
mod recovery_test;
