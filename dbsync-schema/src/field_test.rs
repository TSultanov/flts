use super::*;
use quote::quote;

fn kind(ty: TokenStream) -> Kind {
    Kind::parse(&syn::parse2(ty).unwrap()).unwrap()
}

fn refused(ty: TokenStream) -> String {
    match Kind::parse(&syn::parse2(ty).unwrap()) {
        Ok(_) => panic!("this field type must not have parsed"),
        Err(err) => err.to_string(),
    }
}

#[test]
fn a_string_field_becomes_a_text_column() {
    assert!(matches!(kind(quote!(String)), Kind::Text));
    assert!(matches!(kind(quote!(Option<String>)), Kind::NullableText));
}

#[test]
fn an_unsized_str_is_refused_by_name_rather_than_taken_for_an_enum() {
    let err = refused(quote!(str));
    assert!(err.contains("string field is declared String"), "{err}");

    let optional = refused(quote!(Option<str>));
    assert!(
        optional.contains("string field is declared String"),
        "{optional}"
    );
}

#[test]
fn a_borrowed_field_is_refused_because_a_column_outlives_it() {
    let err = refused(quote!(&'a str));
    assert!(err.contains("cannot be a reference"), "{err}");

    let owned_target = refused(quote!(&'a Book));
    assert!(
        owned_target.contains("cannot be a reference"),
        "{owned_target}"
    );
}

#[test]
fn a_reference_field_remembers_the_parent_it_points_at() {
    let Kind::Reference(parent) = kind(quote!(One<Book>)) else {
        panic!("One<Book> is a reference");
    };
    assert_eq!(quote!(#parent).to_string(), "Book");
}

#[test]
fn a_many_field_remembers_the_child_it_groups() {
    let Kind::Many(child) = kind(quote!(Many<Chapter>)) else {
        panic!("Many<Chapter> groups children");
    };
    assert_eq!(quote!(#child).to_string(), "Chapter");
}

#[test]
fn every_scalar_the_crate_can_store_is_recognised_as_one() {
    for scalar in SCALARS {
        let ty: TokenStream = scalar.parse().unwrap();
        assert!(
            matches!(kind(ty.clone()), Kind::Scalar(_)),
            "{scalar} should be a scalar"
        );
        assert!(
            matches!(kind(quote!(Option<#ty>)), Kind::NullableScalar(_)),
            "Option<{scalar}> should be a nullable scalar"
        );
    }
}

#[test]
fn a_type_the_vocabulary_does_not_name_is_taken_for_an_enum() {
    assert!(matches!(kind(quote!(Shelf)), Kind::Enum(_)));
}

#[test]
fn a_many_field_is_not_stored_in_a_column_and_everything_else_is() {
    let many = Field {
        name: syn::parse_quote!(chapters),
        kind: kind(quote!(Many<Chapter>)),
    };
    let text = Field {
        name: syn::parse_quote!(title),
        kind: kind(quote!(String)),
    };

    assert!(!many.is_stored());
    assert!(text.is_stored());
}

#[test]
fn the_column_a_field_asks_for_names_both_ends_of_a_reference() {
    let child: syn::Ident = syn::parse_quote!(Chapter);
    let column = kind(quote!(One<Book>)).column(&child).to_string();

    assert!(column.contains("RefColumn"), "{column}");
    assert!(column.contains("Book"), "{column}");
    assert!(column.contains("Chapter"), "{column}");
}

#[test]
fn an_optional_parent_is_refused_with_a_way_forward() {
    let err = refused(quote!(Option<One<Book>>));
    assert!(
        err.contains("optional parent is not supported yet"),
        "{err}"
    );
}

#[test]
fn the_name_a_parent_field_used_to_have_is_refused_by_name() {
    let err = refused(quote!(Ref<Book>));
    assert!(err.contains("a parent is declared One<..>"), "{err}");
    assert!(err.contains("one-to-many"), "{err}");
}

#[test]
fn the_old_name_is_refused_inside_an_option_too() {
    let err = refused(quote!(Option<Ref<Book>>));
    assert!(err.contains("a parent is declared One<..>"), "{err}");
}

#[test]
fn an_optional_many_is_refused_because_it_is_already_possibly_empty() {
    let err = refused(quote!(Option<Many<Chapter>>));
    assert!(err.contains("already the absence of children"), "{err}");
}

#[test]
fn an_optional_enum_is_refused_with_a_way_forward() {
    let err = refused(quote!(Option<Shelf>));
    assert!(err.contains("optional enum is not supported yet"), "{err}");
}
