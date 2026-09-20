use super::*;
use quote::quote;
use syn::parse::Parser;

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
        indexing: Indexing::None,
        through: None,
        docs: Vec::new(),
    };
    let text = Field {
        name: syn::parse_quote!(title),
        kind: kind(quote!(String)),
        indexing: Indexing::None,
        through: None,
        docs: Vec::new(),
    };

    assert!(!many.is_stored());
    assert!(text.is_stored());
}

#[test]
fn the_column_a_field_asks_for_names_both_ends_of_a_reference() {
    let child: syn::Ident = syn::parse_quote!(Chapter);
    let column = kind(quote!(One<Book>))
        .column(&child, Indexing::None)
        .to_string();

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

fn field_of(field: TokenStream) -> Field {
    let parsed = syn::Field::parse_named.parse2(field).unwrap();
    Field::parse(&parsed).unwrap()
}

fn refused_field(field: TokenStream) -> String {
    let parsed = syn::Field::parse_named.parse2(field).unwrap();
    match Field::parse(&parsed) {
        Ok(_) => panic!("this field must not have parsed"),
        Err(err) => err.to_string(),
    }
}

#[test]
fn a_field_cannot_request_secondary_uniqueness() {
    let err = Field::parse(
        &syn::Field::parse_named
            .parse2(quote!(#[index(unique)] isbn: String))
            .unwrap(),
    )
    .err()
    .unwrap();
    assert!(err.to_string().contains("only entity UUIDs are unique"));
}

#[test]
fn an_attribute_a_field_does_not_take_is_refused_rather_than_dropped() {
    let err = refused_field(quote!(#[indexed] title: String));
    assert!(err.contains("`indexed` is not an attribute"), "{err}");
    assert!(err.contains("#[index]"), "{err}");

    let err = refused_field(quote!(#[serde(rename = "t")] title: String));
    assert!(err.contains("`serde` is not an attribute"), "{err}");
}

#[test]
fn a_field_that_could_be_compiled_out_is_refused_because_its_column_cannot_be() {
    for conditional in [
        quote!(#[cfg(feature = "x")] isbn: String),
        quote!(#[cfg_attr(test, index)] isbn: String),
    ] {
        let err = refused_field(conditional);
        assert!(err.contains("cannot be compiled out"), "{err}");
    }
}

#[test]
fn an_index_asked_for_twice_or_asked_for_wrongly_is_refused() {
    let err = refused_field(quote!(#[index] #[index] title: String));
    assert!(err.contains("this field is indexed already"), "{err}");

    let err = refused_field(quote!(#[index = "yes"] title: String));
    assert!(err.contains("only entity UUIDs are unique"), "{err}");

    for wrong in [
        quote!(#[index(sorted)] title: String),
        quote!(#[index(unique, sorted)] title: String),
    ] {
        let err = refused_field(wrong);
        assert!(err.contains("only entity UUIDs are unique"), "{err}");
    }
}

#[test]
fn a_field_an_index_could_not_answer_for_is_refused_where_it_is_asked() {
    for float in [
        quote!(#[index] score: f64),
        quote!(#[index] score: f32),
        quote!(#[index] score: Option<f64>),
    ] {
        let err = refused_field(float);
        assert!(err.contains("NaN"), "{err}");
    }

    let err = refused_field(quote!(#[index] book: One<Book>));
    assert!(err.contains("a parent is indexed already"), "{err}");

    let err = refused_field(quote!(#[index] chapters: Many<Chapter>));
    assert!(err.contains("nothing to index"), "{err}");
}

#[test]
fn a_doc_comment_on_a_field_is_kept_rather_than_refused_or_dropped() {
    let documented = field_of(quote! {
        /// The title on the spine.
        #[index]
        title: String
    });

    assert_eq!(documented.docs.len(), 1);
    assert_eq!(documented.indexing, Indexing::Shared);
    assert!(field_of(quote!(title: String)).docs.is_empty());
}
