use super::*;

fn expanded(item: TokenStream) -> String {
    expand(TokenStream::new(), item).unwrap().to_string()
}

fn refused(item: TokenStream) -> String {
    match expand(TokenStream::new(), item) {
        Ok(_) => panic!("this entity must not have expanded"),
        Err(err) => err.to_string(),
    }
}

#[test]
fn an_entity_becomes_a_tag_type_and_a_table_of_columns() {
    let out = expanded(quote! {
        struct Book { title: String, author: String }
    });

    assert!(out.contains("struct Book ;"), "{out}");
    assert!(out.contains("struct BookTable"), "{out}");
    assert!(out.contains(r#"NAME : & 'static str = "book""#), "{out}");
    assert!(out.contains("type Table = BookTable"), "{out}");
    assert!(out.contains("# [derive (Default , Clone)]"), "{out}");
}

#[test]
fn a_many_field_asks_for_no_column_at_all() {
    let out = expanded(quote! {
        struct Book { title: String, chapters: Many<Chapter> }
    });

    assert!(
        out.contains("title : :: dbsync :: schema :: TextColumn"),
        "{out}"
    );
    assert!(
        !out.contains("chapters :"),
        "a Many field is derived, not stored: {out}"
    );
    assert!(
        !out.contains("fn chapters"),
        "the accessor belongs to the container, which can see both tables: {out}"
    );
}

#[test]
fn a_many_field_makes_the_compiler_check_the_child_points_back() {
    let out = expanded(quote! {
        struct Book { title: String, chapters: Many<Chapter> }
    });

    assert!(
        out.contains("groups_by_its_parent :: < Book , Chapter >"),
        "{out}"
    );
}

#[test]
fn a_reference_field_says_whose_child_this_entity_is() {
    let out = expanded(quote! {
        struct Chapter { book: Ref<Book>, title: String }
    });

    assert!(
        out.contains("impl :: dbsync :: schema :: Child < Book > for Chapter"),
        "{out}"
    );
    assert!(out.contains("& table . book"), "{out}");
}

#[test]
fn push_takes_one_argument_per_stored_field_in_the_order_they_were_written() {
    let out = expanded(quote! {
        struct Chapter { book: Ref<Book>, title: String, chapters: Many<Paragraph> }
    });

    assert!(
        out.contains("fn push (& mut self , book : :: dbsync :: Id < Book > , title : & str ,)"),
        "{out}"
    );
    assert!(
        out.contains("reserve (self) ?"),
        "a push touches every column or none: {out}"
    );
    assert!(
        out.contains("let row = < Self as :: dbsync :: schema :: Table > :: rows (self) ;"),
        "the new row's index is the count before the push, not one column's return value: {out}"
    );
}

#[test]
fn every_stored_field_gets_a_reader_and_a_writer() {
    let out = expanded(quote! {
        struct Book { title: String, year: u32, note: Option<String>, rank: Option<u64> }
    });

    for accessor in [
        "fn title (& self , of : :: dbsync :: Id < Book >) -> :: dbsync :: Result < :: std :: borrow :: Cow",
        "fn set_title (& mut self , of : :: dbsync :: Id < Book > , value : & str ,) -> :: dbsync :: Result < () >",
        "fn year (& self , of : :: dbsync :: Id < Book >) -> :: dbsync :: Result < u32 >",
        "fn set_year (& mut self , of : :: dbsync :: Id < Book > , value : u32 ,) -> :: dbsync :: Result < () >",
        "fn note (& self , of : :: dbsync :: Id < Book >) -> :: dbsync :: Result < :: core :: option :: Option",
        "fn rank (& self , of : :: dbsync :: Id < Book >) -> :: dbsync :: Result < :: core :: option :: Option < u64",
        "fn set_rank (& mut self , of : :: dbsync :: Id < Book > , value : :: core :: option :: Option < u64",
    ] {
        assert!(out.contains(accessor), "{accessor} missing from {out}");
    }
}

#[test]
fn a_scalar_reads_through_its_row_index_and_everything_else_through_the_row() {
    let out = expanded(quote! {
        struct Book { title: String, year: u32 }
    });

    assert!(
        out.contains("self . year . row (row . index ()) . copied ()"),
        "{out}"
    );
    assert!(
        out.contains("self . year . set (row . index () , value)"),
        "an arena is addressed by index; only a column knows what a Row means: {out}"
    );
    assert!(out.contains("self . title . get (row)"), "{out}");
    assert!(out.contains("self . title . set (row , value)"), "{out}");
}

#[test]
fn each_kind_of_field_asks_for_the_column_that_holds_it() {
    let out = expanded(quote! {
        struct Card { lemma: String, note: Option<String>, interval: Option<u64>, shelf: Shelf, book: Ref<Book> }
    });

    assert!(
        out.contains("lemma : :: dbsync :: schema :: TextColumn"),
        "{out}"
    );
    assert!(
        out.contains("note : :: dbsync :: schema :: NullableTextColumn"),
        "{out}"
    );
    assert!(
        out.contains("interval : :: dbsync :: schema :: NullableColumn < u64 >"),
        "{out}"
    );
    assert!(
        out.contains("shelf : :: dbsync :: schema :: EnumColumn < Shelf >"),
        "{out}"
    );
    assert!(
        out.contains("book : :: dbsync :: schema :: RefColumn < Book , Card >"),
        "{out}"
    );
}

#[test]
fn a_multi_word_entity_is_named_in_snake_case() {
    let out = expanded(quote! {
        struct ReadingProgress { at: u64 }
    });

    assert!(
        out.contains(r#"NAME : & 'static str = "reading_progress""#),
        "{out}"
    );
    assert!(out.contains("struct ReadingProgressTable"), "{out}");
}

#[test]
fn two_references_to_the_same_parent_are_refused_and_both_are_named() {
    let err = refused(quote! {
        struct Chapter { book: Ref<Book>, reprint: Ref<Book>, title: String }
    });

    assert!(err.contains("`book` and `reprint`"), "{err}");
    assert!(err.contains("cannot tell which one groups it"), "{err}");
}

#[test]
fn two_references_to_different_parents_are_fine() {
    let out = expanded(quote! {
        struct Note { book: Ref<Book>, chapter: Ref<Chapter> }
    });

    assert!(out.contains("Child < Book > for Note"), "{out}");
    assert!(out.contains("Child < Chapter > for Note"), "{out}");
}

#[test]
fn a_generic_entity_is_refused() {
    let err = refused(quote! {
        struct Book<T> { title: T }
    });

    assert!(err.contains("cannot be generic"), "{err}");
}

#[test]
fn a_tuple_entity_is_refused() {
    let err = refused(quote! {
        struct Book(String);
    });

    assert!(err.contains("needs named fields"), "{err}");
}

#[test]
fn an_entity_that_would_claim_the_reserved_prefix_is_refused() {
    let err = refused(quote! {
        struct Dbsync { title: String }
    });

    assert!(err.contains("reserved"), "{err}");
}

#[test]
fn the_attribute_takes_no_arguments() {
    let err = match expand(
        quote! { prefix = "b" },
        quote! { struct Book { title: String } },
    ) {
        Ok(_) => panic!("arguments must be refused"),
        Err(err) => err.to_string(),
    };

    assert!(err.contains("takes no arguments"), "{err}");
}

#[test]
fn an_entity_whose_every_field_is_derived_is_refused() {
    let err = refused(quote! {
        struct Claimant { orphans: Many<Orphan> }
    });

    assert!(err.contains("at least one stored field"), "{err}");
    assert!(err.contains("has no rows"), "{err}");
}
#[test]
fn an_entity_gets_a_row_handle_that_borrows_the_store_its_table_lives_in() {
    let out = expanded(quote! {
        struct Book { title: String }
    });

    let declared = [
        "struct BookRow < 'a , S : :: dbsync :: schema :: Holds < Book >> ",
        "(:: dbsync :: schema :: Entry < 'a , S , Book > ,) ;",
    ]
    .concat();
    let named = [
        "type Row < 'a , S > = BookRow < 'a , S > ",
        "where S : :: dbsync :: schema :: Holds < Book > + 'a ;",
    ]
    .concat();
    let minted = [
        ":: core :: convert :: From < :: dbsync :: schema :: Entry < 'a , S , Book >> ",
        "for BookRow < 'a , S >",
    ]
    .concat();

    assert!(out.contains(&declared), "{out}");
    assert!(
        out.contains(&named),
        "the runtime reaches a handle through the entity, so the entity names it: {out}"
    );
    assert!(out.contains(&minted), "{out}");
    assert!(
        out.contains(":: core :: ops :: Deref for BookRow < 'a , S >"),
        "{out}"
    );
}

#[test]
fn a_row_handle_reads_each_field_out_of_the_table_the_store_holds() {
    let out = expanded(quote! {
        struct Book { title: String, year: u32 }
    });

    let opened = "let table = self . 0 . table () ; let row = self . 0 . row () ; ";
    let text = [
        "pub fn title (& self) -> :: dbsync :: Result < :: std :: borrow :: Cow < 'a , str > > { ",
        opened,
        "table . title . get (row) }",
    ]
    .concat();
    let scalar = [
        "pub fn year (& self) -> :: dbsync :: Result < u32 > { ",
        opened,
        "table . year . row (row . index ()) . copied () }",
    ]
    .concat();

    assert!(out.contains(&text), "{out}");
    assert!(
        out.contains(&scalar),
        "an arena is addressed by index; only a column knows what a Row means: {out}"
    );
}

#[test]
fn a_handle_reads_for_as_long_as_the_store_lives_and_a_table_only_for_the_call() {
    let out = expanded(quote! {
        struct Book { title: String }
    });

    assert!(
        out.contains("-> :: dbsync :: Result < :: std :: borrow :: Cow < 'a , str > >"),
        "a handle holds the store, so what it reads outlives the call: {out}"
    );
    assert!(
        out.contains("-> :: dbsync :: Result < :: std :: borrow :: Cow < '_ , str > >"),
        "a table is borrowed only for the call that reads it: {out}"
    );
}
