//! A declared container, turned into the store that owns the file its tables live in.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Fields, ItemStruct, Type};

use crate::field::set_of;

pub fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    if !attr.is_empty() {
        return Err(syn::Error::new(
            attr.span(),
            "#[dbsync::container] takes no arguments",
        ));
    }

    let input: ItemStruct = syn::parse2(item)?;
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.generics.span(),
            "a container owns one file, so it cannot be generic",
        ));
    }
    let Fields::Named(named) = &input.fields else {
        return Err(syn::Error::new(
            input.fields.span(),
            "a container needs named fields, one Set per entity it holds",
        ));
    };
    if named.named.is_empty() {
        return Err(syn::Error::new(
            input.ident.span(),
            "a container needs at least one Set to hold",
        ));
    }

    let mut sets: Vec<(syn::Ident, Type)> = Vec::new();
    for field in &named.named {
        let held = set_of(&field.ty).ok_or_else(|| {
            syn::Error::new(
                field.ty.span(),
                "every field of a container is a Set of one entity",
            )
        })?;
        let name = field.ident.clone().expect("named fields have idents");
        if let Some((first, _)) = sets
            .iter()
            .find(|(_, seen)| quote!(#seen).to_string() == quote!(#held).to_string())
        {
            return Err(syn::Error::new(
                name.span(),
                format!(
                    "`{first}` already holds this entity; two Sets of it would be two tables \
                     claiming the same columns"
                ),
            ));
        }
        sets.push((name, held.clone()));
    }

    let name = &input.ident;
    let vis = &input.vis;
    let attrs = &input.attrs;

    let fields = sets.iter().map(|(field, held)| {
        quote! { #vis #field: <#held as ::dbsync::schema::Entity>::Table }
    });
    let read_only = sets.iter().map(|(field, _)| quote! { &self.#field });
    let audits = sets.iter().map(|(_, held)| {
        quote! { <#held as ::dbsync::schema::Audited<Self>>::audit_in(self)?; }
    });
    let read_write = sets.iter().map(|(field, _)| quote! { &mut self.#field });
    let holds = sets.iter().map(|(field, held)| {
        quote! {
            impl ::dbsync::schema::Holds<#held> for #name {
                fn table(&self) -> &<#held as ::dbsync::schema::Entity>::Table {
                    &self.#field
                }

                fn table_mut(&mut self) -> &mut <#held as ::dbsync::schema::Entity>::Table {
                    &mut self.#field
                }
            }
        }
    });

    Ok(quote! {
        #(#attrs)*
        #[derive(Default, Clone)]
        #vis struct #name {
            dbsync_state: ::dbsync::schema::StoreState,
            #(#fields,)*
        }

        impl ::dbsync::schema::Store for #name {
            fn state(&self) -> &::dbsync::schema::StoreState {
                &self.dbsync_state
            }

            fn state_mut(&mut self) -> &mut ::dbsync::schema::StoreState {
                &mut self.dbsync_state
            }

            fn tables(&self) -> ::std::vec::Vec<&dyn ::dbsync::schema::AnyTable> {
                ::std::vec![#(#read_only,)*]
            }

            fn tables_mut(&mut self) -> ::std::vec::Vec<&mut dyn ::dbsync::schema::AnyTable> {
                ::std::vec![#(#read_write,)*]
            }

            fn audit(&self) -> ::dbsync::Result<()> {
                #(#audits)*
                ::core::result::Result::Ok(())
            }
        }

        #(#holds)*

        impl #name {
            pub fn at<E>(
                &self,
                id: ::dbsync::Id<E>,
            ) -> ::dbsync::Result<<E as ::dbsync::schema::Entity>::Row<'_, Self>>
            where
                Self: ::dbsync::schema::Holds<E>,
                E: ::dbsync::schema::Entity,
            {
                ::core::result::Result::Ok(::core::convert::Into::into(
                    ::dbsync::schema::Entry::of(self, id)?,
                ))
            }

            pub fn at_mut<E>(
                &mut self,
                id: ::dbsync::Id<E>,
            ) -> ::dbsync::Result<<E as ::dbsync::schema::Entity>::RowMut<'_, Self>>
            where
                Self: ::dbsync::schema::Holds<E>,
                E: ::dbsync::schema::Entity,
            {
                ::core::result::Result::Ok(::core::convert::Into::into(
                    ::dbsync::schema::EntryMut::of(self, id)?,
                ))
            }

            pub fn all<E>(&self) -> ::dbsync::schema::Rows<'_, Self, E>
            where
                Self: ::dbsync::schema::Holds<E>,
                E: ::dbsync::schema::Entity,
            {
                ::dbsync::schema::Rows::all(self)
            }

            pub fn children_of<P, C>(
                &self,
                parent: ::dbsync::Id<P>,
            ) -> ::dbsync::Result<::std::vec::Vec<::dbsync::Id<C>>>
            where
                Self: ::dbsync::schema::Holds<P> + ::dbsync::schema::Holds<C>,
                P: ::dbsync::schema::Entity,
                C: ::dbsync::schema::Child<P>,
            {
                ::dbsync::schema::children_of(self, parent)
            }
        }
    })
}

#[cfg(test)]
#[path = "container_test.rs"]
mod container_test;
