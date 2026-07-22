//! Proc macros for the rspyts application contract.

use proc_macro::TokenStream;
use proc_macro2::Span;
use syn::{DeriveInput, Item, parse_macro_input};

mod application;
mod attributes;
mod export;
mod model;
mod types;

use application::{ModuleInput, expand_application};
use export::expand_export;
use model::{expand_error, expand_type};

#[proc_macro_derive(Model, attributes(rspyts, serde))]
/// Derive the host-neutral contract for a serializable Rust model.
pub fn derive_model(input: TokenStream) -> TokenStream {
    expand_type(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_derive(Error, attributes(rspyts, serde, error, source, from, backtrace))]
/// Derive stable host error information for a Rust error.
pub fn derive_error(input: TokenStream) -> TokenStream {
    expand_error(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_attribute]
/// Export a public function, constant, static, or inherent implementation.
pub fn export(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(Span::call_site(), "`#[rspyts::export]` takes no mode")
            .into_compile_error()
            .into();
    }
    let item = parse_macro_input!(input as Item);
    expand_export(item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro]
/// Declare an application bridge for all linked Rust packages.
pub fn application(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as ModuleInput);
    expand_application(input).into()
}
