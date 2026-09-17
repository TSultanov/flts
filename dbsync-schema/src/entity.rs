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
    let attrs = &input.attrs;
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
        match field.indexing {
            Indexing::None => quote! { #field_name: ::core::default::Default::default() },
            _ => {
                let column = format!("{label}.{field_name}");
                quote! { #field_name: ::dbsync::schema::Indexed::named(#column) }
            }
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

    let accessors = stored.iter().map(|field| {
        let field_name = &field.name;
        let setter = format_ident!("set_{}", field_name);
        let read = field.kind.read(&quote! { '_ });
        let pushed = field.kind.pushed();
        let get = field.kind.get(&quote! { self }, field_name, field.indexing);
        let set = field.kind.set(&quote! { self }, field_name, field.indexing);
        let docs = &field.docs;
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
                #set
            }
        }
    });

    let readers = fields.iter().map(|field| {
        let field_name = &field.name;
        match &field.kind {
            Kind::Many(child) => quote! {
                pub fn #field_name(
                    &self,
                ) -> ::dbsync::Result<::dbsync::schema::Children<#held, S, #child>>
                where
                    S: ::dbsync::schema::Holds<#child>,
                {
                    self.0.children::<#child>()
                }
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
        let set = field
            .kind
            .set(&quote! { table }, field_name, field.indexing);
        quote! {
            pub fn #setter(&mut self, value: #pushed) -> ::dbsync::Result<()> {
                let row = self.0.row();
                let table = self.0.table_mut();
                #set
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
        Kind::Many(child) => Some(quote_spanned_child(child, name)),
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
            #(#columns,)*
        }

        impl ::core::default::Default for #table {
            fn default() -> Self {
                Self {
                    dbsync_ids: ::core::default::Default::default(),
                    #(#built,)*
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

            fn fields(&self) -> ::std::vec::Vec<::dbsync::schema::Field<'_>> {
                ::std::vec![
                    ::dbsync::schema::Field::new("id", &self.dbsync_ids),
                    #(#read_only,)*
                ]
            }

            fn fields_mut(&mut self) -> ::std::vec::Vec<::dbsync::schema::FieldMut<'_>> {
                ::std::vec![
                    ::dbsync::schema::FieldMut::new("id", &mut self.dbsync_ids),
                    #(#read_write,)*
                ]
            }
        }

        impl #table {
            pub fn push(&mut self, #(#arguments,)*) -> ::dbsync::Result<::dbsync::Id<#name>> {
                <Self as ::dbsync::schema::Table>::reserve(self)?;
                let id = <Self as ::dbsync::schema::Table>::mint(self)?;
                #(#pushes)*
                ::core::result::Result::Ok(id)
            }

            #(#accessors)*
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
                #(
                    ::dbsync::schema::audit_reference::<S, #checked, #name>(store)?;
                )*
                ::core::result::Result::Ok(())
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

fn check_no_field_called_id(fields: &[Field]) -> syn::Result<()> {
    for field in fields {
        if field.name == "id" {
            return Err(syn::Error::new(
                field.name.span(),
                "`id` is the identity every entity already has, not a field to declare",
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
