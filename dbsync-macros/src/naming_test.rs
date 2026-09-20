use super::*;

#[test]
fn a_single_word_type_becomes_one_lowercase_word() {
    assert_eq!(snake_case("Book"), "book");
    assert_eq!(snake_case("B"), "b");
}

#[test]
fn a_multi_word_type_name_becomes_snake_case() {
    assert_eq!(snake_case("ShelfLabel"), "shelf_label");
    assert_eq!(snake_case("ReadingProgressEntry"), "reading_progress_entry");
}

#[test]
fn an_acronym_does_not_split_into_one_word_per_letter() {
    assert_eq!(snake_case("HTTPRequest"), "http_request");
    assert_eq!(snake_case("ISBN"), "isbn");
}

#[test]
fn a_digit_ends_a_word_like_a_lowercase_letter_does() {
    assert_eq!(snake_case("Book2"), "book2");
    assert_eq!(snake_case("Book2Chapter"), "book2_chapter");
}

#[test]
fn a_name_that_is_already_snake_case_is_left_alone() {
    assert_eq!(snake_case("book_title"), "book_title");
}

#[test]
fn a_word_ending_before_a_trailing_acronym_still_splits() {
    assert_eq!(snake_case("BookID"), "book_id");
    assert_eq!(snake_case("ParagraphHTML"), "paragraph_html");
}

#[test]
fn a_digit_ending_a_word_before_an_acronym_splits_it_too() {
    assert_eq!(snake_case("Book2ID"), "book2_id");
}
