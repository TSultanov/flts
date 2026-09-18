//! A declared entity, turned into the table that holds it.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{Fields, ItemStruct};

use crate::field::{Field, Indexing, Kind};
use crate::naming::snake_case;

pub fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    if !attr.is_empty() {
        return Err(syn::Error::new(
            attr.span(),
            "#[dbsync::entity] takes no arguments",
        ));
    }

    let input: ItemStruct = syn::parse2(item)?;
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.generics.span(),
            "an entity names columns in a file, so it cannot be generic",
        ));
    }
    let Fields::Named(named) = &input.fields else {
        return Err(syn::Error::new(
            input.fields.span(),
            "an entity needs named fields, one per column",
        ));
    };

    let fields: Vec<Field> = named
        .named
        .iter()
        .map(Field::parse)
        .collect::<syn::Result<_>>()?;
    check_one_parent_each(&fields)?;
    check_no_field_called_id(&fields)?;
    let pair = distinct_of(&input.attrs, &fields)?;

    let name = &input.ident;
    let label = snake_case(&name.to_string());
    if label == "dbsync" {
        return Err(syn::Error::new(
            name.span(),
            "the dbsync prefix is reserved for the schema's own columns",
        ));
    }

    let table = format_ident!("{}Table", name);
    let row_type = format_ident!("{}Row", name);
    let row_mut_type = format_ident!("{}RowMut", name);
    let held = quote! { 'a };
    let vis = &input.vis;
    let attrs: Vec<&syn::Attribute> = input
        .attrs
        .iter()
        .filter(|attr| !attr.path().is_ident("distinct"))
        .collect();
    let stored: Vec<&Field> = fields.iter().filter(|field| field.is_stored()).collect();
    if stored.is_empty() {
        return Err(syn::Error::new(
            name.span(),
            "an entity needs at least one stored field; a table of no columns has no rows              for a Many to group",
        ));
    }

    let columns = stored.iter().map(|field| {
        let field_name = &field.name;
        let ty = field.kind.column(name, field.indexing);
        quote! { #field_name: #ty }
    });
    let built = stored.iter().map(|field| {
        let field_name = &field.name;
        let column = format!("{label}.{field_name}");
        match (&field.kind, field.indexing) {
            (Kind::Reference(_), _) => {
                quote! { #field_name: ::dbsync::schema::RefColumn::named(#column) }
            }
            (_, Indexing::None) => quote! { #field_name: ::core::default::Default::default() },
            _ => quote! { #field_name: ::dbsync::schema::Indexed::named(#column) },
        }
    });
    let pair_column = pair.as_ref().map(|shared| {
        let column = &shared.column;
        let (left, right) = (&shared.left_parent, &shared.right_parent);
        quote! { #column: ::dbsync::schema::Distinct<#left, #right> }
    });
    let pair_built = pair.as_ref().map(|shared| {
        let column = &shared.column;
        let named = format!("{label}.{}", shared.column);
        quote! { #column: ::dbsync::schema::Indexed::named(#named) }
    });
    let pair_field = pair.as_ref().map(|shared| {
        let column = &shared.column;
        let named = shared.column.to_string();
        quote! { ::dbsync::schema::Field::new(#named, &self.#column) }
    });
    let pair_field_mut = pair.as_ref().map(|shared| {
        let column = &shared.column;
        let named = shared.column.to_string();
        quote! { ::dbsync::schema::FieldMut::new(#named, &mut self.#column) }
    });
    let pair_reach = pair.as_ref().map(|shared| {
        let (column, left, right) = (&shared.column, &shared.left, &shared.right);
        quote! {
            let pair = ::dbsync::schema::Pair::of(#left, #right);
            self.#column.reach(pair)?;
            if let ::core::option::Option::Some(taken) =
                <Self as ::dbsync::schema::Table>::live_one(
                    self,
                    self.#column.rows_holding(pair)?,
                )?
            {
                return ::core::result::Result::Err(self.#column.taken_by(taken));
            }
        }
    });
    let pair_reach_again = pair.as_ref().map(|shared| {
        let (column, left, right) = (&shared.column, &shared.left, &shared.right);
        quote! {
            let pair = ::dbsync::schema::Pair::of(#left, #right);
            self.#column.reach(pair)?;
            if let ::core::option::Option::Some(taken) =
                <Self as ::dbsync::schema::Table>::live_one(
                    self,
                    self.#column.rows_holding(pair)?,
                )?
                && taken != held
            {
                return ::core::result::Result::Err(self.#column.taken_by(taken));
            }
        }
    });
    let pair_push = pair.as_ref().map(|shared| {
        let column = &shared.column;
        quote! { self.#column.push(pair)?; }
    });
    let pair_audit = pair.as_ref().map(|shared| {
        let (column, left, right) = (&shared.column, &shared.left, &shared.right);
        let (left_ty, right_ty) = (&shared.left_parent, &shared.right_parent);
        quote! {
            ::dbsync::schema::check_pairs::<#left_ty, #right_ty, #name>(
                &::dbsync::schema::Holds::<#name>::table(store).#column,
                &::dbsync::schema::Holds::<#name>::table(store).#left,
                &::dbsync::schema::Holds::<#name>::table(store).#right,
            )?;
        }
    });

    let read_only = stored.iter().map(|field| {
        let field_name = &field.name;
        let label = field_name.to_string();
        quote! { ::dbsync::schema::Field::new(#label, &self.#field_name) }
    });
    let read_write = stored.iter().map(|field| {
        let field_name = &field.name;
        let label = field_name.to_string();
        quote! { ::dbsync::schema::FieldMut::new(#label, &mut self.#field_name) }
    });

    let arguments = stored.iter().map(|field| {
        let field_name = &field.name;
        let ty = field.kind.pushed();
        quote! { #field_name: #ty }
    });
    let pushes = stored.iter().map(|field| {
        let field_name = &field.name;
        quote! { self.#field_name.push(#field_name)?; }
    });
    let reaches = stored.iter().filter_map(|field| {
        let field_name = &field.name;
        match (&field.kind, field.indexing) {
            (Kind::Reference(_), _) | (_, Indexing::Shared) => {
                Some(quote! { self.#field_name.reach(#field_name)?; })
            }
            _ => None,
        }
    });
    let probes = stored
        .iter()
        .filter(|field| field.indexing == Indexing::Unique)
        .map(|field| {
            let field_name = &field.name;
            quote! {
                if let ::core::option::Option::Some(held) =
                    <Self as ::dbsync::schema::Table>::live_one(
                        self,
                        self.#field_name.rows_holding(#field_name)?,
                    )?
                {
                    return ::core::result::Result::Err(self.#field_name.taken_by(held));
                }
            }
        });

    let carried_whole = stored.iter().map(|field| {
        let field_name = &field.name;
        let read = field.kind.get(&quote! { self }, field_name, field.indexing);
        let owned = field.kind.owned();
        quote! { let #field_name = #read #owned ?; }
    });
    let handed_whole = stored.iter().map(|field| field.kind.lent(&field.name));

    let probes_again = stored
        .iter()
        .filter(|field| field.indexing == Indexing::Unique)
        .map(|field| {
            let field_name = &field.name;
            quote! {
                if let ::core::option::Option::Some(taken) =
                    <Self as ::dbsync::schema::Table>::live_one(
                        self,
                        self.#field_name.rows_holding(#field_name)?,
                    )?
                    && taken != held
                {
                    return ::core::result::Result::Err(self.#field_name.taken_by(taken));
                }
            }
        });

    let arguments_again = stored.iter().map(|field| {
        let field_name = &field.name;
        let ty = field.kind.pushed();
        quote! { #field_name: #ty }
    });
    let pushes_again = stored.iter().map(|field| {
        let field_name = &field.name;
        quote! { self.#field_name.push(#field_name)?; }
    });
    let reaches_again = stored.iter().filter_map(|field| {
        let field_name = &field.name;
        match (&field.kind, field.indexing) {
            (Kind::Reference(_), _) | (_, Indexing::Shared) => {
                Some(quote! { self.#field_name.reach(#field_name)?; })
            }
            _ => None,
        }
    });
    let pair_push_again = pair.as_ref().map(|shared| {
        let column = &shared.column;
        quote! { self.#column.push(pair)?; }
    });

    let finders = stored
        .iter()
        .filter(|field| field.indexing != Indexing::None)
        .map(|field| {
            let field_name = &field.name;
            let finder = format_ident!("find_by_{}", field_name);
            let pushed = field.kind.pushed();
            let docs = &field.docs;
            match field.indexing {
                Indexing::Unique => quote! {
                    #(#docs)*
                    pub fn #finder(
                        &self,
                        value: #pushed,
                    ) -> ::dbsync::Result<::core::option::Option<::dbsync::Id<#name>>> {
                        let held = <Self as ::dbsync::schema::Table>::live_one(
                            self,
                            self.#field_name.rows_holding(value)?,
                        )?;
                        match held {
                            ::core::option::Option::Some(row) => {
                                <Self as ::dbsync::schema::Table>::id_at(self, row)
                                    .map(::core::option::Option::Some)
                            }
                            ::core::option::Option::None => {
                                ::core::result::Result::Ok(::core::option::Option::None)
                            }
                        }
                    }
                },
                _ => quote! {
                    #(#docs)*
                    pub fn #finder(
                        &self,
                        value: #pushed,
                    ) -> ::dbsync::Result<::std::vec::Vec<::dbsync::Id<#name>>> {
                        let mut found = ::std::vec::Vec::new();
                        let rows = <Self as ::dbsync::schema::Table>::live_among(
                            self,
                            self.#field_name.find(value)?,
                        )?;
                        for row in rows {
                            found.push(<Self as ::dbsync::schema::Table>::id_at(self, row)?);
                        }
                        ::core::result::Result::Ok(found)
                    }
                },
            }
        });

    let accessors = stored.iter().map(|field| {
        let field_name = &field.name;
        let setter = format_ident!("set_{}", field_name);
        let read = field.kind.read(&quote! { '_ });
        let pushed = field.kind.pushed();
        let get = field.kind.get(&quote! { self }, field_name, field.indexing);
        let docs = &field.docs;

        let carried = stored
            .iter()
            .filter(|other| other.name != *field_name)
            .map(|other| {
                let other_name = &other.name;
                let read = other.kind.get(&quote! { self }, other_name, other.indexing);
                let owned = other.kind.owned();
                quote! { let #other_name = #read #owned ?; }
            });
        let handed = stored.iter().map(|other| {
            let other_name = &other.name;
            match other.name == *field_name {
                true => quote! { value },
                false => other.kind.lent(other_name),
            }
        });

        quote! {
            #(#docs)*
            pub fn #field_name(&self, of: ::dbsync::Id<#name>) -> ::dbsync::Result<#read> {
                let row = <Self as ::dbsync::schema::Table>::row_of(self, of)?;
                #get
            }

            pub fn #setter(
                &mut self,
                of: ::dbsync::Id<#name>,
                value: #pushed,
            ) -> ::dbsync::Result<()> {
                let row = <Self as ::dbsync::schema::Table>::row_of(self, of)?;
                #(#carried)*
                self.rewrite(of, #(#handed,)*)
            }
        }
    });

    let readers = fields.iter().map(|field| {
        let field_name = &field.name;
        match &field.kind {
            Kind::Many(child) => match &field.through {
                Some(join) => quote! {
                    pub fn #field_name(
                        &self,
                    ) -> ::dbsync::Result<::dbsync::schema::Across<#held, S, #child>>
                    where
                        S: ::dbsync::schema::Holds<#join> + ::dbsync::schema::Holds<#child>,
                    {
                        self.0.across::<#join, #child>()
                    }
                },
                None => quote! {
                    pub fn #field_name(
                        &self,
                    ) -> ::dbsync::Result<::dbsync::schema::Children<#held, S, #child>>
                    where
                        S: ::dbsync::schema::Holds<#child>,
                    {
                        self.0.children::<#child>()
                    }
                },
            },
            Kind::Reference(parent) => quote! {
                pub fn #field_name(
                    &self,
                ) -> ::dbsync::Result<<#parent as ::dbsync::schema::Entity>::Row<#held, S>>
                where
                    S: ::dbsync::schema::Holds<#parent>,
                {
                    let table = self.0.table();
                    let row = self.0.row();
                    self.0.at(table.#field_name.get(row)?)
                }
            },
            kind => {
                let read = kind.read(&held);
                let get = kind.get(&quote! { table }, field_name, field.indexing);
                let docs = &field.docs;
                quote! {
                    #(#docs)*
                    pub fn #field_name(&self) -> ::dbsync::Result<#read> {
                        let table = self.0.table();
                        let row = self.0.row();
                        #get
                    }
                }
            }
        }
    });

    let writers = stored.iter().map(|field| {
        let field_name = &field.name;
        let setter = format_ident!("set_{}", field_name);
        let pushed = field.kind.pushed();
        quote! {
            pub fn #setter(&mut self, value: #pushed) -> ::dbsync::Result<()> {
                let of = self.0.id()?;
                let table = self.0.table_mut();
                table.#setter(of, value)
            }
        }
    });

    let owners: Vec<&syn::Type> = fields
        .iter()
        .filter_map(|field| match &field.kind {
            Kind::Reference(parent) => Some(parent),
            _ => None,
        })
        .collect();
    let held_by = owners.iter();
    let checked = owners.iter();

    let children = fields.iter().filter_map(|field| match &field.kind {
        Kind::Reference(parent) => {
            let field_name = &field.name;
            Some(quote! {
                impl ::dbsync::schema::Child<#parent> for #name {
                    fn parents(
                        table: &#table,
                    ) -> &::dbsync::schema::RefColumn<#parent, #name> {
                        &table.#field_name
                    }
                }
            })
        }
        _ => None,
    });

    let parents = fields.iter().filter_map(|field| match &field.kind {
        Kind::Many(child) => Some(match &field.through {
            Some(join) => quote_spanned_crossing(child, join, name),
            None => quote_spanned_child(child, name),
        }),
        _ => None,
    });

    Ok(quote! {
        #(#attrs)*
        #vis struct #name;

        impl ::dbsync::schema::Entity for #name {
            const NAME: &'static str = #label;

            type Table = #table;

            type Row<#held, S> = #row_type<#held, S>
            where
                S: ::dbsync::schema::Holds<#name> + #held;

            type RowMut<#held, S> = #row_mut_type<#held, S>
            where
                S: ::dbsync::schema::Holds<#name> + #held;
        }

        #[derive(Clone)]
        #vis struct #table {
            dbsync_ids: ::dbsync::schema::IdColumn<#name>,
            dbsync_versions: ::dbsync::schema::VersionColumn<#name>,
            #(#columns,)*
            #pair_column
        }

        impl ::core::default::Default for #table {
            fn default() -> Self {
                Self {
                    dbsync_ids: ::core::default::Default::default(),
                    dbsync_versions: ::core::default::Default::default(),
                    #(#built,)*
                    #pair_built
                }
            }
        }

        impl ::dbsync::schema::Table for #table {
            type Of = #name;

            fn ids(&self) -> &::dbsync::schema::IdColumn<#name> {
                &self.dbsync_ids
            }

            fn ids_mut(&mut self) -> &mut ::dbsync::schema::IdColumn<#name> {
                &mut self.dbsync_ids
            }

            fn versions(&self) -> &::dbsync::schema::VersionColumn<#name> {
                &self.dbsync_versions
            }

            fn versions_mut(&mut self) -> &mut ::dbsync::schema::VersionColumn<#name> {
                &mut self.dbsync_versions
            }

            fn fields(&self) -> ::std::vec::Vec<::dbsync::schema::Field<'_>> {
                ::std::vec![
                    ::dbsync::schema::Field::new("id", &self.dbsync_ids),
                    ::dbsync::schema::Field::new("version", &self.dbsync_versions),
                    #(#read_only,)*
                    #pair_field
                ]
            }

            fn fields_mut(&mut self) -> ::std::vec::Vec<::dbsync::schema::FieldMut<'_>> {
                ::std::vec![
                    ::dbsync::schema::FieldMut::new("id", &mut self.dbsync_ids),
                    ::dbsync::schema::FieldMut::new("version", &mut self.dbsync_versions),
                    #(#read_write,)*
                    #pair_field_mut
                ]
            }
        }

        impl #table {
            pub fn push(&mut self, #(#arguments,)*) -> ::dbsync::Result<::dbsync::Id<#name>> {
                <Self as ::dbsync::schema::Table>::reserve(self)?;
                #(#reaches)*
                #pair_reach
                #(#probes)*
                let id = <Self as ::dbsync::schema::Table>::mint(self)?;
                #(#pushes)*
                #pair_push
                ::core::result::Result::Ok(id)
            }

            fn rewrite(&mut self, of: ::dbsync::Id<#name>, #(#arguments_again,)*)
                -> ::dbsync::Result<()>
            {
                <Self as ::dbsync::schema::Table>::reserve(self)?;
                let held = <Self as ::dbsync::schema::Table>::row_of(self, of)?;
                #(#reaches_again)*
                #pair_reach_again
                #(#probes_again)*
                <Self as ::dbsync::schema::Table>::revise(self, of)?;
                #(#pushes_again)*
                #pair_push_again
                ::core::result::Result::Ok(())
            }

            pub fn remove(&mut self, of: ::dbsync::Id<#name>) -> ::dbsync::Result<()> {
                let row = <Self as ::dbsync::schema::Table>::row_of(self, of)?;
                #(#carried_whole)*
                self.rewrite(of, #(#handed_whole,)*)?;
                <Self as ::dbsync::schema::Table>::withdraw(self, of)
            }

            #(#accessors)*

            #(#finders)*
        }

        #vis struct #row_type<#held, S: ::dbsync::schema::Holds<#name>>(
            ::dbsync::schema::Entry<#held, S, #name>,
        );

        impl<#held, S: ::dbsync::schema::Holds<#name>>
            ::core::convert::From<::dbsync::schema::Entry<#held, S, #name>>
            for #row_type<#held, S>
        {
            fn from(entry: ::dbsync::schema::Entry<#held, S, #name>) -> Self {
                Self(entry)
            }
        }

        impl<#held, S: ::dbsync::schema::Holds<#name>> ::core::ops::Deref
            for #row_type<#held, S>
        {
            type Target = ::dbsync::schema::Entry<#held, S, #name>;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl<#held, S: ::dbsync::schema::Holds<#name>> #row_type<#held, S> {
            #(#readers)*
        }

        #vis struct #row_mut_type<#held, S: ::dbsync::schema::Holds<#name>>(
            ::dbsync::schema::EntryMut<#held, S, #name>,
        );

        impl<#held, S: ::dbsync::schema::Holds<#name>>
            ::core::convert::From<::dbsync::schema::EntryMut<#held, S, #name>>
            for #row_mut_type<#held, S>
        {
            fn from(entry: ::dbsync::schema::EntryMut<#held, S, #name>) -> Self {
                Self(entry)
            }
        }

        impl<#held, S: ::dbsync::schema::Holds<#name>> ::core::ops::Deref
            for #row_mut_type<#held, S>
        {
            type Target = ::dbsync::schema::EntryMut<#held, S, #name>;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl<#held, S: ::dbsync::schema::Holds<#name>> ::core::ops::DerefMut
            for #row_mut_type<#held, S>
        {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }

        impl<#held, S: ::dbsync::schema::Holds<#name>> #row_mut_type<#held, S> {
            #(#writers)*
        }

        impl<S> ::dbsync::schema::Audited<S> for #name
        where
            S: ::dbsync::schema::Holds<#name> #(+ ::dbsync::schema::Holds<#held_by>)*,
        {
            fn audit_in(store: &S) -> ::dbsync::Result<()> {
                <#table as ::dbsync::schema::Table>::audit(
                    ::dbsync::schema::Holds::<#name>::table(store),
                )?;
                #pair_audit
                #(
                    ::dbsync::schema::audit_reference::<S, #checked, #name>(store)?;
                )*
                ::core::result::Result::Ok(())
            }
        }

        impl<S> ::dbsync::schema::Removed<S> for #name
        where
            S: ::dbsync::schema::Holds<#name>,
        {
            fn remove_in(store: &mut S, of: ::dbsync::Id<#name>) -> ::dbsync::Result<()> {
                ::dbsync::schema::Holds::<#name>::table_mut(store).remove(of)
            }
        }

        #(#children)*
        #(#parents)*
    })
}

fn quote_spanned_child(child: &syn::Type, parent: &syn::Ident) -> TokenStream {
    quote::quote_spanned! {child.span()=>
        const _: fn() = || {
            fn groups_by_its_parent<
                P: ::dbsync::schema::Entity,
                C: ::dbsync::schema::Child<P>,
            >() {
            }
            groups_by_its_parent::<#parent, #child>();
        };
    }
}

struct Shared {
    column: syn::Ident,
    left: syn::Ident,
    right: syn::Ident,
    left_parent: syn::Type,
    right_parent: syn::Type,
}

fn distinct_of(attrs: &[syn::Attribute], fields: &[Field]) -> syn::Result<Option<Shared>> {
    let mut asked: Option<&syn::Attribute> = None;

    for attr in attrs {
        if !attr.path().is_ident("distinct") {
            continue;
        }
        if asked.is_some() {
            return Err(syn::Error::new(
                attr.span(),
                "an entity says once which of its parents no two rows may share",
            ));
        }
        asked = Some(attr);
    }
    let Some(attr) = asked else {
        return Ok(None);
    };

    let named = attr
        .parse_args_with(
            syn::punctuated::Punctuated::<syn::Ident, syn::Token![,]>::parse_terminated,
        )
        .map_err(|_| {
            syn::Error::new(
                attr.span(),
                "#[distinct] names the two parents no two rows may share, as \
                 #[distinct(book, author)]",
            )
        })?;
    let mut named = named.into_iter();
    let (Some(left), Some(right), None) = (named.next(), named.next(), named.next()) else {
        return Err(syn::Error::new(
            attr.span(),
            "#[distinct] holds exactly two parents apart; one is #[index(unique)] on the field, \
             and three would be a key this does not carry yet",
        ));
    };
    if left == right {
        return Err(syn::Error::new(
            right.span(),
            "a parent paired with itself is the parent, so nothing is held apart by it",
        ));
    }

    let parent_of = |asked: &syn::Ident| -> syn::Result<syn::Type> {
        let Some(field) = fields.iter().find(|field| field.name == *asked) else {
            return Err(syn::Error::new(
                asked.span(),
                format!("`{asked}` is not a field of this entity, so there is nothing to pair"),
            ));
        };
        match &field.kind {
            Kind::Reference(parent) => Ok(parent.clone()),
            _ => Err(syn::Error::new(
                asked.span(),
                format!(
                    "a pair is made of two parents, and `{asked}` is not one; declare it \
                     One<..>, or hold the field apart with #[index(unique)]"
                ),
            )),
        }
    };
    let (left_parent, right_parent) = (parent_of(&left)?, parent_of(&right)?);

    let column = format_ident!("{left}_{right}");
    if let Some(clash) = fields.iter().find(|field| field.name == column) {
        return Err(syn::Error::new(
            clash.name.span(),
            format!("the pair of `{left}` and `{right}` is held in a column called `{column}`"),
        ));
    }

    Ok(Some(Shared {
        column,
        left,
        right,
        left_parent,
        right_parent,
    }))
}

fn quote_spanned_crossing(far: &syn::Type, join: &syn::Type, near: &syn::Ident) -> TokenStream {
    quote::quote_spanned! {join.span()=>
        const _: fn() = || {
            fn crosses<
                A: ::dbsync::schema::Entity,
                B: ::dbsync::schema::Entity,
                J: ::dbsync::schema::Child<A> + ::dbsync::schema::Child<B>,
            >() {
            }
            crosses::<#near, #far, #join>();
        };
    }
}

fn check_no_field_called_id(fields: &[Field]) -> syn::Result<()> {
    for field in fields {
        if field.name == "id" {
            return Err(syn::Error::new(
                field.name.span(),
                "`id` is the identity every entity already has, not a field to declare",
            ));
        }
        if field.name == "version" {
            return Err(syn::Error::new(
                field.name.span(),
                "`version` is the write every row already carries, not a field to declare",
            ));
        }
    }
    Ok(())
}

fn check_one_parent_each(fields: &[Field]) -> syn::Result<()> {
    let mut seen: Vec<(String, &syn::Ident)> = Vec::new();

    for field in fields {
        let Kind::Reference(parent) = &field.kind else {
            continue;
        };
        let key = quote! { #parent }.to_string();
        if let Some((_, first)) = seen.iter().find(|(seen, _)| *seen == key) {
            return Err(syn::Error::new(
                field.name.span(),
                format!(
                    "`{}` and `{}` both point at the same parent, so a Many of this entity \
                     cannot tell which one groups it",
                    first, field.name
                ),
            ));
        }
        seen.push((key, &field.name));
    }
    Ok(())
}

#[cfg(test)]
#[path = "entity_test.rs"]
mod entity_test;
