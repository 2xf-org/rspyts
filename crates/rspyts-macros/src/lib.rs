//! Proc macros for the rspyts application contract.

use heck::{ToKebabCase, ToLowerCamelCase, ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DataEnum, DataStruct, DeriveInput, Expr, Fields, FnArg, GenericArgument,
    Ident, ImplItem, ImplItemFn, Item, ItemConst, ItemFn, ItemImpl, ItemStatic, Lit, LitStr, Meta,
    Pat, PathArguments, ReturnType, Token, Type as SynType, TypePath, UnOp,
    ext::IdentExt,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    spanned::Spanned,
    token::Comma,
};

mod application;
mod attributes;
mod export;
mod model;
mod types;

use application::*;
use export::*;
use model::*;

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
/// Declare the one aggregate Python and WebAssembly application binding.
pub fn application(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as ModuleInput);
    expand_application(input).into()
}
