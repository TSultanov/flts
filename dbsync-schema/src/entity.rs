//! A declared entity, turned into the table that holds it.

use proc_macro2::TokenStream;
use quote::format_ident;
use syn::spanned::Spanned;
use syn::{Fields, ItemStruct};

use crate::field::{Field, Indexing, Kind};
use crate::naming::snake_case;

macro_rules! generated {
    ($($tokens:tt)*) => { quote::quote_spanned!(proc_macro2::Span::mixed_site()=> $($tokens)*) };
}

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
    let keeps = keeping_of(&input.attrs)?.map(|what| {
        generated! { const KEEPS: ::dbsync::schema::Retention = #what; }
    });

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
    let held = generated! { 'a };
    let vis = &input.vis;
    let attrs: Vec<&syn::Attribute> = input
        .attrs
        .iter()
        .filter(|attr| !attr.path().is_ident("distinct") && !attr.path().is_ident("merge"))
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
        generated! { #field_name: #ty }
    });
    let built = stored.iter().map(|field| {
        let field_name = &field.name;
        let column = format!("{label}.{field_name}");
        match (&field.kind, field.indexing) {
            (Kind::Reference(_), _) => {
                generated! { #field_name: ::dbsync::schema::RefColumn::named(#column) }
            }
            (_, Indexing::None) => generated! { #field_name: ::core::default::Default::default() },
            _ => generated! { #field_name: ::dbsync::schema::Indexed::named(#column) },
        }
    });
    let pair_column = pair.as_ref().map(|shared| {
        let column = &shared.column;
        let (left, right) = (&shared.left_parent, &shared.right_parent);
        generated! { #column: ::dbsync::schema::Distinct<#left, #right> }
    });
    let pair_built = pair.as_ref().map(|shared| {
        let column = &shared.column;
        let named = format!("{label}.{}", shared.column);
        generated! { #column: ::dbsync::schema::Indexed::named(#named) }
    });
    let pair_field = pair.as_ref().map(|shared| {
        let column = &shared.column;
        let named = shared.column.to_string();
        generated! { ::dbsync::schema::Field::new(#named, &self.#column) }
    });
    let pair_field_mut = pair.as_ref().map(|shared| {
        let column = &shared.column;
        let named = shared.column.to_string();
        generated! { ::dbsync::schema::FieldMut::new(#named, &mut self.#column) }
    });
    let pair_reach = pair.as_ref().map(|shared| {
        let (column, left, right) = (&shared.column, &shared.left, &shared.right);
        generated! {
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
        generated! {
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
        generated! { self.#column.push(pair)?; }
    });
    let pair_audit = pair.as_ref().map(|shared| {
        let (column, left, right) = (&shared.column, &shared.left, &shared.right);
        let (left_ty, right_ty) = (&shared.left_parent, &shared.right_parent);
        generated! {
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
        generated! { ::dbsync::schema::Field::new(#label, &self.#field_name) }
    });
    let read_write = stored.iter().map(|field| {
        let field_name = &field.name;
        let label = field_name.to_string();
        generated! { ::dbsync::schema::FieldMut::new(#label, &mut self.#field_name) }
    });

    let arguments = stored.iter().map(|field| {
        let field_name = &field.name;
        let ty = field.kind.pushed();
        generated! { #field_name: #ty }
    });
    let pushes = stored.iter().map(|field| {
        let field_name = &field.name;
        generated! { self.#field_name.push(#field_name)?; }
    });
    let reaches = stored.iter().filter_map(|field| {
        let field_name = &field.name;
        match (&field.kind, field.indexing) {
            (Kind::Reference(_), _) | (_, Indexing::Shared) => {
                Some(generated! { self.#field_name.reach(#field_name)?; })
            }
            _ => None,
        }
    });
    let probes = stored
        .iter()
        .filter(|field| field.indexing == Indexing::Unique)
        .map(|field| {
            let field_name = &field.name;
            generated! {
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

    let carried_theirs = stored.iter().map(|field| {
        let field_name = &field.name;
        let read = field
            .kind
            .get(&generated! { theirs }, field_name, field.indexing);
        let owned = field.kind.owned();
        generated! { let #field_name = #read #owned ?; }
    });
    let reaches_theirs = stored.iter().filter_map(|field| {
        let field_name = &field.name;
        let lent = field.kind.lent(field_name);
        match (&field.kind, field.indexing) {
            (Kind::Reference(_), _) => Some(generated! { self.#field_name.reach(#lent)?; }),
            (_, Indexing::None) => None,
            _ => Some(generated! { self.#field_name.reach(#lent)?; }),
        }
    });
    let pushes_theirs: Vec<_> = stored
        .iter()
        .map(|field| {
            let field_name = &field.name;
            let lent = field.kind.lent(field_name);
            generated! { self.#field_name.push(#lent)?; }
        })
        .collect();
    let pair_reach_plain = pair.as_ref().map(|shared| {
        let (column, left, right) = (&shared.column, &shared.left, &shared.right);
        generated! {
            let pair = ::dbsync::schema::Pair::of(#left, #right);
            self.#column.reach(pair)?;
        }
    });
    let pair_push_theirs = pair.as_ref().map(|shared| {
        let column = &shared.column;
        generated! { self.#column.push(pair)?; }
    });

    let carried_whole: Vec<_> = stored
        .iter()
        .map(|field| {
            let field_name = &field.name;
            let read = field
                .kind
                .get(&generated! { self }, field_name, field.indexing);
            let owned = field.kind.owned();
            generated! { let #field_name = #read #owned ?; }
        })
        .collect();

    let probes_again = stored
        .iter()
        .filter(|field| field.indexing == Indexing::Unique)
        .map(|field| {
            let field_name = &field.name;
            generated! {
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
        generated! { #field_name: #ty }
    });
    let pushes_again = stored.iter().map(|field| {
        let field_name = &field.name;
        generated! { self.#field_name.push(#field_name)?; }
    });
    let reaches_again = stored.iter().filter_map(|field| {
        let field_name = &field.name;
        match (&field.kind, field.indexing) {
            (Kind::Reference(_), _) | (_, Indexing::Shared) => {
                Some(generated! { self.#field_name.reach(#field_name)?; })
            }
            _ => None,
        }
    });
    let pair_push_again = pair.as_ref().map(|shared| {
        let column = &shared.column;
        generated! { self.#column.push(pair)?; }
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
                Indexing::Unique => generated! {
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
                _ => generated! {
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
        let read = field.kind.read(&generated! { '_ });
        let pushed = field.kind.pushed();
        let get = field
            .kind
            .get(&generated! { self }, field_name, field.indexing);
        let docs = &field.docs;

        let carried = stored
            .iter()
            .filter(|other| other.name != *field_name)
            .map(|other| {
                let other_name = &other.name;
                let read = other
                    .kind
                    .get(&generated! { self }, other_name, other.indexing);
                let owned = other.kind.owned();
                generated! { let #other_name = #read #owned ?; }
            });
        let handed = stored.iter().map(|other| {
            let other_name = &other.name;
            match other.name == *field_name {
                true => generated! { __dbsync_value },
                false => other.kind.lent(other_name),
            }
        });

        generated! {
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
                let __dbsync_value = value;
                let row = <Self as ::dbsync::schema::Table>::logical_row_of(self, of)?;
                #(#carried)*
                self.rewrite(of, #(#handed,)*)
            }
        }
    });

    let readers = fields.iter().map(|field| {
        let field_name = &field.name;
        match &field.kind {
            Kind::Many(child) => match &field.through {
                Some(join) => generated! {
                    pub fn #field_name(
                        &self,
                    ) -> ::dbsync::Result<::dbsync::schema::Across<#held, S, #child>>
                    where
                        S: ::dbsync::schema::Holds<#join> + ::dbsync::schema::Holds<#child>,
                    {
                        self.0.across::<#join, #child>()
                    }
                },
                None => generated! {
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
            Kind::Reference(parent) => generated! {
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
                let get = kind.get(&generated! { table }, field_name, field.indexing);
                let docs = &field.docs;
                generated! {
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
        generated! {
            pub fn #setter(&mut self, value: #pushed) -> ::dbsync::Result<()> {
                let of = self.0.id()?;
                let table = self.0.table_mut();
                table.#setter(of, value)
            }
        }
    });

    let payloads = stored.iter().map(|field| {
        let field_name = &field.name;
        let read = field
            .kind
            .get(&generated! { self }, field_name, field.indexing);
        let encoded = encode_value(&field.kind, generated! { value });
        generated! { { let value = #read?; #encoded } }
    });
    let claims = stored
        .iter()
        .filter(|field| field.indexing == Indexing::Unique)
        .map(|field| {
            let field_name = &field.name;
            let read = field
                .kind
                .get(&generated! { self }, field_name, field.indexing);
            let encoded = encode_value(&field.kind, generated! { value });
            let column = format!("{label}.{field_name}");
            let absent = match field.kind {
                Kind::NullableText | Kind::NullableScalar(_) => generated! { value.is_none() },
                _ => generated! { false },
            };
            generated! { {
                let value = #read?;
                if !#absent {
                    let mut out = ::std::vec::Vec::new();
                    #encoded
                    claims.push(::dbsync::schema::Claim { column: #column, value: out });
                }
            } }
        });
    let pair_claim = pair.as_ref().map(|shared| {
        let (left, right) = (&shared.left, &shared.right);
        let column = format!("{label}.{}", shared.column);
        generated! {
            let pair = ::dbsync::schema::Pair::of(self.#left.get(row)?, self.#right.get(row)?);
            if pair.is_whole() {
                let mut out = ::std::vec::Vec::new();
                ::dbsync::schema::encode_record(&pair, &mut out);
                claims.push(::dbsync::schema::Claim { column: #column, value: out });
            }
        }
    });
    let references = stored.iter().filter_map(|field| {
        let Kind::Reference(parent) = &field.kind else {
            return None;
        };
        let field_name = &field.name;
        let column = format!("{label}.{field_name}");
        Some(generated! { references.push(::dbsync::schema::Reference {
            column: #column, entity: <#parent as ::dbsync::schema::Entity>::NAME,
            id: self.#field_name.get(row)?.uuid(),
        }); })
    });
    let schema_fields = stored.iter().map(|field| {
        let field_name = field.name.to_string();
        let unique = field.indexing == Indexing::Unique;
        let ty = match &field.kind {
            Kind::Text => generated! { "text".to_owned() },
            Kind::NullableText => generated! { "optional text".to_owned() },
            Kind::Scalar(ty) => generated! { stringify!(#ty).to_owned() },
            Kind::NullableScalar(ty) => generated! { ::std::format!("optional {}", stringify!(#ty)) },
            Kind::Reference(ty) => generated! { ::std::format!("reference {}", <#ty as ::dbsync::schema::Entity>::NAME) },
            Kind::Enum(ty) => generated! { ::std::format!("enum {} {:?}", <#ty as ::dbsync::schema::UnitEnum>::NAME, <#ty as ::dbsync::schema::UnitEnum>::VARIANTS) },
            Kind::Many(_) => unreachable!(),
        };
        generated! { fields.push((#field_name, #ty, #unique)); }
    });
    let distinct_schema = pair
        .as_ref()
        .map(|p| format!("{}:{}", p.left, p.right))
        .unwrap_or_default();

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
            Some(generated! {
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

    Ok(generated! {
        #(#attrs)*
        #vis struct #name;

        impl ::dbsync::schema::Entity for #name {
            const NAME: &'static str = #label;
            #keeps

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

            fn payload(&self, row: ::dbsync::schema::Row) -> ::dbsync::Result<::std::vec::Vec<u8>> {
                let mut out = ::std::vec::Vec::new();
                #(#payloads)*
                ::core::result::Result::Ok(out)
            }

            fn claims(&self, row: ::dbsync::schema::Row) -> ::dbsync::Result<::std::vec::Vec<::dbsync::schema::Claim>> {
                let mut claims = ::std::vec::Vec::new();
                let _ = row;
                #(#claims)*
                #pair_claim
                ::core::result::Result::Ok(claims)
            }

            fn references(&self, row: ::dbsync::schema::Row) -> ::dbsync::Result<::std::vec::Vec<::dbsync::schema::Reference>> {
                let mut references = ::std::vec::Vec::new();
                let _ = row;
                #(#references)*
                ::core::result::Result::Ok(references)
            }

            fn schema(&self) -> ::std::string::String {
                let mut fields = ::std::vec::Vec::new();
                #(#schema_fields)*
                ::std::format!("{} {:?} distinct {}", #label, fields, #distinct_schema)
            }

            fn recover_row(&mut self, of: ::dbsync::Id<#name>, dot: ::dbsync::schema::Dot, restore: bool) -> ::dbsync::Result<()> {
                let row = <Self as ::dbsync::schema::Table>::revision_row(self, of, dot)?;
                #(#carried_whole)*
                <Self as ::dbsync::schema::Table>::reserve(self)?;
                #pair_reach_plain
                <Self as ::dbsync::schema::Table>::begin_recovery(self, of, restore)?;
                #(#pushes_theirs)*
                #pair_push_theirs
                <Self as ::dbsync::schema::Table>::refresh_one(self, of)
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
                <Self as ::dbsync::schema::Table>::refresh_one(self, id)?;
                ::core::result::Result::Ok(id)
            }

            fn rewrite(&mut self, of: ::dbsync::Id<#name>, #(#arguments_again,)*)
                -> ::dbsync::Result<()>
            {
                <Self as ::dbsync::schema::Table>::reserve(self)?;
                let held = <Self as ::dbsync::schema::Table>::logical_row_of(self, of)?;
                #(#reaches_again)*
                #pair_reach_again
                #(#probes_again)*
                <Self as ::dbsync::schema::Table>::revise(self, of)?;
                #(#pushes_again)*
                #pair_push_again
                <Self as ::dbsync::schema::Table>::refresh_one(self, of)

            }

            pub fn merge_from(
                &mut self,
                theirs: &Self,
                seen: &::dbsync::schema::VersionVector,
                told: &::dbsync::schema::VersionVector,
                report: &mut ::dbsync::schema::MergeReport,
            ) -> ::dbsync::Result<()> {
                for row in <Self as ::dbsync::schema::Table>::rows_by_dot(theirs)? {
                    let version = <Self as ::dbsync::schema::Table>::versions(theirs).raw_at(row)?;
                    let of = <Self as ::dbsync::schema::Table>::id_at(theirs, row)?;
                    if let ::core::option::Option::Some(held) =
                        <Self as ::dbsync::schema::Table>::versions(self).row_for_dot(version.dot())
                    {
                        if <Self as ::dbsync::schema::Table>::id_at(self, held)? != of
                            || <Self as ::dbsync::schema::Table>::versions(self).raw_at(held)? != version
                            || <Self as ::dbsync::schema::Table>::versions(self).parents(held)? != <Self as ::dbsync::schema::Table>::versions(theirs).parents(row)?
                            || <Self as ::dbsync::schema::Table>::payload(self, held)? != <Self as ::dbsync::schema::Table>::payload(theirs, row)?
                        { return ::core::result::Result::Err(::dbsync::Error::corrupt("revision", "a dot identifies different revisions")); }
                        report.known += 1;
                        continue;
                    }
                    let _ = (seen, told);

                    #(#carried_theirs)*
                    <Self as ::dbsync::schema::Table>::reserve(self)?;
                    #(#reaches_theirs)*
                    #pair_reach_plain
                    let parents = <Self as ::dbsync::schema::Table>::versions(theirs).parents(row)?.to_vec();
                    <Self as ::dbsync::schema::Table>::adopt_revision(self, of, version, parents)?;
                    #(#pushes_theirs)*
                    #pair_push_theirs
                    report.taken += 1;
                }
                <Self as ::dbsync::schema::Table>::check_versions(self)?;
                <Self as ::dbsync::schema::Table>::refresh(self)

            }

            pub fn settle_unique(
                &mut self,
                report: &mut ::dbsync::schema::MergeReport,
            ) -> ::dbsync::Result<()> {
                let _ = report;
                <Self as ::dbsync::schema::Table>::refresh(self)

            }

            pub fn remove(&mut self, of: ::dbsync::Id<#name>) -> ::dbsync::Result<()> {
                let row = <Self as ::dbsync::schema::Table>::logical_row_of(self, of)?;
                #(#carried_whole)*
                <Self as ::dbsync::schema::Table>::reserve(self)?;
                #pair_reach_plain
                <Self as ::dbsync::schema::Table>::begin_recovery(self, of, false)?;
                #(#pushes_theirs)*
                #pair_push_theirs
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

fn keeping_of(attrs: &[syn::Attribute]) -> syn::Result<Option<TokenStream>> {
    let mut said: Option<&syn::Attribute> = None;
    for attr in attrs {
        if !attr.path().is_ident("merge") {
            continue;
        }
        if said.is_some() {
            return Err(syn::Error::new(
                attr.span(),
                "an entity says once what a merge keeps of the versions it writes over",
            ));
        }
        said = Some(attr);
    }

    let Some(attr) = said else {
        return Ok(None);
    };

    let asked = attr
        .parse_args_with(
            syn::punctuated::Punctuated::<syn::Ident, syn::Token![,]>::parse_terminated,
        )
        .map_err(|_| syn::Error::new(attr.span(), KEEPS))?;
    let [only] = asked.iter().collect::<Vec<_>>()[..] else {
        return Err(syn::Error::new(attr.span(), KEEPS));
    };

    match only.to_string().as_str() {
        "newest" => Ok(Some(generated! { ::dbsync::schema::Retention::Newest })),
        "history" => Ok(Some(generated! { ::dbsync::schema::Retention::History })),
        written => Err(syn::Error::new(
            only.span(),
            format!("`{written}` is not something a merge keeps; {KEEPS}"),
        )),
    }
}

const KEEPS: &str = "#[merge] says what becomes of the versions a write leaves behind,                      as #[merge(newest)], which lets them be dropped, or #[merge(history)],                      which does not";

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
        let key = generated! { #parent }.to_string();
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

fn encode_value(kind: &Kind, value: TokenStream) -> TokenStream {
    match kind {
        Kind::Text => generated! { ::dbsync::schema::encode_text(&#value, &mut out); },
        Kind::NullableText => generated! {
            out.push(u8::from(#value.is_some()));
            if let ::core::option::Option::Some(value) = #value { ::dbsync::schema::encode_text(&value, &mut out); }
        },
        Kind::NullableScalar(_) => generated! {
            out.push(u8::from(#value.is_some()));
            if let ::core::option::Option::Some(value) = #value { ::dbsync::schema::encode_record(&value, &mut out); }
        },
        Kind::Enum(_) => generated! { out.push(::dbsync::schema::UnitEnum::to_u8(#value)); },
        _ => generated! { ::dbsync::schema::encode_record(&#value, &mut out); },
    }
}
