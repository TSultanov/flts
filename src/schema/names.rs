//! How an entity and a field become the names of the file columns that hold them.

pub fn qualified(entity: &str, field: &str) -> String {
    format!("{entity}.{field}")
}

pub fn heap_name(entity: &str, field: &str) -> String {
    format!("{entity}.{field}.bytes")
}

pub fn presence_name(entity: &str, field: &str) -> String {
    format!("{entity}.{field}.present")
}

pub fn bucket_name(entity: &str, field: &str) -> String {
    format!("{entity}.{field}.buckets")
}

pub fn link_name(entity: &str, field: &str) -> String {
    format!("{entity}.{field}.links")
}

#[cfg(test)]
#[path = "names_test.rs"]
mod names_test;
