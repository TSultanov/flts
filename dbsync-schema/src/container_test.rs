use super::*;

fn expanded(item: TokenStream) -> String {
    expand(TokenStream::new(), item).unwrap().to_string()
}

fn refused(item: TokenStream) -> String {
    match expand(TokenStream::new(), item) {
        Ok(_) => panic!("this container must not have expanded"),
        Err(err) => err.to_string(),
    }
}

#[test]
fn a_container_holds_one_table_per_set_and_the_state_of_its_file() {
    let out = expanded(quote! {
        struct Library { books: Set<Book>, chapters: Set<Chapter> }
    });

    assert!(
        out.contains("dbsync_state : :: dbsync :: schema :: StoreState"),
        "{out}"
    );
    assert!(
        out.contains("books : < Book as :: dbsync :: schema :: Entity > :: Table"),
        "{out}"
    );
    assert!(
        out.contains("chapters : < Chapter as :: dbsync :: schema :: Entity > :: Table"),
        "{out}"
    );
    assert!(out.contains("# [derive (Default , Clone)]"), "{out}");
}

#[test]
fn a_container_lists_its_tables_in_the_order_they_were_declared() {
    let out = expanded(quote! {
        struct Library { books: Set<Book>, chapters: Set<Chapter> }
    });

    assert!(
        out.contains("vec ! [& self . books , & self . chapters ,]"),
        "{out}"
    );
    assert!(
        out.contains("vec ! [& mut self . books , & mut self . chapters ,]"),
        "{out}"
    );
}

#[test]
fn a_container_reaches_each_table_by_the_entity_that_names_it() {
    let out = expanded(quote! {
        struct Library { books: Set<Book> }
    });

    assert!(
        out.contains("impl :: dbsync :: schema :: Holds < Book > for Library"),
        "{out}"
    );
    assert!(out.contains("& self . books"), "{out}");
    assert!(out.contains("& mut self . books"), "{out}");
}

#[test]
fn a_container_can_join_any_parent_to_any_child_it_holds() {
    let out = expanded(quote! {
        struct Library { books: Set<Book>, chapters: Set<Chapter> }
    });

    assert!(out.contains("pub fn children_of < P , C >"), "{out}");
    assert!(
        out.contains("C : :: dbsync :: schema :: Child < P >"),
        "{out}"
    );
    assert!(
        out.contains(":: dbsync :: schema :: children_of (self , parent)"),
        "{out}"
    );
}

#[test]
fn the_field_name_is_the_path_and_the_entity_is_the_column_prefix() {
    let out = expanded(quote! {
        struct Library { shelved: Set<Book> }
    });

    assert!(
        out.contains("shelved : < Book as :: dbsync :: schema :: Entity > :: Table"),
        "a Set may be called anything; only the entity names the columns: {out}"
    );
}

#[test]
fn a_field_that_is_not_a_set_is_refused() {
    let err = refused(quote! {
        struct Library { books: Vec<Book> }
    });

    assert!(err.contains("every field of a container is a Set"), "{err}");
}

#[test]
fn holding_the_same_entity_twice_is_refused_and_names_the_first() {
    let err = refused(quote! {
        struct Library { books: Set<Book>, again: Set<Book> }
    });

    assert!(err.contains("`books` already holds this entity"), "{err}");
    assert!(err.contains("claiming the same columns"), "{err}");
}

#[test]
fn a_container_that_holds_nothing_is_refused() {
    let err = refused(quote! {
        struct Library {}
    });

    assert!(err.contains("at least one Set"), "{err}");
}

#[test]
fn a_generic_container_is_refused() {
    let err = refused(quote! {
        struct Library<T> { books: Set<T> }
    });

    assert!(err.contains("cannot be generic"), "{err}");
}

#[test]
fn a_tuple_container_is_refused() {
    let err = refused(quote! {
        struct Library(Set<Book>);
    });

    assert!(err.contains("needs named fields"), "{err}");
}

#[test]
fn the_attribute_takes_no_arguments() {
    let err = match expand(
        quote! { path = "x" },
        quote! { struct Library { books: Set<Book> } },
    ) {
        Ok(_) => panic!("arguments must be refused"),
        Err(err) => err.to_string(),
    };

    assert!(err.contains("takes no arguments"), "{err}");
}
