use super::*;
use quote::quote;

fn expanded(item: TokenStream) -> String {
    expand(TokenStream::new(), item).unwrap().to_string()
}

fn refused(item: TokenStream) -> String {
    match expand(TokenStream::new(), item) {
        Ok(_) => panic!("this enum must not have expanded"),
        Err(err) => err.to_string(),
    }
}

#[test]
fn variants_are_numbered_from_zero_in_the_order_they_are_written() {
    let out = expanded(quote! {
        enum Shelf { Fiction, Reference, Rare }
    });

    assert!(out.contains("Fiction = 0"), "{out}");
    assert!(out.contains("Reference = 1"), "{out}");
    assert!(out.contains("Rare = 2"), "{out}");
    assert!(out.contains("# [repr (u8)]"), "{out}");
}

#[test]
fn an_explicit_discriminant_sets_the_count_for_the_ones_after_it() {
    let out = expanded(quote! {
        enum Shelf { Fiction = 7, Reference, Rare = 200 }
    });

    assert!(out.contains("Fiction = 7"), "{out}");
    assert!(out.contains("Reference = 8"), "{out}");
    assert!(out.contains("Rare = 200"), "{out}");
}

#[test]
fn the_column_label_is_the_type_name_in_snake_case() {
    let out = expanded(quote! {
        enum ShelfLabel { Only }
    });

    assert!(
        out.contains(r#"NAME : & 'static str = "shelf_label""#),
        "{out}"
    );
    assert!(out.contains(r#""unknown {} {}" , "shelf_label""#), "{out}");
}

#[test]
fn the_derives_the_trait_needs_are_added_and_the_ones_already_there_are_not() {
    let bare = expanded(quote! {
        enum Shelf { Only }
    });
    assert!(bare.contains("# [derive (Clone , Copy)]"), "{bare}");

    let partly = expanded(quote! {
        #[derive(Clone, Debug)]
        enum Shelf { Only }
    });
    assert!(partly.contains("# [derive (Copy)]"), "{partly}");
    assert!(
        !partly.contains("# [derive (Clone , Copy)]"),
        "deriving Clone twice is a conflicting implementation: {partly}"
    );

    let fully = expanded(quote! {
        #[derive(Clone, Copy)]
        enum Shelf { Only }
    });
    assert!(
        fully.contains("# [derive (Clone , Copy)]"),
        "the user's own derive survives: {fully}"
    );
}

#[test]
fn the_visibility_and_attributes_the_author_wrote_survive() {
    let out = expanded(quote! {
        #[derive(Clone, Copy, PartialEq)]
        pub enum Shelf { Only }
    });

    assert!(out.contains("pub enum Shelf"), "{out}");
    assert!(out.contains("PartialEq"), "{out}");
}

#[test]
fn a_variant_that_carries_data_is_refused() {
    let err = refused(quote! {
        enum Shelf { Fiction, Signed(String) }
    });

    assert!(err.contains("cannot carry data"), "{err}");
}

#[test]
fn a_named_variant_that_carries_data_is_refused_too() {
    let err = refused(quote! {
        enum Shelf { Fiction, Signed { by: String } }
    });

    assert!(err.contains("cannot carry data"), "{err}");
}

#[test]
fn a_duplicate_discriminant_is_refused_and_names_the_variant_that_took_it() {
    let err = refused(quote! {
        enum Shelf { Fiction = 1, Reference = 1 }
    });

    assert!(
        err.contains("discriminant 1 is already taken by `Fiction`"),
        "{err}"
    );
}

#[test]
fn a_discriminant_a_byte_cannot_hold_is_refused() {
    let err = refused(quote! {
        enum Shelf { Fiction = 256 }
    });
    assert!(err.contains("under 256"), "{err}");

    let overflowed = refused(quote! {
        enum Shelf { Fiction = 255, Reference }
    });
    assert!(overflowed.contains("under 256"), "{overflowed}");
}

#[test]
fn a_discriminant_that_is_not_a_plain_integer_is_refused() {
    let err = refused(quote! {
        enum Shelf { Fiction = 1 + 1 }
    });

    assert!(err.contains("plain integer literal"), "{err}");
}

#[test]
fn an_enum_with_no_variants_is_refused() {
    let err = refused(quote! {
        enum Shelf {}
    });

    assert!(err.contains("at least one variant"), "{err}");
}

#[test]
fn a_generic_enum_is_refused() {
    let err = refused(quote! {
        enum Shelf<T> { Only(T) }
    });

    assert!(err.contains("cannot be generic"), "{err}");
}

#[test]
fn an_enum_that_already_names_its_repr_is_refused() {
    let err = refused(quote! {
        #[repr(u16)]
        enum Shelf { Only }
    });

    assert!(err.contains("writes the repr itself"), "{err}");
}

#[test]
fn the_attribute_takes_no_arguments() {
    let err = match expand(quote! { prefix = "s" }, quote! { enum Shelf { Only } }) {
        Ok(_) => panic!("arguments must be refused"),
        Err(err) => err.to_string(),
    };

    assert!(err.contains("takes no arguments"), "{err}");
}
