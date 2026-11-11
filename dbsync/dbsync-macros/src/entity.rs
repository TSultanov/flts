//! A declared entity, turned into the table that holds it.

use proc_macro2::TokenStream;
use quote::format_ident;
use syn::spanned::Spanned;
use syn::{Fields, ItemStruct};

use crate::field::{Field, Indexing, Kind, Sorting};
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
    check_orders(&fields)?;
    check_no_field_called_id(&fields)?;
    if let Some(attr) = input
        .attrs
        .iter()
        .find(|attr| attr.path().is_ident("distinct"))
    {
        return Err(syn::Error::new(
            attr.span(),
            "only entity UUIDs are unique; distinct parent pairs are not supported",
        ));
    }
    let (retention, resolver) = merging_of(&input.attrs)?;
    let keeps =
        retention.map(|what| generated! { const KEEPS: ::dbsync::schema::Retention = #what; });

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
    let value_type = format_ident!("{}Value", name);
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

    let value_fields = stored.iter().map(|field| {
        let field_name = &field.name;
        let ty = match &field.kind {
            Kind::Text => generated! { ::std::string::String },
            Kind::NullableText => generated! { ::core::option::Option<::std::string::String> },
            Kind::Bytes => generated! { ::std::vec::Vec<u8> },
            Kind::NullableBytes => generated! { ::core::option::Option<::std::vec::Vec<u8>> },
            other => other.pushed(),
        };
        generated! { pub #field_name: #ty }
    });
    let value_names: Vec<_> = stored.iter().map(|field| &field.name).collect();
    let current_whole: Vec<_> = stored
        .iter()
        .map(|field| {
            let field_name = &field.name;
            generated! { let #field_name = current.#field_name; }
        })
        .collect();
    let current_read = if resolver.is_some() {
        generated! { let current = self.dbsync_value(of)?; #(#current_whole)* }
    } else {
        let reads = stored.iter().map(|field| {
            let field_name = &field.name;
            let read = field.get(&generated! { self });
            let owned = field.kind.owned();
            generated! { let #field_name = #read #owned ?; }
        });
        generated! { let row = <Self as ::dbsync::schema::Table>::logical_row_of(self, of)?; #(#reads)* }
    };

    let columns = stored.iter().map(|field| {
        let field_name = &field.name;
        let prefix = prefix_of(field, &fields);
        let ty = field.kind.column(name, field.indexing, prefix.as_ref());
        generated! { #field_name: #ty }
    });
    let built = stored.iter().map(|field| {
        let field_name = &field.name;
        let column = format!("{label}.{field_name}");
        match (&field.kind, field.indexing) {
            (Kind::Reference(_), _) => {
                generated! { #field_name: ::dbsync::schema::RefColumn::named(#column) }
            }
            (Kind::NullableReference(_), _) => {
                generated! { #field_name: ::dbsync::schema::OptionalRefColumn::named(#column) }
            }
            _ if field.sorting != Sorting::None => {
                generated! { #field_name: ::dbsync::schema::Sorted::named(#column) }
            }
            (_, Indexing::None) => generated! { #field_name: ::core::default::Default::default() },
            _ => generated! { #field_name: ::dbsync::schema::Indexed::named(#column) },
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
            (Kind::Reference(_) | Kind::NullableReference(_), _) | (_, Indexing::Shared) => {
                Some(generated! { self.#field_name.reach(#field_name)?; })
            }
            _ => None,
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
    let carried_whole: Vec<_> = stored
        .iter()
        .map(|field| {
            let field_name = &field.name;
            let read = field.get(&generated! { self });
            let owned = field.kind.owned();
            generated! { let #field_name = #read #owned ?; }
        })
        .collect();

    let arguments_given: Vec<_> = stored
        .iter()
        .map(|field| {
            let field_name = &field.name;
            let ty = field.kind.pushed();
            generated! { #field_name: #ty }
        })
        .collect();
    let given: Vec<_> = stored.iter().map(|field| &field.name).collect();
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
            (Kind::Reference(_) | Kind::NullableReference(_), _) | (_, Indexing::Shared) => {
                Some(generated! { self.#field_name.reach(#field_name)?; })
            }
            _ => None,
        }
    });
    let finders = stored.iter().filter(|field| field.indexing != Indexing::None).map(|field| {
        let field_name = &field.name;
        let finder = format_ident!("find_by_{}", field_name);
        let pushed = field.kind.pushed();
        let equality = match &field.kind {
            Kind::Text => generated! { row_value.#field_name.as_str() == value },
            Kind::NullableText => generated! { row_value.#field_name.as_deref() == value },
            Kind::Enum(_) => generated! { ::dbsync::schema::UnitEnum::to_u8(row_value.#field_name) == ::dbsync::schema::UnitEnum::to_u8(value) },
            _ => generated! { row_value.#field_name == value },
        };
        let resolved = resolver.as_ref().map(|_| generated! {
            rows.retain(|row| <Self as ::dbsync::schema::Table>::id_at(self, *row).is_ok_and(|id| self.dbsync_resolved.get(id.uuid()).is_none()));
            for (uuid, row_value) in self.dbsync_resolved.entries() {
                if #equality && let Some(row) = <Self as ::dbsync::schema::Table>::live_row_of(self, ::dbsync::Id::from_uuid(uuid))? { rows.push(row); }
            }
            rows.sort_by_key(|row| row.index());
        });
        generated! {
            pub fn #finder(&self, value: #pushed) -> ::dbsync::Result<::std::vec::Vec<::dbsync::Id<#name>>> {
                let mut rows = <Self as ::dbsync::schema::Table>::live_among(self, self.#field_name.find(value)?)?;
                #resolved
                rows.into_iter().map(|row| <Self as ::dbsync::schema::Table>::id_at(self, row)).collect()
            }
        }
    });

    let ranges = stored.iter().filter(|field| field.sorting != Sorting::None).map(|field| {
        let field_name = &field.name;
        let finder = format_ident!("by_{}", field_name);
        let (bound, value) = sort_value(&field.kind);
        let key = sort_key_of(&field.kind, generated! { row_value.#field_name });
        let (parent, lookup, belongs) = match &field.sorting {
            Sorting::Within(by) => {
                let parent_type = parent_named(by, &fields);
                (
                    Some(generated! { parent: ::dbsync::Id<#parent_type>, }),
                    generated! { self.#field_name.children_between(parent, low, high, &self.#by)? },
                    generated! { row_value.#by == parent && },
                )
            }
            _ => (None, generated! { self.#field_name.between(low, high)? }, generated! {}),
        };
        let resolved = resolver.as_ref().map(|_| generated! {
            keyed.retain(|(_, row)| <Self as ::dbsync::schema::Table>::id_at(self, *row).is_ok_and(|id| self.dbsync_resolved.get(id.uuid()).is_none()));
            for (uuid, row_value) in self.dbsync_resolved.entries() {
                let key = #key;
                if #belongs (low..=high).contains(&key) && let Some(row) = <Self as ::dbsync::schema::Table>::live_row_of(self, ::dbsync::Id::from_uuid(uuid))? { keyed.push((key, row)); }
            }
        });
        generated! {
            pub fn #finder(&self, #parent range: impl ::core::ops::RangeBounds<#bound>) -> ::dbsync::Result<::std::vec::Vec<::dbsync::Id<#name>>> {
                let range = (
                    ::core::ops::RangeBounds::start_bound(&range).map(|value| #value),
                    ::core::ops::RangeBounds::end_bound(&range).map(|value| #value),
                );
                let ::core::option::Option::Some((low, high)) = ::dbsync::schema::key_bounds(&range) else {
                    return ::core::result::Result::Ok(::std::vec::Vec::new());
                };
                #[allow(unused_mut)]
                let mut keyed = #lookup;
                #resolved
                <Self as ::dbsync::schema::Table>::live_in_key_order(self, keyed)?
                    .into_iter()
                    .map(|row| <Self as ::dbsync::schema::Table>::id_at(self, row))
                    .collect()
            }
        }
    });
    let folds: Vec<_> = stored
        .iter()
        .filter_map(|field| {
            let field_name = &field.name;
            match &field.sorting {
                Sorting::None => None,
                Sorting::Sorted => Some(generated! { self.#field_name.fold(force)?; }),
                Sorting::Within(by) => {
                    Some(generated! { self.#field_name.fold_within(&self.#by, force)?; })
                }
            }
        })
        .collect();
    let prefix_audits: Vec<_> = stored
        .iter()
        .filter_map(|field| {
            let field_name = &field.name;
            match &field.sorting {
                Sorting::Within(by) => {
                    Some(generated! { self.#field_name.audit_within(&self.#by)?; })
                }
                _ => None,
            }
        })
        .collect();
    let index_hooks = (!folds.is_empty()).then(|| {
        generated! {
            fn settle_indexes(&mut self, force: bool) -> ::dbsync::Result<()> {
                #(#folds)*
                ::core::result::Result::Ok(())
            }

            fn audit_indexes(&self) -> ::dbsync::Result<()> {
                #(#prefix_audits)*
                ::core::result::Result::Ok(())
            }
        }
    });

    let accessors = stored.iter().map(|field| {
        let field_name = &field.name;
        let setter = format_ident!("set_{}", field_name);
        let read = field.kind.read(&generated! { '_ });
        let pushed = field.kind.pushed();
        let get = field.get(&generated! { self });
        let docs = &field.docs;

        let carried = stored.iter().filter(|other| other.name != *field_name).map(|other| {
            let other_name = &other.name;
            if resolver.is_some() { generated! { let #other_name = current.#other_name; } }
            else {
                let read = other.get(&generated! { self });
                let owned = other.kind.owned();
                generated! { let #other_name = #read #owned ?; }
            }
        });
        let begin = if resolver.is_some() { generated! { let current = self.dbsync_value(of)?; } }
            else { generated! { let row = <Self as ::dbsync::schema::Table>::logical_row_of(self, of)?; } };
        let cached = resolver.as_ref().map(|_| {
            let value = match &field.kind {
                Kind::Text | Kind::Bytes => generated! { ::std::borrow::Cow::Owned(current.#field_name.clone()) },
                Kind::NullableText | Kind::NullableBytes => generated! { current.#field_name.clone().map(::std::borrow::Cow::Owned) },
                _ => generated! { current.#field_name },
            };
            generated! { if let Some(current) = self.dbsync_resolved.get(of.uuid()) { return Ok(#value); } }
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
                #cached
                #get
            }

            pub fn #setter(
                &mut self,
                of: ::dbsync::Id<#name>,
                value: #pushed,
            ) -> ::dbsync::Result<()> {
                let __dbsync_value = value;
                #begin
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
                    self.0.at(<#name as ::dbsync::schema::Child<#parent>>::parent_at(table, row, self.0.is_historical())?)
                }
            },
            Kind::NullableReference(parent) => generated! {
                pub fn #field_name(
                    &self,
                ) -> ::dbsync::Result<::core::option::Option<<#parent as ::dbsync::schema::Entity>::Row<#held, S>>>
                where
                    S: ::dbsync::schema::Holds<#parent>,
                {
                    let table = self.0.table();
                    let row = self.0.row();
                    let parent = <#name as ::dbsync::schema::Child<#parent>>::parent_at(table, row, self.0.is_historical())?;
                    if parent.is_nil() { return ::core::result::Result::Ok(::core::option::Option::None); }
                    self.0.at(parent).map(::core::option::Option::Some)
                }
            },
            kind => {
                let read = kind.read(&held);
                let get = field.get(&generated! { table });
                let docs = &field.docs;
                let current = resolver.as_ref().map(|_| generated! { if !self.0.is_historical() { return table.#field_name(self.0.id()?); } });
                generated! {
                    #(#docs)*
                    pub fn #field_name(&self) -> ::dbsync::Result<#read> {
                        let table = self.0.table();
                        let row = self.0.row();
                        #current
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
        let read = field.get(&generated! { self });
        let encoded = encode_value(&field.kind, generated! { value });
        generated! { { let value = #read?; #encoded } }
    });
    let references = stored.iter().filter_map(|field| {
        let (parent, held) = match &field.kind {
            Kind::Reference(parent) => (parent, generated! {}),
            Kind::NullableReference(parent) => (parent, generated! { .references() }),
            _ => return None,
        };
        let field_name = &field.name;
        let column = format!("{label}.{field_name}");
        Some(generated! { references.push(::dbsync::schema::Reference {
            column: #column, entity: <#parent as ::dbsync::schema::Entity>::NAME,
            id: self.#field_name #held.get(row)?.uuid(),
        }); })
    });
    let reference_columns = stored.iter().filter_map(|field| {
        let (Kind::Reference(parent) | Kind::NullableReference(parent)) = &field.kind else {
            return None;
        };
        let column = format!("{label}.{}", field.name);
        Some(generated! { (#column, <#parent as ::dbsync::schema::Entity>::NAME) })
    });
    let resolved_references: Vec<_> = stored
        .iter()
        .filter_map(|field| {
            let field_name = &field.name;
            let (parent, id) = match &field.kind {
                Kind::Reference(parent) => (parent, generated! { value.#field_name.uuid() }),
                Kind::NullableReference(parent) => (
                    parent,
                    generated! { value.#field_name.map_or(::dbsync::Uuid::NIL, |id| id.uuid()) },
                ),
                _ => return None,
            };
            let column = format!("{label}.{field_name}");
            Some(
                generated! { references.push(::dbsync::schema::Reference { column: #column,
                entity: <#parent as ::dbsync::schema::Entity>::NAME, id: #id }); },
            )
        })
        .collect();
    let checks = stored.iter().filter_map(|field| {
        let field_name = &field.name;
        match &field.kind {
            Kind::Enum(ty) => Some(generated! { <#ty as ::dbsync::schema::UnitEnum>::from_u8(::dbsync::schema::UnitEnum::to_u8(value.#field_name))?; }),
            Kind::NullableEnum(ty) => Some(generated! { if let ::core::option::Option::Some(held) = value.#field_name { <#ty as ::dbsync::schema::UnitEnum>::from_u8(::dbsync::schema::UnitEnum::to_u8(held))?; } }),
            _ => None,
        }
    });
    let resolution = resolver.as_ref().map(|function| generated! {
        fn resolve(&self, id: ::dbsync::Id<#name>) -> ::dbsync::Result<()> {
            let rows = <Self as ::dbsync::schema::Table>::eligible_heads(self, id)?;
            if rows.len() < 2 { self.dbsync_resolved.remove(id.uuid()); return Ok(()); }
            let sources = rows.iter().map(|row| self.dbsync_versions.raw_at(*row).map(|v| v.dot())).collect::<::dbsync::Result<::std::vec::Vec<_>>>()?;
            if self.dbsync_resolved.sources(id.uuid()).as_ref() == Some(&sources) { return Ok(()); }
            let mut candidates = ::std::vec::Vec::new();
            let mut supporting = ::std::collections::BTreeSet::new();
            for row in rows {
                supporting.insert(row.index());
                supporting.extend(self.dbsync_versions.ancestors(row)?);
                let version = self.dbsync_versions.raw_at(row)?;
                if !version.is_stamped() { return Err(::dbsync::Error::invalid("concurrent merge inputs need published revision identities")); }
                candidates.push(::dbsync::schema::MergeCandidate { version, value: self.dbsync_raw_value(row)? });
            }
            let mut ancestry = ::std::vec::Vec::new();
            for index in supporting {
                let row = ::dbsync::schema::Row::of(index, <Self as ::dbsync::schema::Table>::rows(self), <#name as ::dbsync::schema::Entity>::NAME)?;
                let version = self.dbsync_versions.raw_at(row)?;
                let parents = self.dbsync_versions.parents(row)?.iter().map(|parent| match parent {
                    ::dbsync::schema::Parent::Published(dot) => Ok(*dot),
                    _ => Err(::dbsync::Error::invalid("merge ancestry must be published")),
                }).collect::<::dbsync::Result<::std::vec::Vec<_>>>()?;
                ancestry.push(::dbsync::schema::RevisionAncestry { dot: version.dot(), parents });
            }
            ancestry.sort_by_key(|revision| revision.dot);
            let input = ::dbsync::schema::MergeInput { id: id.uuid(), candidates: &candidates, ancestry: &ancestry };
            let value: #value_type = #function(&input);
            #(#checks)*
            self.dbsync_resolved.insert(id.uuid(), sources, value);
            Ok(())
        }

        fn invalidate_resolved(&self) { self.dbsync_resolved.clear(); }

        fn is_derived(&self, id: ::dbsync::Id<#name>) -> bool { self.dbsync_resolved.get(id.uuid()).is_some() }

        fn source_dots(&self, id: ::dbsync::Id<#name>) -> ::dbsync::Result<::std::vec::Vec<::dbsync::schema::Dot>> {
            if let Some(sources) = self.dbsync_resolved.sources(id.uuid()) { return Ok(sources); }
            <Self as ::dbsync::schema::Table>::versions_of(self, id)?.into_iter()
                .filter(|row| <Self as ::dbsync::schema::Table>::seen_at(self, *row).is_ok_and(|v| v.is_current()))
                .map(|row| self.dbsync_versions.raw_at(row).map(|v| v.dot())).collect()
        }

        fn current_references(&self, id: ::dbsync::Id<#name>, row: ::dbsync::schema::Row) -> ::dbsync::Result<::std::vec::Vec<::dbsync::schema::Reference>> {
            if let Some(value) = self.dbsync_resolved.get(id.uuid()) {
                let mut references = ::std::vec::Vec::new();
                let _ = &value;
                #(#resolved_references)*
                return Ok(references);
            }
            <Self as ::dbsync::schema::Table>::references(self, row)
        }

        fn additional_references(&self) -> ::dbsync::Result<::std::vec::Vec<::dbsync::schema::Reference>> {
            let mut references = ::std::vec::Vec::new();
            for (_, value) in self.dbsync_resolved.entries() {
                let _ = &value;
                #(#resolved_references)*
            }
            Ok(references)
        }
    });

    let owners: Vec<&syn::Type> = fields
        .iter()
        .filter_map(|field| match &field.kind {
            Kind::Reference(parent) | Kind::NullableReference(parent) => Some(parent),
            _ => None,
        })
        .collect();
    let held_by = owners.iter();

    let children = fields.iter().filter_map(|field| match &field.kind {
        Kind::Reference(parent) | Kind::NullableReference(parent) => {
            let field_name = &field.name;
            let optional = matches!(field.kind, Kind::NullableReference(_));
            let (column, named, same) = match optional {
                true => (
                    generated! { table.#field_name.references() },
                    generated! { table.dbsync_value(id)?.#field_name.unwrap_or(::dbsync::Id::NIL) },
                    generated! { value.#field_name == ::core::option::Option::Some(parent) },
                ),
                false => (
                    generated! { table.#field_name },
                    generated! { table.dbsync_value(id)?.#field_name },
                    generated! { value.#field_name == parent },
                ),
            };
            let position = stored.iter().find(|other| other.sorting == Sorting::Within(field_name.clone()));
            let resolved_parent = resolver.as_ref().map(|_| generated! {
                fn parent_at(table: &#table, row: ::dbsync::schema::Row, historical: bool) -> ::dbsync::Result<::dbsync::Id<#parent>> {
                    if historical { return #column.get(row); }
                    let id = <#table as ::dbsync::schema::Table>::id_at(table, row)?;
                    Ok(#named)
                }
            });
            let ordered = position.map(|position| {
                let position_name = &position.name;
                let key = sort_key_of(&position.kind, generated! { value.#position_name });
                let patch = resolver.as_ref().map(|_| generated! {
                    keyed.retain(|(_, row)| <#table as ::dbsync::schema::Table>::id_at(table, *row).is_ok_and(|id| table.dbsync_resolved.get(id.uuid()).is_none()));
                    for (uuid, value) in table.dbsync_resolved.entries() {
                        if #same && let Some(row) = <#table as ::dbsync::schema::Table>::live_row_of(table, ::dbsync::Id::from_uuid(uuid))? { keyed.push((#key, row)); }
                    }
                });
                generated! {
                    fn child_rows(table: &#table, parent: ::dbsync::Id<#parent>) -> ::dbsync::Result<::std::vec::Vec<::dbsync::schema::Row>> {
                        #[allow(unused_mut)]
                        let mut keyed = table.#position_name.children(parent, &table.#field_name)?;
                        #patch
                        <#table as ::dbsync::schema::Table>::live_in_key_order(table, keyed)
                    }
                }
            });
            let resolved = match ordered {
                Some(ordered) => Some(generated! { #resolved_parent #ordered }),
                None => resolver.as_ref().map(|_| generated! {
                #resolved_parent

                fn child_rows(table: &#table, parent: ::dbsync::Id<#parent>) -> ::dbsync::Result<::std::vec::Vec<::dbsync::schema::Row>> {
                    let mut rows = <#table as ::dbsync::schema::Table>::live_among(table, #column.child_rows_of(parent)?)?;
                    rows.retain(|row| <#table as ::dbsync::schema::Table>::id_at(table, *row).is_ok_and(|id| table.dbsync_resolved.get(id.uuid()).is_none()));
                    for (uuid, value) in table.dbsync_resolved.entries() {
                        if #same && let Some(row) = <#table as ::dbsync::schema::Table>::live_row_of(table, ::dbsync::Id::from_uuid(uuid))? { rows.push(row); }
                    }
                    <#table as ::dbsync::schema::Table>::in_creation_order(table, &mut rows)?;
                    Ok(rows)
                }
                }),
            };
            Some(generated! {
                impl ::dbsync::schema::Child<#parent> for #name {
                    #resolved

                    fn parents(
                        table: &#table,
                    ) -> &::dbsync::schema::RefColumn<#parent, #name> {
                        &#column
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

        #[derive(Clone)]
        #vis struct #value_type { #(#value_fields,)* }

        impl ::dbsync::schema::Entity for #name {
            const NAME: &'static str = #label;
            #keeps

            type Value = #value_type;
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
            dbsync_resolved: ::dbsync::schema::Resolved<#value_type>,
            #(#columns,)*
        }

        impl ::core::default::Default for #table {
            fn default() -> Self {
                Self {
                    dbsync_ids: ::core::default::Default::default(),
                    dbsync_versions: ::core::default::Default::default(),
                    dbsync_resolved: ::core::default::Default::default(),
                    #(#built,)*
                }
            }
        }

        impl ::dbsync::schema::Table for #table {
            type Of = #name;

            #resolution

            #index_hooks

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

            fn reference_columns(&self) -> ::std::vec::Vec<(&'static str, &'static str)> {
                ::std::vec![#(#reference_columns,)*]
            }

            fn references(&self, row: ::dbsync::schema::Row) -> ::dbsync::Result<::std::vec::Vec<::dbsync::schema::Reference>> {
                let mut references = ::std::vec::Vec::new();
                let _ = row;
                #(#references)*
                ::core::result::Result::Ok(references)
            }

            fn recover_row(&mut self, of: ::dbsync::Id<#name>, dot: ::dbsync::schema::Dot, restore: bool) -> ::dbsync::Result<()> {
                let row = <Self as ::dbsync::schema::Table>::revision_row(self, of, dot)?;
                #(#carried_whole)*
                <Self as ::dbsync::schema::Table>::reserve(self)?;
                <Self as ::dbsync::schema::Table>::begin_recovery(self, of, restore)?;
                #(#pushes_theirs)*
                <Self as ::dbsync::schema::Table>::refresh_one(self, of)
            }

            fn fields(&self) -> ::std::vec::Vec<::dbsync::schema::Field<'_>> {
                ::std::vec![
                    ::dbsync::schema::Field::new("id", &self.dbsync_ids),
                    ::dbsync::schema::Field::new("version", &self.dbsync_versions),
                    #(#read_only,)*
                ]
            }

            fn fields_mut(&mut self) -> ::std::vec::Vec<::dbsync::schema::FieldMut<'_>> {
                ::std::vec![
                    ::dbsync::schema::FieldMut::new("id", &mut self.dbsync_ids),
                    ::dbsync::schema::FieldMut::new("version", &mut self.dbsync_versions),
                    #(#read_write,)*
                ]
            }
        }

        impl ::dbsync::schema::MigrationTable for #table {
            fn revision_value(&self, row: ::dbsync::schema::Row) -> ::dbsync::Result<#value_type> {
                self.dbsync_raw_value(row)
            }

            fn append_revision_value(
                &mut self,
                id: ::dbsync::Id<#name>,
                version: ::dbsync::schema::Version,
                parents: ::std::vec::Vec<::dbsync::schema::Parent>,
                value: #value_type,
            ) -> ::dbsync::Result<()> {
                let #value_type { #(#value_names,)* } = value;
                <Self as ::dbsync::schema::Table>::reserve(self)?;
                <Self as ::dbsync::schema::Table>::adopt_revision(self, id, version, parents)?;
                #(#pushes_theirs)*
                Ok(())
            }
        }

        impl #table {
            fn dbsync_raw_value(&self, row: ::dbsync::schema::Row) -> ::dbsync::Result<#value_type> {
                #(#carried_whole)*
                Ok(#value_type { #(#value_names,)* })
            }

            fn dbsync_value(&self, of: ::dbsync::Id<#name>) -> ::dbsync::Result<#value_type> {
                let row = <Self as ::dbsync::schema::Table>::logical_row_of(self, of)?;
                match self.dbsync_resolved.get(of.uuid()) {
                    Some(value) => Ok((*value).clone()),
                    None => self.dbsync_raw_value(row),
                }
            }

            pub fn push(&mut self, #(#arguments_given,)*) -> ::dbsync::Result<::dbsync::Id<#name>> {
                let id = ::dbsync::Id::mint();
                self.insert(id, #(#given,)*)?;
                ::core::result::Result::Ok(id)
            }

            pub fn insert(&mut self, id: ::dbsync::Id<#name>, #(#arguments,)*) -> ::dbsync::Result<()> {
                if !<Self as ::dbsync::schema::Table>::versions_of(self, id)?.is_empty() {
                    return ::core::result::Result::Err(::dbsync::Error::invalid(::std::format!(
                        "{} already has a row with id {id}",
                        <#name as ::dbsync::schema::Entity>::NAME
                    )));
                }
                <Self as ::dbsync::schema::Table>::reserve(self)?;
                #(#reaches)*
                <Self as ::dbsync::schema::Table>::mint_as(self, id)?;
                #(#pushes)*
                <Self as ::dbsync::schema::Table>::refresh_one(self, id)
            }

            fn rewrite(&mut self, of: ::dbsync::Id<#name>, #(#arguments_again,)*)
                -> ::dbsync::Result<()>
            {
                <Self as ::dbsync::schema::Table>::reserve(self)?;
                #(#reaches_again)*
                <Self as ::dbsync::schema::Table>::revise(self, of)?;
                #(#pushes_again)*
                <Self as ::dbsync::schema::Table>::refresh_one(self, of)

            }

            pub fn remove(&mut self, of: ::dbsync::Id<#name>) -> ::dbsync::Result<()> {
                #current_read
                <Self as ::dbsync::schema::Table>::reserve(self)?;
                <Self as ::dbsync::schema::Table>::begin_recovery(self, of, false)?;
                #(#pushes_theirs)*
                <Self as ::dbsync::schema::Table>::withdraw(self, of)
            }

            #(#accessors)*

            #(#finders)*

            #(#ranges)*
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

fn merging_of(attrs: &[syn::Attribute]) -> syn::Result<(Option<TokenStream>, Option<syn::Path>)> {
    let mut retention = None;
    let mut resolver = None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("merge")) {
        let options = attr.parse_args_with(
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
        )?;
        if options.is_empty() {
            return Err(syn::Error::new(
                attr.span(),
                "#[merge] expects newest, history, or with = path::function",
            ));
        }
        for option in options {
            match option {
                syn::Meta::Path(path) if path.is_ident("newest") || path.is_ident("history") => {
                    if retention.is_some() {
                        return Err(syn::Error::new(
                            path.span(),
                            "an entity says once what a merge keeps",
                        ));
                    }
                    retention = Some(if path.is_ident("history") {
                        generated! { ::dbsync::schema::Retention::History }
                    } else {
                        generated! { ::dbsync::schema::Retention::Newest }
                    });
                }
                syn::Meta::NameValue(named) if named.path.is_ident("with") => {
                    if resolver.is_some() {
                        return Err(syn::Error::new(
                            named.span(),
                            "an entity names one merge function",
                        ));
                    }
                    let syn::Expr::Path(function) = named.value else {
                        return Err(syn::Error::new(
                            named.span(),
                            "with expects a function path",
                        ));
                    };
                    if function.qself.is_some() {
                        return Err(syn::Error::new(
                            function.span(),
                            "with expects a function path",
                        ));
                    }
                    resolver = Some(function.path);
                }
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "#[merge] accepts newest, history, and with = path::function",
                    ));
                }
            }
        }
    }
    Ok((retention, resolver))
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

fn check_orders(fields: &[Field]) -> syn::Result<()> {
    let mut ordered: Vec<&syn::Ident> = Vec::new();
    for field in fields {
        let Sorting::Within(by) = &field.sorting else {
            continue;
        };
        let named = fields.iter().find(|other| other.name == *by);
        if !named.is_some_and(|other| matches!(other.kind, Kind::Reference(_))) {
            return Err(syn::Error::new(
                by.span(),
                format!(
                    "#[order({by})] orders children within the parent a One field names, and \
                     `{by}` is not a One field of this entity"
                ),
            ));
        }
        if ordered.contains(&by) {
            return Err(syn::Error::new(
                by.span(),
                format!("the children `{by}` groups are ordered by one position field, not two"),
            ));
        }
        ordered.push(by);
    }
    Ok(())
}

fn parent_named<'a>(by: &syn::Ident, fields: &'a [Field]) -> &'a syn::Type {
    fields
        .iter()
        .find_map(|field| match &field.kind {
            Kind::Reference(parent) if field.name == *by => Some(parent),
            _ => None,
        })
        .expect("check_orders confirmed the field an order names is a reference")
}

fn prefix_of(field: &Field, fields: &[Field]) -> Option<TokenStream> {
    match &field.sorting {
        Sorting::None => None,
        Sorting::Sorted => Some(generated! { () }),
        Sorting::Within(by) => {
            let parent = parent_named(by, fields);
            Some(generated! { ::dbsync::Id<#parent> })
        }
    }
}

fn sort_value(kind: &Kind) -> (TokenStream, TokenStream) {
    match kind {
        Kind::Scalar(ty) | Kind::NullableScalar(ty) => (generated! { #ty }, generated! { *value }),
        Kind::Enum(ty) => (
            generated! { #ty },
            generated! { ::dbsync::schema::UnitEnum::to_u8(*value) },
        ),
        _ => unreachable!("only numbers and enums are sorted"),
    }
}

fn sort_key_of(kind: &Kind, value: TokenStream) -> TokenStream {
    match kind {
        Kind::Scalar(_) => generated! { ::dbsync::schema::SortKey::sort_key(#value) },
        Kind::NullableScalar(_) => {
            generated! { ::dbsync::schema::SortKey::sort_key(::dbsync::schema::Maybe(#value)) }
        }
        Kind::Enum(_) => generated! {
            ::dbsync::schema::SortKey::sort_key(::dbsync::schema::UnitEnum::to_u8(#value))
        },
        _ => unreachable!("only numbers and enums are sorted"),
    }
}

fn check_one_parent_each(fields: &[Field]) -> syn::Result<()> {
    let mut seen: Vec<(String, &syn::Ident)> = Vec::new();

    for field in fields {
        let (Kind::Reference(parent) | Kind::NullableReference(parent)) = &field.kind else {
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
        Kind::Bytes => generated! { ::dbsync::schema::encode_bytes(&#value, &mut out); },
        Kind::NullableBytes => generated! {
            out.push(u8::from(#value.is_some()));
            if let ::core::option::Option::Some(value) = #value { ::dbsync::schema::encode_bytes(&value, &mut out); }
        },
        Kind::NullableReference(_) => generated! {
            ::dbsync::schema::encode_record(&#value.unwrap_or(::dbsync::Id::NIL), &mut out);
        },
        Kind::NullableEnum(_) => generated! {
            out.push(u8::from(#value.is_some()));
            if let ::core::option::Option::Some(value) = #value { out.push(::dbsync::schema::UnitEnum::to_u8(value)); }
        },
        _ => generated! { ::dbsync::schema::encode_record(&#value, &mut out); },
    }
}
