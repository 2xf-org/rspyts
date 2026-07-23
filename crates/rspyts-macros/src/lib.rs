//! Procedural macro implementation for rspyts contracts.
//!
//! This crate is an implementation detail of `rspyts`; application code
//! should use the macros re-exported by that crate. Expansion is organized by
//! declaration kind: model and error derives produce registry metadata,
//! exports produce host wrappers, and the application macro emits the package
//! discovery entry points.

#![deny(missing_docs, rustdoc::broken_intra_doc_links)]
#![forbid(unsafe_code)]

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

/// Derive the host-neutral contract for a serializable Rust model.
///
/// The derive supports named structs, string enums, internally tagged enums,
/// and transparent one-field aliases. Unsupported or lossy Serde attributes
/// are rejected during macro expansion.
#[proc_macro_derive(Model, attributes(rspyts, serde))]
pub fn derive_model(input: TokenStream) -> TokenStream {
    expand_type(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Derive stable host error information for a Rust error.
///
/// Each variant receives a stable snake-case code unless overridden with
/// `#[serde(rename = "...")]`.
#[proc_macro_derive(Error, attributes(rspyts, serde, error, source, from, backtrace))]
pub fn derive_error(input: TokenStream) -> TokenStream {
    expand_error(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Export a public function, constant, static, or inherent implementation.
///
/// Function and method wrappers are emitted for Python on native targets and
/// for JavaScript on `wasm32`. Annotated byte and numeric-buffer parameters
/// use direct host ABIs rather than Serde conversion.
#[proc_macro_attribute]
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

/// Declare an application bridge for all linked Rust packages.
///
/// Optional crate identifiers force additional contract crates to be linked
/// into the final bridge so their inventory registrations are discoverable.
#[proc_macro]
pub fn application(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as ModuleInput);
    expand_application(input).into()
}
