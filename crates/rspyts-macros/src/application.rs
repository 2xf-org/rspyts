use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    Ident, Token,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
};

pub(super) struct ModuleInput {
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
                    env!("CARGO_PKG_NAME"),
                    env!("CARGO_PKG_VERSION"),
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
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
                "native",
            ).expect("invalid rspyts registry");
            ::rspyts::__private::serde_json::to_string(&__rspyts_manifest)
                .expect("rspyts manifest serialization failed")
        }
    }
}
