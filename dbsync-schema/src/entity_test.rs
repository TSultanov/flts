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
    assert!(out.contains("# [derive (Clone)]"), "{out}");
    assert!(
        out.contains("impl :: core :: default :: Default for BookTable"),
        "a table builds its own columns, because an indexed one must be told its name: {out}"
    );
    assert!(
        out.contains("title : :: core :: default :: Default :: default ()"),
        "{out}"
    );
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
        !out.contains("chapters : ::"),
        "a Many field is derived, not stored: {out}"
    );
    assert!(
        !out.contains("chapters : & str"),
        "a Many field is never pushed: {out}"
    );
}

#[test]
fn a_many_field_hands_a_row_the_children_that_point_back_at_it() {
    let out = expanded(quote! {
        struct Book { title: String, chapters: Many<Chapter> }
    });

    let walks = [
        "pub fn chapters (& self ,) ",
        "-> :: dbsync :: Result < :: dbsync :: schema :: Children < 'a , S , Chapter >> ",
        "where S : :: dbsync :: schema :: Holds < Chapter > , ",
        "{ self . 0 . children :: < Chapter > () }",
    ]
    .concat();

    assert!(
        out.contains(&walks),
        "a store that holds only books cannot walk to a chapter, so the bound is per accessor:          {out}"
    );
}

#[test]
fn a_reference_field_hands_a_row_the_parent_it_points_at_not_the_id() {
    let out = expanded(quote! {
        struct Chapter { book: One<Book>, title: String }
    });

    let follows = [
        "pub fn book (& self ,) ",
        "-> :: dbsync :: Result << Book as :: dbsync :: schema :: Entity > :: Row < 'a , S >> ",
        "where S : :: dbsync :: schema :: Holds < Book > , ",
        "{ let table = self . 0 . table () ; let row = self . 0 . row () ; ",
        "self . 0 . at (table . book . get (row) ?) }",
    ]
    .concat();

    assert!(
        out.contains(&follows),
        "the parent may be declared anywhere, so it is named through the entity: {out}"
    );
    assert!(
        out.contains("pub fn book (& self , of : :: dbsync :: Id < Chapter >)"),
        "the raw id stays reachable through the table: {out}"
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
        struct Chapter { book: One<Book>, title: String }
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
        struct Chapter { book: One<Book>, title: String, chapters: Many<Paragraph> }
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
        out.contains("let id = < Self as :: dbsync :: schema :: Table > :: mint (self) ? ;"),
        "the new row is named by an id minted for it, not by where it landed: {out}"
    );
    assert!(
        out.contains(":: core :: result :: Result :: Ok (id)"),
        "push hands back the id it minted: {out}"
    );
}

#[test]
fn every_table_carries_the_identity_column_before_the_fields_that_were_declared() {
    let out = expanded(quote! {
        struct Chapter { book: One<Book>, title: String }
    });

    assert!(
        out.contains("dbsync_ids : :: dbsync :: schema :: IdColumn < Chapter >"),
        "{out}"
    );
    assert!(
        out.contains(
            "vec ! [:: dbsync :: schema :: Field :: new (\"id\" , & self . dbsync_ids) , \
             :: dbsync :: schema :: Field :: new (\"book\""
        ),
        "the identity column is the first field, so it is what counts the rows: {out}"
    );
    assert!(
        out.contains("fn ids (& self) -> & :: dbsync :: schema :: IdColumn < Chapter >"),
        "{out}"
    );
}

#[test]
fn a_field_called_id_is_refused_because_every_entity_already_has_one() {
    let err = refused(quote! {
        struct Chapter { id: String, title: String }
    });

    assert!(
        err.contains("`id` is the identity every entity already has"),
        "{err}"
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
        struct Card { lemma: String, note: Option<String>, interval: Option<u64>, shelf: Shelf, book: One<Book> }
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
        struct Chapter { book: One<Book>, reprint: One<Book>, title: String }
    });

    assert!(err.contains("`book` and `reprint`"), "{err}");
    assert!(err.contains("cannot tell which one groups it"), "{err}");
}

#[test]
fn two_references_to_different_parents_are_fine() {
    let out = expanded(quote! {
        struct Note { book: One<Book>, chapter: One<Chapter> }
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

#[test]
fn an_entity_gets_a_mutable_handle_that_only_writes() {
    let out = expanded(quote! {
        struct Book { title: String, chapters: Many<Chapter> }
    });

    let declared = [
        "struct BookRowMut < 'a , S : :: dbsync :: schema :: Holds < Book >> ",
        "(:: dbsync :: schema :: EntryMut < 'a , S , Book > ,) ;",
    ]
    .concat();
    let named = [
        "type RowMut < 'a , S > = BookRowMut < 'a , S > ",
        "where S : :: dbsync :: schema :: Holds < Book > + 'a ;",
    ]
    .concat();

    assert!(out.contains(&declared), "{out}");
    assert!(out.contains(&named), "{out}");
    assert!(
        out.contains(":: core :: ops :: DerefMut for BookRowMut < 'a , S >"),
        "{out}"
    );
    assert!(
        !out.contains("fn set_chapters"),
        "a Many is grouped from the child, so it is written by moving the child: {out}"
    );
}

#[test]
fn a_mutable_handle_writes_through_the_table_the_store_lends_it() {
    let out = expanded(quote! {
        struct Book { title: String, year: u32 }
    });

    let opened = "{ let row = self . 0 . row () ; let table = self . 0 . table_mut () ; ";
    let text = [
        "pub fn set_title (& mut self , value : & str) -> :: dbsync :: Result < () > ",
        opened,
        "table . title . set (row , value) }",
    ]
    .concat();
    let scalar = [
        "pub fn set_year (& mut self , value : u32) -> :: dbsync :: Result < () > ",
        opened,
        "table . year . set (row . index () , value) }",
    ]
    .concat();

    assert!(
        out.contains(&text),
        "the row is read before the table is borrowed to write it: {out}"
    );
    assert!(out.contains(&scalar), "{out}");
}

#[test]
fn an_entity_audits_its_own_columns_even_when_it_answers_to_no_parent() {
    let out = expanded(quote! {
        struct Book { title: String }
    });

    assert!(
        out.contains(
            "impl < S > :: dbsync :: schema :: Audited < S > for Book \
             where S : :: dbsync :: schema :: Holds < Book > ,"
        ),
        "{out}"
    );
    assert!(
        out.contains("< BookTable as :: dbsync :: schema :: Table > :: audit"),
        "{out}"
    );
    assert!(
        !out.contains("audit_reference"),
        "an entity with no One field has no parent to check: {out}"
    );
}

#[test]
fn an_entity_that_names_a_parent_has_that_reference_checked_by_the_audit() {
    let out = expanded(quote! {
        struct Chapter { book: One<Book>, title: String }
    });

    assert!(
        out.contains(
            "where S : :: dbsync :: schema :: Holds < Chapter > \
             + :: dbsync :: schema :: Holds < Book > ,"
        ),
        "the audit needs both tables, so the bound names both: {out}"
    );
    assert!(
        out.contains(":: dbsync :: schema :: audit_reference :: < S , Book , Chapter > (store) ?"),
        "{out}"
    );
}

#[test]
fn an_indexed_field_asks_for_a_column_that_carries_its_own_index() {
    let out = expanded(quote! {
        struct Book {
            #[index] title: String,
            #[index(unique)] isbn: String,
            #[index] year: u32,
            author: String,
        }
    });

    for asked in [
        "title : :: dbsync :: schema :: Indexed < :: dbsync :: schema :: TextColumn , false >",
        "isbn : :: dbsync :: schema :: Indexed < :: dbsync :: schema :: TextColumn , true >",
        "year : :: dbsync :: schema :: Indexed < :: dbsync :: Arena < u32 > , false >",
        "author : :: dbsync :: schema :: TextColumn",
    ] {
        assert!(out.contains(asked), "{asked} is missing from {out}");
    }
}

#[test]
fn an_indexed_column_is_built_knowing_the_column_it_indexes() {
    let out = expanded(quote! {
        struct Book { #[index] title: String, author: String }
    });

    assert!(
        out.contains(r#"title : :: dbsync :: schema :: Indexed :: named ("book.title")"#),
        "{out}"
    );
    assert!(
        out.contains("author : :: core :: default :: Default :: default ()"),
        "{out}"
    );
}

#[test]
fn a_reference_column_is_built_knowing_the_field_that_holds_it() {
    let out = expanded(quote! {
        struct Card { credited_to: One<Author>, lemma: String }
    });

    assert!(
        out.contains(
            r#"credited_to : :: dbsync :: schema :: RefColumn :: named ("card.credited_to")"#
        ),
        "a reference knows the field it sits in, not the parent it points at: {out}"
    );
}

#[test]
fn an_indexed_field_is_reached_through_the_column_its_index_wraps() {
    let out = expanded(quote! {
        struct Book { #[index] year: u32, month: u32 }
    });

    for asked in [
        "self . year . column () . row (row . index ()) . copied ()",
        "self . month . row (row . index ()) . copied ()",
        "self . year . set (row , value)",
        "self . month . set (row . index () , value)",
    ] {
        assert!(out.contains(asked), "{asked} is missing from {out}");
    }
}

#[test]
fn what_an_author_wrote_above_a_field_is_written_above_the_way_to_read_it() {
    let out = expanded(quote! {
        struct Book {
            /// The title on the spine.
            title: String,
        }
    });

    assert_eq!(
        out.matches(r#"doc = r" The title on the spine.""#).count(),
        2,
        "what a field says about itself belongs on the table accessor and on the row reader: {out}"
    );
}

#[test]
fn a_unique_field_is_probed_before_the_id_is_minted_and_a_shared_one_is_not() {
    let out = expanded(quote! {
        struct Edition {
            #[index(unique)] isbn: String,
            #[index] year: u32,
        }
    });

    assert!(
        out.contains("self . isbn . refuse_duplicate (isbn) ?"),
        "{out}"
    );
    assert!(
        !out.contains("self . year . refuse_duplicate"),
        "a value rows may share is never refused: {out}"
    );

    let probe = out
        .find("refuse_duplicate")
        .expect("the unique field is probed");
    let mint = out.find("mint (self)").expect("an id is minted");
    assert!(
        probe < mint,
        "a duplicate is refused before the row that would have held it exists: {out}"
    );
}

#[test]
fn a_unique_field_is_found_as_one_row_and_a_shared_one_as_many() {
    let out = expanded(quote! {
        struct Edition {
            #[index(unique)] isbn: String,
            #[index] year: u32,
        }
    });

    assert!(
        out.contains("find_by_isbn (& self , value : & str ,) -> :: dbsync :: Result < :: core :: option :: Option <"),
        "{out}"
    );
    assert!(
        out.contains(
            "find_by_year (& self , value : u32 ,) -> :: dbsync :: Result < :: std :: vec :: Vec <"
        ),
        "{out}"
    );
}

#[test]
fn a_push_reaches_for_every_bucket_it_will_write_before_it_mints() {
    let out = expanded(quote! {
        struct Chapter {
            book: One<Book>,
            #[index] title: String,
            #[index(unique)] slug: String,
            text: String,
        }
    });

    assert!(out.contains("self . book . reach (book) ?"), "{out}");
    assert!(out.contains("self . title . reach (title) ?"), "{out}");
    assert!(
        !out.contains("self . text . reach"),
        "a column with no index has no bucket to reach for: {out}"
    );
    assert!(
        !out.contains("self . slug . reach"),
        "a unique field is probed, and the probe reads that bucket already: {out}"
    );

    let reach = out
        .find("reach (book)")
        .expect("the parent's bucket is reached for");
    let mint = out.find("mint (self)").expect("an id is minted");
    assert!(
        reach < mint,
        "a bucket that cannot be read must stop a push before the id exists: {out}"
    );
}

#[test]
fn a_many_that_names_a_table_to_cross_reads_the_far_side_rather_than_the_near_one() {
    let out = expanded(quote! {
        struct Book {
            title: String,
            #[through(Authorship)]
            authors: Many<Author>,
            authorships: Many<Authorship>,
        }
    });

    assert!(
        out.contains("self . 0 . across :: < Authorship , Author > ()"),
        "{out}"
    );
    assert!(
        out.contains("Result < :: dbsync :: schema :: Across < 'a , S , Author >>"),
        "{out}"
    );
    assert!(
        out.contains("Holds < Authorship > + :: dbsync :: schema :: Holds < Author >"),
        "a crossing needs both tables, not just the far one: {out}"
    );
    assert!(
        out.contains("self . 0 . children :: < Authorship > ()"),
        "a Many with no table to cross is still the children that name this row: {out}"
    );
    assert!(out.contains("fn crosses"), "{out}");
    assert!(
        out.contains("crosses :: < Book , Author , Authorship > ()"),
        "{out}"
    );
}

#[test]
fn a_through_is_refused_on_anything_but_a_many() {
    let err = refused(quote! {
        struct Book {
            #[through(Authorship)]
            title: String,
        }
    });

    assert!(err.contains("only a Many crosses a table"), "{err}");
}

#[test]
fn a_through_names_one_table_and_says_so_when_it_does_not() {
    for asked in [
        quote! { struct Book { #[through] authors: Many<Author>, t: String } },
        quote! { struct Book { #[through()] authors: Many<Author>, t: String } },
        quote! { struct Book { #[through(A, B)] authors: Many<Author>, t: String } },
    ] {
        let err = refused(asked);
        assert!(
            err.contains("names the one table the hop goes over"),
            "{err}"
        );
    }

    let err = refused(quote! {
        struct Book {
            #[through(Authorship)]
            #[through(Translation)]
            authors: Many<Author>,
            t: String,
        }
    });
    assert!(err.contains("crosses one table already"), "{err}");
}

#[test]
fn a_distinct_pair_becomes_a_column_of_its_own_that_the_push_probes_first() {
    let out = expanded(quote! {
        #[distinct(book, author)]
        struct Authorship {
            book: One<Book>,
            author: One<Author>,
            credit: String,
        }
    });

    assert!(
        out.contains("book_author : :: dbsync :: schema :: Distinct < Book , Author >"),
        "{out}"
    );
    assert!(
        out.contains(
            r#"book_author : :: dbsync :: schema :: Indexed :: named ("authorship.book_author")"#
        ),
        "{out}"
    );
    assert!(
        out.contains(r#"Field :: new ("book_author" , & self . book_author)"#),
        "the pair is a column of the table like any other: {out}"
    );

    let push = out
        .split("pub fn push")
        .nth(1)
        .expect("the table has a push");
    let probe = push.find("refuse_duplicate").expect("the push probes");
    let mint = push.find("mint").expect("the push mints");
    assert!(probe < mint, "a refused pair must not spend an id: {out}");

    assert!(
        out.contains("check_pairs :: < Book , Author , Authorship >"),
        "the audit holds the pair to the two references beside it: {out}"
    );
    assert!(
        !out.contains("# [distinct (book , author)]"),
        "the attribute is consumed, not forwarded onto the tag: {out}"
    );
}

#[test]
fn a_set_of_either_parent_in_a_distinct_pair_moves_the_pair_with_it() {
    let out = expanded(quote! {
        #[distinct(book, author)]
        struct Authorship {
            book: One<Book>,
            author: One<Author>,
        }
    });

    assert!(
        out.contains(
            "book_author . set (row , :: dbsync :: schema :: Pair :: of (value , \
             self . author . get (row) ?)) ?"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "book_author . set (row , :: dbsync :: schema :: Pair :: of (\
             table . book . get (row) ? , value)) ?"
        ),
        "{out}"
    );
}

#[test]
fn a_distinct_names_two_parents_of_this_entity_and_nothing_else() {
    let err = refused(quote! {
        #[distinct(book)]
        struct Authorship { book: One<Book>, author: One<Author> }
    });
    assert!(err.contains("exactly two parents"), "{err}");

    let err = refused(quote! {
        #[distinct(book, author, credit)]
        struct Authorship { book: One<Book>, author: One<Author>, credit: String }
    });
    assert!(err.contains("exactly two parents"), "{err}");

    let err = refused(quote! {
        #[distinct(book, credit)]
        struct Authorship { book: One<Book>, author: One<Author>, credit: String }
    });
    assert!(err.contains("`credit` is not one"), "{err}");
    assert!(err.contains("#[index(unique)]"), "{err}");

    let err = refused(quote! {
        #[distinct(book, editor)]
        struct Authorship { book: One<Book>, author: One<Author> }
    });
    assert!(
        err.contains("`editor` is not a field of this entity"),
        "{err}"
    );

    let err = refused(quote! {
        #[distinct(book, book)]
        struct Authorship { book: One<Book>, author: One<Author> }
    });
    assert!(err.contains("paired with itself"), "{err}");

    let err = refused(quote! {
        #[distinct(book, author)]
        #[distinct(author, book)]
        struct Authorship { book: One<Book>, author: One<Author> }
    });
    assert!(err.contains("says once"), "{err}");
}

#[test]
fn a_field_named_after_the_pair_it_would_share_a_column_with_is_refused() {
    let err = refused(quote! {
        #[distinct(book, author)]
        struct Authorship {
            book: One<Book>,
            author: One<Author>,
            book_author: String,
        }
    });

    assert!(
        err.contains("is held in a column called `book_author`"),
        "{err}"
    );
}
