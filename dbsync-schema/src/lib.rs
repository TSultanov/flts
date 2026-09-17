//! Turns entity declarations into the columns that hold them.

mod entity;
mod field;
mod field_enum;
mod naming;

use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn entity(attr: TokenStream, item: TokenStream) -> TokenStream {
    match entity::expand(attr.into(), item.into()) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn field_enum(attr: TokenStream, item: TokenStream) -> TokenStream {
    match field_enum::expand(attr.into(), item.into()) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
