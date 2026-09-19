//! Helpers shared by test modules.

pub mod column_file_bytes;
mod committed;
mod flip_byte;
pub mod library;
pub mod props;
mod temp_dir;

pub use committed::committed;
pub use flip_byte::flip_byte;
pub use temp_dir::TempDir;

pub fn is_merge_metadata<S: crate::schema::Store>(store: &S, name: &str) -> bool {
    use crate::schema::ColumnKind;
    name == crate::schema::SCHEMA
        || name.ends_with(".id.buckets")
        || name.ends_with(".id.links")
        || store.specs().iter().any(|column| {
            column.name == name
                && matches!(
                    column.kind,
                    ColumnKind::Identity
                        | ColumnKind::Version
                        | ColumnKind::Clock
                        | ColumnKind::Ref { .. }
                )
        })
}
