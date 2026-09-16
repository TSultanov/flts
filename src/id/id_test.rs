use super::*;
use crate::test_utils::props;
use proptest::prelude::*;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;

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

fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

proptest! {
    #[test]
    fn an_id_hashes_as_the_number_it_wraps(raw in props::interesting_u64()) {
        prop_assert_eq!(hash_of(&Id::<Chapter>::from_raw(raw)), hash_of(&raw));
    }

    #[test]
    fn an_id_orders_and_compares_by_its_raw_value(
        a in props::interesting_u64(),
        b in props::interesting_u64(),
    ) {
        let left = Id::<Chapter>::from_raw(a);
        let right = Id::<Chapter>::from_raw(b);
        prop_assert_eq!(left.cmp(&right), a.cmp(&b));
        prop_assert_eq!(left == right, a == b);
    }

    #[test]
    fn casting_an_id_to_another_table_keeps_its_raw_value(raw in props::interesting_u64()) {
        let id = Id::<Chapter>::from_raw(raw);
        prop_assert_eq!(id.cast::<Paragraph>().raw(), raw);
        prop_assert_eq!(id.cast::<Paragraph>().cast::<Chapter>(), id);
    }

    #[test]
    fn an_index_survives_the_trip_through_a_raw_value(index: usize) {
        prop_assert_eq!(Id::<Chapter>::from_index(index).index(), index);
    }
}
