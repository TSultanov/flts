//! Turns entity declarations into the columns that hold them.

mod field_enum;
mod naming;

use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn field_enum(attr: TokenStream, item: TokenStream) -> TokenStream {
    match field_enum::expand(attr.into(), item.into()) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
