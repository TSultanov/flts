//! Typed tables over named columns: the runtime a generated schema wires itself into.

mod bytes;
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
mod optional;
mod peers;
mod projection;
mod reference;
mod resolved;
mod shard;
mod sorted;
mod store;
mod table;
mod text;
mod version;

pub use bytes::{BytesColumn, NullableBytesColumn};
pub use clock::VersionVector;
pub use column::{Column, ColumnKind, ColumnSpec, Field, FieldMut, Residency, Row};
pub use diff::SchemaDiff;
pub use entity::{
    Audited, Child, Entity, Holds, Removed, Retention, across_rows_of, child_rows_of, children_of,
};
pub use entry::{Across, Children, Entry, EntryMut, Rows};
pub use enums::{EnumColumn, UnitEnum, enum_type};
pub use identity::IdColumn;
pub use index::{HashIndex, IndexKey, Keys};
pub use indexed::{Indexed, Keyed};
pub use merge::{
    Combined, Concurrent, Conflict, MergeReport, OwnShards, PruneReport, PurgeReport,
    ReferenceConflict, RevisionRecord, RowStatus, VisibilityChange,
};
pub use metadata::SchemaMetadata;
pub use migration::{Migration, MigrationReport, MigrationStepReport, MigrationTable, Migrations};
pub use names::{
    bucket_name, heap_name, is_derived_name, link_name, presence_name, qualified, sorted_name,
};
pub use nullable::{Maybe, NullableColumn};
pub use optional::{OptionalEnumColumn, OptionalRefColumn};
pub use peers::Peer;
pub use projection::{Candidate, Reference, View, encode_bytes, encode_record, encode_text};
pub use reference::RefColumn;
pub use resolved::{MergeCandidate, MergeInput, Resolved, RevisionAncestry};
pub use shard::{ShardFile, ShardRow};
pub use sorted::{Prefix, SortEntry, SortKey, Sorted, SortedRun, key_bounds};
pub use store::{Batch, CLOCK, Layout, SCHEMA, SHARDS, ShardMark, Store, StoreState};
pub use table::{Adoption, AnyTable, Table};
pub use text::{NullableTextColumn, TextColumn, TextKey};
pub use version::{Dot, Parent, Version, VersionColumn};

#[cfg(all(test, feature = "macros"))]
#[path = "generated_test.rs"]
mod generated_test;

#[cfg(all(test, feature = "macros"))]
#[path = "declared_test.rs"]
mod declared_test;

#[cfg(all(test, feature = "macros"))]
#[path = "kinds_test.rs"]
mod kinds_test;

#[cfg(all(test, feature = "macros"))]
#[path = "sync_test.rs"]
mod sync_test;

#[cfg(all(test, feature = "macros"))]
#[path = "evolve_test.rs"]
mod evolve_test;

#[cfg(all(test, feature = "macros"))]
#[path = "prune_test.rs"]
mod prune_test;

#[cfg(all(test, feature = "macros"))]
#[path = "purge_test.rs"]
mod purge_test;

#[cfg(all(test, feature = "macros"))]
#[path = "ordered_test.rs"]
mod ordered_test;
