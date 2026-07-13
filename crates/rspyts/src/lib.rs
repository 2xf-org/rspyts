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
//! /// Parameters controlling the analysis pass.
//! pub struct AnalysisParams {
//!     /// Minimum event duration, in seconds.
//!     pub min_duration_s: f64,
//!     pub threshold: Option<f64>,
//! }
//!
//! #[bridge(error)]
//! pub enum AnalysisError {
//!     InvalidSampleRate,
//!     WindowTooLarge { max: u32 },
//! }
//!
//! #[bridge]
//! /// Analyze a signal buffer.
//! pub fn analyze_signal(
//!     samples: &[f64],
//!     sample_rate: u32,
//!     params: &AnalysisParams,
//! ) -> Result<Buf<f64>, AnalysisError> {
//!     // …
//! }
//!
//! rspyts::export!();
//! ```
//!
//! The normative references live in the repository:
//! `docs/design/type-system.md` (what can cross), `docs/design/abi.md`
//! (how it crosses), and `docs/design/codegen.md` (what gets generated).

pub use rspyts_core::{BridgeErr, BridgeError, Bridged, Buf, Json};
pub use rspyts_macros::bridge;

/// Export the module-level rspyts symbols (`rspyts_abi_version`,
/// `rspyts_manifest`, `rspyts_alloc`, `rspyts_free`). Invoke exactly once
/// at the root of every bridged cdylib crate.
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
    pub use rspyts_core::registry;
    pub use rspyts_core::shim;
    pub use serde;
    pub use serde_json;
}
