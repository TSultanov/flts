use super::*;
use std::collections::HashMap;

struct Chapter;
struct Paragraph;

#[test]
fn ids_of_different_tables_are_different_types() {
    let chapter = Id::<Chapter>::from_index(3);
    let paragraph = Id::<Paragraph>::from_index(3);
    assert_eq!(chapter.raw(), paragraph.raw());
    assert_eq!(chapter, chapter.cast::<Paragraph>().cast::<Chapter>());
}

#[test]
fn ids_are_usable_as_map_keys_and_sort_by_raw() {
    let mut counts: HashMap<Id<Chapter>, u32> = HashMap::new();
    *counts.entry(Id::from_index(7)).or_default() += 1;
    *counts.entry(Id::from_index(7)).or_default() += 1;
    assert_eq!(counts[&Id::<Chapter>::from_index(7)], 2);

    let mut ids: Vec<Id<Chapter>> = vec![Id::from_index(2), Id::from_index(0)];
    ids.sort();
    assert_eq!(ids, vec![Id::from_index(0), Id::from_index(2)]);
}

#[test]
fn debug_names_the_table() {
    assert_eq!(
        format!("{:?}", Id::<Paragraph>::from_index(12)),
        "Id<Paragraph>(12)"
    );
}
