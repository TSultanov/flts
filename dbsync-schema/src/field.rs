//! What a declared field means: the column that holds it and the accessors that reach it.

use proc_macro2::TokenStream;
use quote::ToTokens;
use syn::spanned::Spanned;
use syn::{GenericArgument, PathArguments, Type, TypePath};

macro_rules! generated {
    ($($tokens:tt)*) => { quote::quote_spanned!(proc_macro2::Span::mixed_site()=> $($tokens)*) };
}

const SCALARS: &[&str] = &[
    "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64", "bool",
];

const FLOATS: &[&str] = &["f32", "f64"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Indexing {
    None,
    Shared,
}

pub enum Kind {
    Text,
    NullableText,
    Reference(Type),
    Many(Type),
    Scalar(Type),
    NullableScalar(Type),
    Enum(Type),
}

pub struct Field {
    pub name: syn::Ident,
    pub kind: Kind,
    pub indexing: Indexing,
    pub through: Option<Type>,
    pub docs: Vec<syn::Attribute>,
}

impl Field {
    pub fn parse(field: &syn::Field) -> syn::Result<Self> {
        let name = field
            .ident
            .clone()
            .ok_or_else(|| syn::Error::new(field.span(), "an entity needs named fields"))?;
        let kind = Kind::parse(&field.ty)?;
        let (indexing, through) = asked_of(field, &kind)?;
        let docs = field
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("doc"))
            .cloned()
            .collect();
        Ok(Self {
            name,
            kind,
            indexing,
            through,
            docs,
        })
    }

    pub fn is_stored(&self) -> bool {
        !matches!(self.kind, Kind::Many(_))
    }
}

fn asked_of(field: &syn::Field, kind: &Kind) -> syn::Result<(Indexing, Option<Type>)> {
    let mut asked: Option<(&syn::Attribute, Indexing)> = None;
    let mut crossed: Option<Type> = None;

    for attr in &field.attrs {
        if attr.path().is_ident("doc") {
            continue;
        }
        if attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr") {
            return Err(syn::Error::new(
                attr.span(),
                "a column outlives the build that named it, so a field cannot be compiled out",
            ));
        }
        if attr.path().is_ident("through") {
            if !matches!(kind, Kind::Many(_)) {
                return Err(syn::Error::new(
                    attr.span(),
                    "only a Many crosses a table to reach the rows on its far side; a field that \
                     holds a value is already where its value is",
                ));
            }
            if crossed.is_some() {
                return Err(syn::Error::new(
                    attr.span(),
                    "this field crosses one table already; a second would be a different question",
                ));
            }
            crossed = Some(one_table(attr)?);
            continue;
        }
        if !attr.path().is_ident("index") {
            let written = attr.path().to_token_stream().to_string().replace(' ', "");
            return Err(syn::Error::new(
                attr.span(),
                format!(
                    "`{written}` is not an attribute a field of an entity takes, and this struct \
                     becomes a tag, so it would be dropped without a word; #[index] and #[through(..)] are the only ones"
                ),
            ));
        }
        if asked.is_some() {
            return Err(syn::Error::new(
                attr.span(),
                "this field is indexed already; one #[index] says all there is to say",
            ));
        }
        asked = Some((attr, indexing_of(attr)?));
    }

    let Some((attr, indexing)) = asked else {
        return Ok((Indexing::None, crossed));
    };
    match kind {
        Kind::Reference(_) => Err(syn::Error::new(
            attr.span(),
            "a parent is indexed already, which is how a Many finds its children",
        )),
        Kind::Many(_) => Err(syn::Error::new(
            attr.span(),
            "a Many is the children that name this row, not a column of its own, so there is \
             nothing to index",
        )),
        Kind::Scalar(ty) | Kind::NullableScalar(ty) if is_float(ty) => Err(syn::Error::new(
            attr.span(),
            "an index answers by equality, and a float is not equal to itself when it is NaN, \
             so a float field cannot be indexed",
        )),
        _ => Ok((indexing, crossed)),
    }
}

fn one_table(attr: &syn::Attribute) -> syn::Result<Type> {
    let named =
        attr.parse_args_with(syn::punctuated::Punctuated::<Type, syn::Token![,]>::parse_terminated);
    let mut named = match named {
        Ok(named) => named.into_iter(),
        Err(_) => {
            return Err(syn::Error::new(
                attr.span(),
                "#[through] names the one table the hop goes over, as #[through(Authorship)]",
            ));
        }
    };
    match (named.next(), named.next()) {
        (Some(only), None) => Ok(only),
        _ => Err(syn::Error::new(
            attr.span(),
            "#[through] names the one table the hop goes over, as #[through(Authorship)]",
        )),
    }
}

fn indexing_of(attr: &syn::Attribute) -> syn::Result<Indexing> {
    match &attr.meta {
        syn::Meta::Path(_) => Ok(Indexing::Shared),
        _ => Err(syn::Error::new(
            attr.span(),
            "only entity UUIDs are unique; use #[index] for a nonunique lookup",
        )),
    }
}

fn is_float(ty: &Type) -> bool {
    named(ty).is_some_and(|name| FLOATS.contains(&name.as_str()))
}

impl Kind {
    fn parse(ty: &Type) -> syn::Result<Self> {
        if is_string(ty) {
            return Ok(Kind::Text);
        }
        unsized_text(ty)?;
        renamed(ty)?;
        if let Some(inner) = wrapped("Option", ty) {
            return Self::nullable(inner, ty);
        }
        if let Some(parent) = wrapped("One", ty) {
            return Ok(Kind::Reference(parent.clone()));
        }
        if let Some(child) = wrapped("Many", ty) {
            return Ok(Kind::Many(child.clone()));
        }
        if is_scalar(ty) {
            return Ok(Kind::Scalar(ty.clone()));
        }
        Ok(Kind::Enum(ty.clone()))
    }

    fn nullable(inner: &Type, whole: &Type) -> syn::Result<Self> {
        if is_string(inner) {
            return Ok(Kind::NullableText);
        }
        unsized_text(inner)?;
        renamed(inner)?;
        if is_scalar(inner) {
            return Ok(Kind::NullableScalar(inner.clone()));
        }
        if wrapped("One", inner).is_some() {
            return Err(syn::Error::new(
                whole.span(),
                "an optional parent is not supported yet; declare it One<..> for now",
            ));
        }
        if wrapped("Many", inner).is_some() {
            return Err(syn::Error::new(
                whole.span(),
                "Many is already the absence of children when there are none, so it cannot be optional",
            ));
        }
        Err(syn::Error::new(
            whole.span(),
            "an optional enum is not supported yet; declare it without Option for now",
        ))
    }

    pub fn column(&self, child: &syn::Ident, indexing: Indexing) -> TokenStream {
        let bare = self.bare_column(child);
        match indexing {
            Indexing::None => bare,
            _ => {
                generated! { ::dbsync::schema::Indexed<#bare> }
            }
        }
    }

    fn bare_column(&self, child: &syn::Ident) -> TokenStream {
        match self {
            Kind::Text => generated! { ::dbsync::schema::TextColumn },
            Kind::NullableText => generated! { ::dbsync::schema::NullableTextColumn },
            Kind::Reference(parent) => {
                generated! { ::dbsync::schema::RefColumn<#parent, #child> }
            }
            Kind::Scalar(ty) => generated! { ::dbsync::Arena<#ty> },
            Kind::NullableScalar(ty) => generated! { ::dbsync::schema::NullableColumn<#ty> },
            Kind::Enum(ty) => generated! { ::dbsync::schema::EnumColumn<#ty> },
            Kind::Many(_) => unreachable!("a Many field has no column"),
        }
    }

    pub fn pushed(&self) -> TokenStream {
        match self {
            Kind::Text => generated! { &str },
            Kind::NullableText => generated! { ::core::option::Option<&str> },
            Kind::Reference(parent) => generated! { ::dbsync::Id<#parent> },
            Kind::Scalar(ty) => generated! { #ty },
            Kind::NullableScalar(ty) => generated! { ::core::option::Option<#ty> },
            Kind::Enum(ty) => generated! { #ty },
            Kind::Many(_) => unreachable!("a Many field is never pushed"),
        }
    }

    pub fn read(&self, held: &TokenStream) -> TokenStream {
        match self {
            Kind::Text => generated! { ::std::borrow::Cow<#held, str> },
            Kind::NullableText => {
                generated! { ::core::option::Option<::std::borrow::Cow<#held, str>> }
            }
            Kind::Reference(parent) => generated! { ::dbsync::Id<#parent> },
            Kind::Scalar(ty) => generated! { #ty },
            Kind::NullableScalar(ty) => generated! { ::core::option::Option<#ty> },
            Kind::Enum(ty) => generated! { #ty },
            Kind::Many(_) => unreachable!("a Many field is not read from a column"),
        }
    }

    pub fn get(&self, table: &TokenStream, name: &syn::Ident, indexing: Indexing) -> TokenStream {
        let held = match indexing {
            Indexing::None => generated! { #table.#name },
            _ => generated! { #table.#name.column() },
        };
        match self {
            Kind::Scalar(_) => generated! { #held.row(row.index()).copied() },
            _ => generated! { #held.get(row) },
        }
    }

    pub fn owned(&self) -> TokenStream {
        match self {
            Kind::Text => generated! { .map(|held| held.into_owned()) },
            Kind::NullableText => {
                generated! { .map(|held| held.map(|text| text.into_owned())) }
            }
            _ => generated! {},
        }
    }

    pub fn lent(&self, name: &syn::Ident) -> TokenStream {
        match self {
            Kind::Text => generated! { &#name },
            Kind::NullableText => generated! { #name.as_deref() },
            _ => generated! { #name },
        }
    }
}

pub fn set_of(ty: &Type) -> Option<&Type> {
    wrapped("Set", ty)
}

fn is_string(ty: &Type) -> bool {
    named(ty).is_some_and(|name| name == "String")
}

fn unsized_text(ty: &Type) -> syn::Result<()> {
    if named(ty).is_some_and(|name| name == "str") {
        return Err(syn::Error::new(
            ty.span(),
            "a field is stored by value, so a string field is declared String",
        ));
    }
    if matches!(ty, Type::Reference(_)) {
        return Err(syn::Error::new(
            ty.span(),
            "a field is stored by value and outlives any borrow, so it cannot be a reference",
        ));
    }
    Ok(())
}

fn renamed(ty: &Type) -> syn::Result<()> {
    if wrapped("Ref", ty).is_some() {
        return Err(syn::Error::new(
            ty.span(),
            "a parent is declared One<..>, the one side of the one-to-many a Many walks",
        ));
    }
    Ok(())
}

fn is_scalar(ty: &Type) -> bool {
    named(ty).is_some_and(|name| SCALARS.contains(&name.as_str()))
}

fn named(ty: &Type) -> Option<String> {
    let Type::Path(TypePath { qself: None, path }) = ty else {
        return None;
    };
    let last = path.segments.last()?;
    matches!(last.arguments, PathArguments::None).then(|| last.ident.to_string())
}

fn wrapped<'a>(outer: &str, ty: &'a Type) -> Option<&'a Type> {
    let Type::Path(TypePath { qself: None, path }) = ty else {
        return None;
    };
    let last = path.segments.last()?;
    if last.ident != outer {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    match args.args.first()? {
        GenericArgument::Type(inner) => Some(inner),
        _ => None,
    }
}

#[cfg(test)]
#[path = "field_test.rs"]
mod field_test;
