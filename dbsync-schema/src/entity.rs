//! A declared entity, turned into the table that holds it.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{Fields, ItemStruct};

use crate::field::{Field, Kind};
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
        let ty = field.kind.column(name);
        quote! { #field_name: #ty }
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
        let get = field.kind.get(&quote! { self }, field_name);
        let set = field.kind.set(&quote! { self }, field_name);
        quote! {
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

    let readers = stored.iter().map(|field| {
        let field_name = &field.name;
        let read = field.kind.read(&held);
        let get = field.kind.get(&quote! { table }, field_name);
        quote! {
            pub fn #field_name(&self) -> ::dbsync::Result<#read> {
                let table = self.0.table();
                let row = self.0.row();
                #get
            }
        }
    });

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
        }

        #[derive(Default, Clone)]
        #vis struct #table {
            #(#columns,)*
        }

        impl ::dbsync::schema::Table for #table {
            type Of = #name;

            fn fields(&self) -> ::std::vec::Vec<::dbsync::schema::Field<'_>> {
                ::std::vec![#(#read_only,)*]
            }

            fn fields_mut(&mut self) -> ::std::vec::Vec<::dbsync::schema::FieldMut<'_>> {
                ::std::vec![#(#read_write,)*]
            }
        }

        impl #table {
            pub fn push(&mut self, #(#arguments,)*) -> ::dbsync::Result<::dbsync::Id<#name>> {
                <Self as ::dbsync::schema::Table>::reserve(self)?;
                let row = <Self as ::dbsync::schema::Table>::rows(self);
                #(#pushes)*
                ::core::result::Result::Ok(::dbsync::Id::from_index(row))
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
