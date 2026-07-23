//! Expansion of the application-level discovery and host-module entry points.
//!
//! The generated C ABI never unwinds and pairs every owned payload with an
//! explicit free function. Native builds also expose the Python module, while
//! Wasm builds expose contract JSON for tooling and wasm-bindgen exports.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    Ident, Token,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
};

/// Optional crate identifiers linked into the generated application bridge.
pub(super) struct ModuleInput {
    /// Crates that must be linked so inventory can discover their exports.
    crates: Vec<Ident>,
}

impl Parse for ModuleInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        Ok(Self {
            crates: Punctuated::<Ident, Token![,]>::parse_terminated(input)?
                .into_iter()
                .collect(),
        })
    }
}

/// Emit native discovery/Python entry points and the Wasm contract export.
pub(super) fn expand_application(input: ModuleInput) -> TokenStream2 {
    let crates = input.crates;
    quote! {
        #(extern crate #crates as _;)*

        #[cfg(target_arch = "wasm32")]
        use ::rspyts::__private::wasm_bindgen;

        #[cfg(not(target_arch = "wasm32"))]
        #[unsafe(export_name = concat!("rspyts_discovery_v1_contract__", env!("CARGO_PKG_NAME")))]
        pub extern "C" fn rspyts_contract() -> ::rspyts::__private::DiscoveryResult {
            ::rspyts::__private::discovery_contract(|| {
                let __rspyts_manifest = ::rspyts::registry::manifest(
                    option_env!("RSPYTS_APPLICATION_NAME").unwrap_or(env!("CARGO_PKG_NAME")),
                    option_env!("RSPYTS_APPLICATION_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
                    "native",
                ).map_err(|__rspyts_error| format!("invalid rspyts registry: {__rspyts_error}"))?;
                ::rspyts::__private::serde_json::to_string(&__rspyts_manifest)
                    .map_err(|__rspyts_error| format!("rspyts manifest serialization failed: {__rspyts_error}"))
            })
        }

        #[cfg(not(target_arch = "wasm32"))]
        #[unsafe(export_name = concat!("rspyts_discovery_v1_contract_free__", env!("CARGO_PKG_NAME")))]
        pub unsafe extern "C" fn rspyts_contract_free(pointer: *mut ::std::ffi::c_char) {
            unsafe { ::rspyts::__private::discovery_free(pointer) }
        }

        #[cfg(not(target_arch = "wasm32"))]
        #[::rspyts::__private::pyo3::pymodule]
        #[pyo3(crate = "::rspyts::__private::pyo3")]
        fn native(
            __rspyts_module: &::rspyts::__private::pyo3::Bound<'_, ::rspyts::__private::pyo3::types::PyModule>,
        ) -> ::rspyts::__private::pyo3::PyResult<()> {
            ::rspyts::runtime::python::register(__rspyts_module)
        }

        #[cfg(target_arch = "wasm32")]
        #[::rspyts::__private::wasm_bindgen::prelude::wasm_bindgen(
            wasm_bindgen = ::rspyts::__private::wasm_bindgen
        )]
        pub fn rspyts_contract_json() -> String {
            let __rspyts_manifest = ::rspyts::registry::manifest(
                option_env!("RSPYTS_APPLICATION_NAME").unwrap_or(env!("CARGO_PKG_NAME")),
                option_env!("RSPYTS_APPLICATION_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
                "native",
            ).expect("invalid rspyts registry");
            ::rspyts::__private::serde_json::to_string(&__rspyts_manifest)
                .expect("rspyts manifest serialization failed")
        }
    }
}
