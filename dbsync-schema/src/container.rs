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
    let defaults = sets
        .iter()
        .map(|(field, _)| quote! { #field: ::core::default::Default::default() });
    let clones = sets
        .iter()
        .map(|(field, _)| quote! { #field: self.#field.clone() });
    let read_only = sets.iter().map(|(field, _)| quote! { &self.#field });
    let audits = sets.iter().map(|(_, held)| {
        quote! { <#held as ::dbsync::schema::Audited<Self>>::audit_in(self)?; }
    });
    let read_write = sets.iter().map(|(field, _)| quote! { &mut self.#field });
    let merges = sets.iter().map(|(field, _)| {
        quote! { self.#field.merge_from(&theirs.#field, &seen, &told, &mut report)?; }
    });
    let settles = sets.iter().map(|(field, _)| {
        quote! { self.#field.settle_unique(&mut report)?; }
    });
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
        #vis struct #name {
            dbsync_state: ::dbsync::schema::StoreState,
            dbsync_recovery: ::core::option::Option<::std::boxed::Box<::dbsync::schema::Recovery<Self>>>,
            #(#fields,)*
        }

        impl ::core::default::Default for #name {
            fn default() -> Self {
                let mut store = Self { dbsync_state: ::core::default::Default::default(), dbsync_recovery: None, #(#defaults,)* };
                <Self as ::dbsync::schema::Store>::bind_tables(&mut store);
                store
            }
        }

        impl ::core::clone::Clone for #name {
            fn clone(&self) -> Self {
                let mut store = Self { dbsync_state: self.dbsync_state.clone(), dbsync_recovery: self.dbsync_recovery.clone(), #(#clones,)* };
                <Self as ::dbsync::schema::Store>::bind_tables(&mut store);
                store
            }
        }

        impl ::dbsync::schema::Store for #name {
            fn recovery(&self) -> ::core::option::Option<&::dbsync::schema::Recovery<Self>> { self.dbsync_recovery.as_deref() }

            fn set_recovery(&mut self, recovery: ::core::option::Option<::dbsync::schema::Recovery<Self>>) { self.dbsync_recovery = recovery.map(::std::boxed::Box::new); }

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
                <Self as ::dbsync::schema::Store>::audit_dots(self)?;
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
                <Self as ::dbsync::schema::Store>::check_mutable(self)?;
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

            pub fn merge(
                &mut self,
                theirs: &Self,
            ) -> ::dbsync::Result<::dbsync::schema::MergeReport> {
                <Self as ::dbsync::schema::Store>::check_published(self)?;
                <Self as ::dbsync::schema::Store>::check_published(theirs)?;
                <Self as ::dbsync::schema::Store>::audit(self)?;
                <Self as ::dbsync::schema::Store>::audit(theirs)?;

                let kept = <Self as ::dbsync::schema::Store>::snapshot(self);
                let seen = <Self as ::dbsync::schema::Store>::clock(self).clone();
                let told = <Self as ::dbsync::schema::Store>::clock(theirs).clone();
                let mut report = ::dbsync::schema::MergeReport::default();

                let said = (|| -> ::dbsync::Result<()> {
                    #(#merges)*
                    #(#settles)*
                    ::core::result::Result::Ok(())
                })();

                if let ::core::result::Result::Err(err) = said {
                    *self = kept;
                    return ::core::result::Result::Err(err);
                }

                if let Err(err) = <Self as ::dbsync::schema::Store>::absorb(self, &told) { *self = kept; return Err(err); }
                match <Self as ::dbsync::schema::Store>::audit(self) {
                    ::core::result::Result::Ok(()) => match <Self as ::dbsync::schema::Store>::finish_report(self, &kept, report) {
                        ::core::result::Result::Ok(report) => ::core::result::Result::Ok(report),
                        ::core::result::Result::Err(err) => { *self = kept; ::core::result::Result::Err(err) }
                    },
                    ::core::result::Result::Err(said) => {
                        *self = kept;
                        ::core::result::Result::Err(::dbsync::Error::invalid(::std::format!(
                            "these two copies cannot be merged as they stand: {said}"
                        )))
                    }
                }
            }

            pub fn history<E>(&self, id: ::dbsync::Id<E>) -> ::dbsync::Result<::std::vec::Vec<E::Row<'_, Self>>>
            where Self: ::dbsync::schema::Holds<E>, E: ::dbsync::schema::Entity {
                let table = ::dbsync::schema::Holds::<E>::table(self);
                let mut rows = ::dbsync::schema::Table::versions_of(table, id)?;
                if rows.is_empty() { return Err(::dbsync::Error::invalid("unknown entity id")); }
                rows.sort_by_key(|row| ::dbsync::schema::Table::versions(table).raw_at(*row).map(|v| (!v.is_stamped(), v.dot(), row.index())).ok());
                Ok(rows.into_iter().map(|row| ::dbsync::schema::Entry::at_row(self, row).into()).collect())
            }

            pub fn at_version<E>(&self, id: ::dbsync::Id<E>, dot: ::dbsync::schema::Dot) -> ::dbsync::Result<E::Row<'_, Self>>
            where Self: ::dbsync::schema::Holds<E>, E: ::dbsync::schema::Entity {
                let row = ::dbsync::schema::Table::revision_row(::dbsync::schema::Holds::<E>::table(self), id, dot)?;
                Ok(::dbsync::schema::Entry::at_row(self, row).into())
            }

            pub fn status<E>(&self, id: ::dbsync::Id<E>) -> ::dbsync::Result<::dbsync::schema::RowStatus>
            where Self: ::dbsync::schema::Holds<E>, E: ::dbsync::schema::Entity {
                <Self as ::dbsync::schema::Store>::statuses(self)?.get(&(E::NAME, id.uuid())).cloned().ok_or_else(|| ::dbsync::Error::invalid("unknown entity id"))
            }

            pub fn at_hidden<E>(&self, id: ::dbsync::Id<E>) -> ::dbsync::Result<E::Row<'_, Self>>
            where Self: ::dbsync::schema::Holds<E>, E: ::dbsync::schema::Entity {
                let row = ::dbsync::schema::Table::logical_row_of(::dbsync::schema::Holds::<E>::table(self), id)?;
                Ok(::dbsync::schema::Entry::at_row(self, row).into())
            }

            pub fn at_hidden_mut<E>(&mut self, id: ::dbsync::Id<E>) -> ::dbsync::Result<E::RowMut<'_, Self>>
            where Self: ::dbsync::schema::Holds<E>, E: ::dbsync::schema::Entity {
                <Self as ::dbsync::schema::Store>::check_mutable(self)?;
                Ok(::dbsync::schema::EntryMut::hidden(self, id)?.into())
            }

            pub fn resolve<E>(&mut self, id: ::dbsync::Id<E>, dot: ::dbsync::schema::Dot) -> ::dbsync::Result<()>
            where Self: ::dbsync::schema::Holds<E>, E: ::dbsync::schema::Entity {
                self.recover_row(id, dot, false)
            }

            pub fn restore<E>(&mut self, id: ::dbsync::Id<E>, dot: ::dbsync::schema::Dot) -> ::dbsync::Result<()>
            where Self: ::dbsync::schema::Holds<E>, E: ::dbsync::schema::Entity {
                self.recover_row(id, dot, true)
            }

            fn recover_row<E>(&mut self, id: ::dbsync::Id<E>, dot: ::dbsync::schema::Dot, restore: bool) -> ::dbsync::Result<()>
            where Self: ::dbsync::schema::Holds<E>, E: ::dbsync::schema::Entity {
                <Self as ::dbsync::schema::Store>::check_mutable(self)?;
                let kept = <Self as ::dbsync::schema::Store>::snapshot(self);
                let result = (|| {
                    ::dbsync::schema::Table::recover_row(::dbsync::schema::Holds::<E>::table_mut(self), id, dot, restore)?;
                    <Self as ::dbsync::schema::Store>::audit(self)
                })();
                if result.is_err() { *self = kept; }
                result
            }

            pub fn remove<E>(&mut self, of: ::dbsync::Id<E>) -> ::dbsync::Result<()>
            where
                Self: ::dbsync::schema::Holds<E>,
                E: ::dbsync::schema::Removed<Self>,
            {
                <Self as ::dbsync::schema::Store>::check_mutable(self)?;
                let kept = <Self as ::dbsync::schema::Store>::snapshot(self);
                let result = <E as ::dbsync::schema::Removed<Self>>::remove_in(self, of)
                    .and_then(|()| <Self as ::dbsync::schema::Store>::audit(self));
                match result {
                    ::core::result::Result::Ok(()) => ::core::result::Result::Ok(()),
                    ::core::result::Result::Err(said) => {
                        *self = kept;
                        ::core::result::Result::Err(::dbsync::Error::invalid(::std::format!(
                            "{} {of} cannot be taken out while something still names it: {said}",
                            <E as ::dbsync::schema::Entity>::NAME,
                        )))
                    }
                }
            }
        }
    })
}

#[cfg(test)]
#[path = "container_test.rs"]
mod container_test;
