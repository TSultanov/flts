//! What a declared field means: the column that holds it and the accessors that reach it.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{GenericArgument, PathArguments, Type, TypePath};

const SCALARS: &[&str] = &[
    "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64", "bool",
];

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
}

impl Field {
    pub fn parse(field: &syn::Field) -> syn::Result<Self> {
        let name = field
            .ident
            .clone()
            .ok_or_else(|| syn::Error::new(field.span(), "an entity needs named fields"))?;
        Ok(Self {
            name,
            kind: Kind::parse(&field.ty)?,
        })
    }

    pub fn is_stored(&self) -> bool {
        !matches!(self.kind, Kind::Many(_))
    }
}

impl Kind {
    fn parse(ty: &Type) -> syn::Result<Self> {
        if is_str(ty) {
            return Ok(Kind::Text);
        }
        if let Some(inner) = wrapped("Option", ty) {
            return Self::nullable(inner, ty);
        }
        if let Some(parent) = wrapped("Ref", ty) {
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
        if is_str(inner) {
            return Ok(Kind::NullableText);
        }
        if is_scalar(inner) {
            return Ok(Kind::NullableScalar(inner.clone()));
        }
        if wrapped("Ref", inner).is_some() {
            return Err(syn::Error::new(
                whole.span(),
                "an optional reference is not supported yet; declare it Ref<..> for now",
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

    pub fn column(&self, child: &syn::Ident) -> TokenStream {
        match self {
            Kind::Text => quote! { ::dbsync::schema::TextColumn },
            Kind::NullableText => quote! { ::dbsync::schema::NullableTextColumn },
            Kind::Reference(parent) => {
                quote! { ::dbsync::schema::RefColumn<#parent, #child> }
            }
            Kind::Scalar(ty) => quote! { ::dbsync::Arena<#ty> },
            Kind::NullableScalar(ty) => quote! { ::dbsync::schema::NullableColumn<#ty> },
            Kind::Enum(ty) => quote! { ::dbsync::schema::EnumColumn<#ty> },
            Kind::Many(_) => unreachable!("a Many field has no column"),
        }
    }

    pub fn pushed(&self) -> TokenStream {
        match self {
            Kind::Text => quote! { &str },
            Kind::NullableText => quote! { ::core::option::Option<&str> },
            Kind::Reference(parent) => quote! { ::dbsync::Id<#parent> },
            Kind::Scalar(ty) => quote! { #ty },
            Kind::NullableScalar(ty) => quote! { ::core::option::Option<#ty> },
            Kind::Enum(ty) => quote! { #ty },
            Kind::Many(_) => unreachable!("a Many field is never pushed"),
        }
    }

    pub fn read(&self) -> TokenStream {
        match self {
            Kind::Text => quote! { ::std::borrow::Cow<'_, str> },
            Kind::NullableText => {
                quote! { ::core::option::Option<::std::borrow::Cow<'_, str>> }
            }
            Kind::Reference(parent) => quote! { ::dbsync::Id<#parent> },
            Kind::Scalar(ty) => quote! { #ty },
            Kind::NullableScalar(ty) => quote! { ::core::option::Option<#ty> },
            Kind::Enum(ty) => quote! { #ty },
            Kind::Many(_) => unreachable!("a Many field is not read from a column"),
        }
    }

    pub fn get(&self, name: &syn::Ident) -> TokenStream {
        match self {
            Kind::Scalar(_) => quote! { self.#name.row(row.index()).copied() },
            _ => quote! { self.#name.get(row) },
        }
    }

    pub fn set(&self, name: &syn::Ident) -> TokenStream {
        match self {
            Kind::Scalar(_) => quote! { self.#name.set(row.index(), value) },
            _ => quote! { self.#name.set(row, value) },
        }
    }
}

fn is_str(ty: &Type) -> bool {
    named(ty).is_some_and(|name| name == "str")
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
