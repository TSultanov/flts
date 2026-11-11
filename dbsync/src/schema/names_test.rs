use super::*;

#[test]
fn a_field_is_named_after_the_entity_that_holds_it() {
    assert_eq!(qualified("book", "title"), "book.title");
    assert_eq!(heap_name("book", "title"), "book.title.bytes");
    assert_eq!(presence_name("card", "interval"), "card.interval.present");
    assert_eq!(bucket_name("book", "id"), "book.id.buckets");
    assert_eq!(link_name("book", "id"), "book.id.links");
    assert_eq!(sorted_name("card", "due"), "card.due.sorted");
}

#[test]
fn the_six_names_a_field_can_take_are_all_different() {
    let names = [
        qualified("book", "title"),
        heap_name("book", "title"),
        presence_name("book", "title"),
        bucket_name("book", "title"),
        link_name("book", "title"),
        sorted_name("book", "title"),
    ];

    for (at, name) in names.iter().enumerate() {
        assert!(name.starts_with("book.title"), "{name}");
        assert!(
            !names[..at].contains(name),
            "{name} is claimed by two different columns"
        );
    }
}

#[test]
fn only_the_names_an_index_writes_are_derived() {
    for derived in [
        bucket_name("book", "title"),
        link_name("book", "title"),
        sorted_name("book", "title"),
    ] {
        assert!(is_derived_name(&derived), "{derived}");
    }
    for stored in [
        qualified("book", "title"),
        heap_name("book", "title"),
        presence_name("book", "title"),
        qualified("book", "sorted_by"),
    ] {
        assert!(!is_derived_name(&stored), "{stored}");
    }
}
