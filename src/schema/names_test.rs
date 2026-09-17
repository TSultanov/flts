use super::*;

#[test]
fn a_field_is_named_after_the_entity_that_holds_it() {
    assert_eq!(qualified("book", "title"), "book.title");
    assert_eq!(heap_name("book", "title"), "book.title.bytes");
    assert_eq!(presence_name("card", "interval"), "card.interval.present");
}

#[test]
fn the_three_names_a_field_can_take_are_all_different() {
    let names = [
        qualified("book", "title"),
        heap_name("book", "title"),
        presence_name("book", "title"),
    ];

    for (at, name) in names.iter().enumerate() {
        assert!(name.starts_with("book.title"), "{name}");
        assert!(
            !names[..at].contains(name),
            "{name} is claimed by two different columns"
        );
    }
}
