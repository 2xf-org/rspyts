//! **rspyts** — define it in Rust, call it in Python and TypeScript.
//!
//! Annotate types, functions, and impl blocks with [`#[bridge]`](macro@bridge),
//! place [`export!()`](macro@export) once in your cdylib, and run
//! `rspyts generate`: you get pydantic models, TypeScript types, JSON
//! Schema, and fully typed wrapper functions for both languages — all
//! projected from the single Rust definition, all crossing one small C ABI.
//!
//! ```ignore
//! use rspyts::{bridge, Buf};
//!
//! #[bridge]
//! /// Options controlling value processing.
//! pub struct QueryOptions {
//!     /// Minimum value to include.
//!     pub minimum_value: f64,
//!     pub tolerance: Option<f64>,
//! }
//!
//! #[bridge(error)]
//! pub enum QueryError {
//!     InvalidBatchSize,
//!     BatchTooLarge { max: u32 },
//! }
//!
//! #[bridge]
//! /// Process a buffer of numeric values.
//! pub fn process_values(
//!     values: &[f64],
//!     batch_size: u32,
//!     options: &QueryOptions,
//! ) -> Result<Buf<f64>, QueryError> {
//!     // …
//! }
//!
//! rspyts::export!();
//! ```
//!
//! The normative references live in the repository:
//! `docs/design/type-system.md` (what can cross), `docs/design/abi.md`
//! (how it crosses), and `docs/design/codegen.md` (what gets generated).

pub use rspyts_core::{BridgeErr, BridgeError, Bridged, Buf, Bytes};
pub use rspyts_macros::bridge;

/// Export the module-level rspyts symbols (`rspyts_abi_version`,
/// `rspyts_manifest`, `rspyts_contract_fingerprint`, `rspyts_alloc`, and
/// `rspyts_free`). Invoke exactly once at the root of every bridged cdylib
/// crate.
#[macro_export]
macro_rules! export {
    () => {
        #[unsafe(no_mangle)]
        pub extern "C" fn rspyts_abi_version() -> u32 {
            $crate::__private::ABI_VERSION
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn rspyts_manifest() -> *mut u8 {
            $crate::__private::shim::run(|| {
                ::core::result::Result::Ok($crate::__private::registry::build_manifest(
                    env!("CARGO_PKG_NAME"),
                    env!("CARGO_PKG_VERSION"),
                ))
            })
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn rspyts_contract_fingerprint() -> *mut u8 {
            $crate::__private::shim::run(|| {
                let manifest = $crate::__private::registry::build_manifest(
                    env!("CARGO_PKG_NAME"),
                    env!("CARGO_PKG_VERSION"),
                );
                ::core::result::Result::Ok($crate::__private::manifest_fingerprint(&manifest))
            })
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn rspyts_alloc(len: usize) -> *mut u8 {
            $crate::__private::envelope::alloc(len)
        }

        /// # Safety
        /// Callers must pass a pointer and length pair previously produced
        /// by `rspyts_alloc` or returned as an envelope.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn rspyts_free(ptr: *mut u8, len: usize) {
            $crate::__private::envelope::dealloc(ptr, len)
        }
    };
}

/// Implementation details referenced by macro-generated code. Semver-exempt;
/// never use directly.
#[doc(hidden)]
pub mod __private {
    pub use inventory;
    pub use rspyts_core::ABI_VERSION;
    pub use rspyts_core::bridged::{Bridged, BufElem, SliceElem};
    pub use rspyts_core::envelope;
    pub use rspyts_core::handles::Slab;
    pub use rspyts_core::ir;
    pub use rspyts_core::manifest_fingerprint;
    pub use rspyts_core::registry;
    pub use rspyts_core::shim;
    pub use rspyts_core::wire;
    pub use serde;
    pub use serde_json;
}
