use super::*;
use crate::test_utils::props;
use proptest::prelude::*;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;

struct Chapter;
struct Paragraph;

#[test]
fn ids_of_different_tables_are_different_types() {
    let uuid = Uuid::now_v7();
    let chapter = Id::<Chapter>::from_uuid(uuid);
    let paragraph = Id::<Paragraph>::from_uuid(uuid);

    assert_eq!(chapter.uuid(), paragraph.uuid());
    assert_eq!(chapter, chapter.cast::<Paragraph>().cast::<Chapter>());
}

#[test]
fn every_minted_id_is_one_no_other_row_was_given() {
    let minted: Vec<Id<Chapter>> = (0..256).map(|_| Id::mint()).collect();
    let distinct: std::collections::HashSet<Id<Chapter>> = minted.iter().copied().collect();

    assert_eq!(distinct.len(), minted.len());
    assert!(minted.iter().all(|id| !id.is_nil()));
}

#[test]
fn ids_are_usable_as_map_keys_and_sort_the_way_they_were_minted() {
    let first = Id::<Chapter>::mint();
    let second = Id::<Chapter>::mint();

    let mut counts: HashMap<Id<Chapter>, u32> = HashMap::new();
    *counts.entry(first).or_default() += 1;
    *counts.entry(first).or_default() += 1;
    assert_eq!(counts[&first], 2);

    let mut ids = vec![second, first];
    ids.sort();
    assert_eq!(ids, vec![first, second]);
}

#[test]
fn the_nil_id_is_the_one_no_row_can_have() {
    assert!(Id::<Chapter>::NIL.is_nil());
    assert_eq!(Id::<Chapter>::NIL.uuid(), Uuid::NIL);
    assert!(!Id::<Chapter>::mint().is_nil());
}

#[test]
fn debug_names_the_table_and_display_names_only_the_id() {
    let id = Id::<Paragraph>::from_uuid(Uuid::from_u128(0x0199_c4f2_8a1c_7f3b_9d0e_5a6c_1b2d_3e4f));

    assert_eq!(
        format!("{id:?}"),
        "Id<Paragraph>(0199c4f2-8a1c-7f3b-9d0e-5a6c1b2d3e4f)"
    );
    assert_eq!(format!("{id}"), "0199c4f2-8a1c-7f3b-9d0e-5a6c1b2d3e4f");
}

#[test]
fn a_text_that_is_not_a_uuid_is_not_an_id_either() {
    let err = "not an id".parse::<Id<Chapter>>().unwrap_err();
    assert!(err.to_string().contains("a uuid is 36 characters"), "{err}");
}

fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

proptest! {
    #[test]
    fn an_id_hashes_as_the_uuid_it_wraps(uuid in props::uuid()) {
        prop_assert_eq!(hash_of(&Id::<Chapter>::from_uuid(uuid)), hash_of(&uuid));
    }

    #[test]
    fn an_id_orders_and_compares_by_the_uuid_it_wraps(a in props::uuid(), b in props::uuid()) {
        let left = Id::<Chapter>::from_uuid(a);
        let right = Id::<Chapter>::from_uuid(b);

        prop_assert_eq!(left.cmp(&right), a.cmp(&b));
        prop_assert_eq!(left == right, a == b);
    }

    #[test]
    fn casting_an_id_to_another_table_keeps_the_uuid_it_names(uuid in props::uuid()) {
        let id = Id::<Chapter>::from_uuid(uuid);

        prop_assert_eq!(id.cast::<Paragraph>().uuid(), uuid);
        prop_assert_eq!(id.cast::<Paragraph>().cast::<Chapter>(), id);
    }

    #[test]
    fn an_id_finds_its_way_back_from_its_own_text(uuid in props::uuid()) {
        let id = Id::<Chapter>::from_uuid(uuid);

        prop_assert_eq!(id.to_string().parse::<Id<Chapter>>().unwrap(), id);
    }
}
