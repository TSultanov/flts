//! A unit enum that a column can hold, checked here so nothing downstream has to.

use proc_macro2::{Literal, TokenStream};
use quote::quote;
use syn::spanned::Spanned;
use syn::{Expr, ExprLit, Fields, ItemEnum, Lit, Meta};

use crate::naming::snake_case;

pub fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    if !attr.is_empty() {
        return Err(syn::Error::new(
            attr.span(),
            "#[dbsync::field_enum] takes no arguments",
        ));
    }

    let input: ItemEnum = syn::parse2(item)?;
    check_plain(&input)?;
    let codes = discriminants(&input)?;

    let name = &input.ident;
    let label = snake_case(&name.to_string());
    let missing = added_derives(&input);
    let arms = codes.iter().map(|(variant, code)| {
        let code = Literal::u8_unsuffixed(*code);
        quote! { #code => ::core::result::Result::Ok(Self::#variant) }
    });

    let attrs = &input.attrs;
    let vis = &input.vis;
    let variants = codes.iter().map(|(variant, code)| {
        let code = Literal::u8_unsuffixed(*code);
        quote! { #variant = #code }
    });

    Ok(quote! {
        #(#attrs)*
        #missing
        #[repr(u8)]
        #vis enum #name {
            #(#variants,)*
        }

        impl ::dbsync::schema::UnitEnum for #name {
            const NAME: &'static str = #label;

            fn to_u8(self) -> u8 {
                self as u8
            }

            fn from_u8(code: u8) -> ::dbsync::Result<Self> {
                match code {
                    #(#arms,)*
                    other => ::core::result::Result::Err(::dbsync::Error::unsupported(
                        ::std::format!("unknown {} {}", #label, other),
                    )),
                }
            }
        }
    })
}

fn check_plain(input: &ItemEnum) -> syn::Result<()> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.generics.span(),
            "a column holds one byte per row, so a field enum cannot be generic",
        ));
    }
    if input.variants.is_empty() {
        return Err(syn::Error::new(
            input.ident.span(),
            "a field enum needs at least one variant for a column to hold",
        ));
    }
    if let Some(repr) = input.attrs.iter().find(|attr| attr.path().is_ident("repr")) {
        return Err(syn::Error::new(
            repr.span(),
            "#[dbsync::field_enum] writes the repr itself, as u8",
        ));
    }

    for variant in &input.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new(
                variant.span(),
                "a field enum stores a discriminant, so its variants cannot carry data",
            ));
        }
    }
    Ok(())
}

fn discriminants(input: &ItemEnum) -> syn::Result<Vec<(syn::Ident, u8)>> {
    let mut codes: Vec<(syn::Ident, u8)> = Vec::new();
    let mut next: u16 = 0;

    for variant in &input.variants {
        let code = match &variant.discriminant {
            Some((_, expr)) => literal_u8(expr)?,
            None => u8::try_from(next).map_err(|_| {
                syn::Error::new(
                    variant.span(),
                    "a column holds one byte per row, so discriminants must stay under 256",
                )
            })?,
        };

        if let Some((taken, _)) = codes.iter().find(|(_, seen)| *seen == code) {
            return Err(syn::Error::new(
                variant.span(),
                format!("discriminant {code} is already taken by `{taken}`"),
            ));
        }

        codes.push((variant.ident.clone(), code));
        next = u16::from(code) + 1;
    }

    Ok(codes)
}

fn literal_u8(expr: &Expr) -> syn::Result<u8> {
    let Expr::Lit(ExprLit {
        lit: Lit::Int(int), ..
    }) = expr
    else {
        return Err(syn::Error::new(
            expr.span(),
            "a discriminant must be a plain integer literal",
        ));
    };
    int.base10_parse::<u8>().map_err(|_| {
        syn::Error::new(
            expr.span(),
            "a column holds one byte per row, so discriminants must stay under 256",
        )
    })
}

fn added_derives(input: &ItemEnum) -> TokenStream {
    let already = |want: &str| {
        input.attrs.iter().any(|attr| {
            attr.path().is_ident("derive")
                && attr
                    .parse_args_with(
                        syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
                    )
                    .is_ok_and(|list| list.iter().any(|meta| meta.path().is_ident(want)))
        })
    };

    let clone = (!already("Clone")).then(|| quote! { Clone });
    let copy = (!already("Copy")).then(|| quote! { Copy });

    match (clone, copy) {
        (None, None) => quote! {},
        (clone, copy) => {
            let wanted = clone.into_iter().chain(copy);
            quote! { #[derive(#(#wanted),*)] }
        }
    }
}

#[cfg(test)]
#[path = "field_enum_test.rs"]
mod field_enum_test;
