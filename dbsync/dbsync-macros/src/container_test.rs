use super::*;

fn expanded(item: TokenStream) -> String {
    expand(quote! { schema = "library", version = 1 }, item)
        .unwrap()
        .to_string()
}

fn refused(item: TokenStream) -> String {
    match expand(quote! { schema = "library", version = 1 }, item) {
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
    assert!(
        out.contains("impl :: core :: default :: Default for Library"),
        "{out}"
    );
    assert!(
        out.contains("impl :: core :: clone :: Clone for Library"),
        "{out}"
    );
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
fn unknown_arguments_are_rejected() {
    let err = match expand(
        quote! { path = "x" },
        quote! { struct Library { books: Set<Book> } },
    ) {
        Ok(_) => panic!("arguments must be refused"),
        Err(err) => err.to_string(),
    };

    assert!(err.contains("expected unique schema and version"), "{err}");
}

#[test]
fn a_container_mints_a_handle_for_any_entity_it_holds() {
    let out = expanded(quote! {
        struct Library { books: Set<Book> }
    });

    let mints = [
        "pub fn at < E > (& self , id : :: dbsync :: Id < E > ,) ",
        "-> :: dbsync :: Result << E as :: dbsync :: schema :: Entity > :: Row < '_ , Self >>",
    ]
    .concat();

    assert!(out.contains(&mints), "{out}");
    assert!(
        out.contains(":: dbsync :: schema :: Entry :: of (self , id) ?"),
        "the handle is minted once, where a bad id can still be named: {out}"
    );
}

#[test]
fn a_container_walks_every_row_of_any_entity_it_holds() {
    let out = expanded(quote! {
        struct Library { books: Set<Book> }
    });

    assert!(
        out.contains("pub fn all < E > (& self) -> :: dbsync :: schema :: Rows < '_ , Self , E >"),
        "{out}"
    );
    assert!(
        out.contains(":: dbsync :: schema :: Rows :: all (self)"),
        "{out}"
    );
}

#[test]
fn a_container_mints_a_mutable_handle_for_any_entity_it_holds() {
    let out = expanded(quote! {
        struct Library { books: Set<Book> }
    });

    let mints = [
        "pub fn at_mut < E > (& mut self , id : :: dbsync :: Id < E > ,) ",
        "-> :: dbsync :: Result << E as :: dbsync :: schema :: Entity > :: RowMut < '_ , Self >>",
    ]
    .concat();

    assert!(out.contains(&mints), "{out}");
    assert!(
        out.contains(":: dbsync :: schema :: EntryMut :: of (self , id) ?"),
        "{out}"
    );
}

#[test]
fn the_children_a_join_hands_back_are_owned_because_it_gathers_them() {
    let out = expanded(quote! {
        struct Library { books: Set<Book>, chapters: Set<Chapter> }
    });

    assert!(
        out.contains(":: dbsync :: Result < :: std :: vec :: Vec < :: dbsync :: Id < C >> >"),
        "a chain is walked and sorted, so there is no slice to lend: {out}"
    );
}

#[test]
fn a_container_audits_every_entity_it_holds_and_not_only_its_own_columns() {
    let out = expanded(quote! {
        struct Library { books: Set<Book>, chapters: Set<Chapter> }
    });

    assert!(
        out.contains("fn audit (& self) -> :: dbsync :: Result < () >"),
        "{out}"
    );
    assert!(
        out.contains("< Book as :: dbsync :: schema :: Audited < Self >> :: audit_in (self) ?"),
        "{out}"
    );
    assert!(
        out.contains("< Chapter as :: dbsync :: schema :: Audited < Self >> :: audit_in (self) ?"),
        "{out}"
    );
}

#[test]
fn a_store_takes_a_row_out_and_puts_it_back_if_the_audit_refuses() {
    let out = expanded(quote! {
        struct Library { books: Set<Book>, chapters: Set<Chapter> }
    });

    assert!(
        out.contains("pub fn remove < E > (& mut self , of : :: dbsync :: Id < E >)"),
        "{out}"
    );
    assert!(
        out.contains("let kept = < Self as :: dbsync :: schema :: Store > :: snapshot (self)"),
        "a snapshot is an Arc-level clone, so the way back costs nothing: {out}"
    );
    assert!(
        out.contains("* self = kept"),
        "a removal something still names leaves the store as it was: {out}"
    );
    assert!(
        out.contains("cannot be taken out while something still names it"),
        "{out}"
    );
}

#[test]
fn a_store_purges_a_row_with_all_that_needs_it_or_purges_nothing() {
    let out = expanded(quote! {
        struct Library { books: Set<Book>, chapters: Set<Chapter> }
    });

    let purges = out.split("pub fn purge < E >").nth(1).unwrap();
    let purges = purges.split("pub fn is_purged").next().unwrap();
    assert!(purges.contains(":: check_mutable (self) ?"), "{purges}");
    assert!(
        purges.contains(":: purge_cascading (self , E :: NAME , of . uuid ())"),
        "{purges}"
    );
    assert!(
        purges.contains("if result . is_err () { * self = kept ; }"),
        "{purges}"
    );
    assert!(
        out.contains("pub fn is_purged < E > (& self , id : :: dbsync :: Id < E >) -> :: dbsync :: Result < bool >"),
        "{out}"
    );
}

#[test]
fn container_emits_explicit_schema_identity() {
    let out = expand(
        quote! { schema = "catalog", version = 42 },
        quote! { struct Library { books: Set<Book> } },
    )
    .unwrap()
    .to_string();
    assert!(
        out.contains("const SCHEMA_NAME : & 'static str = \"catalog\""),
        "{out}"
    );
    assert!(out.contains("const SCHEMA_VERSION : u64 = 42"), "{out}");
}

#[test]
fn schema_identity_arguments_are_required_unique_and_valid() {
    for attr in [
        quote! {},
        quote! { schema = "library" },
        quote! { version = 1 },
        quote! { schema = "", version = 1 },
        quote! { schema = "  ", version = 1 },
        quote! { schema = "library", version = 0 },
        quote! { schema = "library", version = -1 },
        quote! { schema = "library", version = 18446744073709551616 },
        quote! { schema = "library", version = "1" },
        quote! { schema = "library", schema = "other", version = 1 },
        quote! { schema = "library", version = 1, version = 2 },
    ] {
        assert!(
            expand(attr.clone(), quote! { struct Library { books: Set<Book> } }).is_err(),
            "accepted {attr}"
        );
    }
}
