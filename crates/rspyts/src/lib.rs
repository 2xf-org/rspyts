//! Define one Rust API and expose it as native Python and WebAssembly-backed
//! TypeScript packages.
//!
//! The crate deliberately separates four responsibilities:
//!
//! - [`ir`] is the serialized, host-neutral contract.
//! - [`registry`] discovers and validates linked exports.
//! - [`bridge`] converts values at native host boundaries.
//! - [`runtime`] registers generated call targets and typed errors.
//!
//! Application authors normally interact with the [`Model`] and [`Error`]
//! derives, the [`export`] attribute, and the [`application`] macro. The CLI
//! consumes the discovery ABI emitted by those macros; its implementation
//! details are re-exported through [`__private`] solely for generated code.

#![deny(missing_docs, rustdoc::broken_intra_doc_links)]
#![forbid(unsafe_op_in_unsafe_fn)]

pub mod bridge;
pub mod ir;
pub mod registry;
pub mod runtime;
mod types;

pub use rspyts_macros::{Error, Model, application, export};
pub use types::ContractType;

mod discovery {
    //! Panic-safe C ABI used by the build orchestrator during discovery.

    use std::ffi::{CString, c_char};
    use std::mem;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::ptr;

    /// Discovery completed and `payload` contains contract JSON.
    pub const SUCCESS: u32 = 0;
    /// Discovery failed while constructing or validating the contract.
    pub const ERROR: u32 = 1;
    /// Discovery panicked before it could return an error payload.
    pub const PANIC: u32 = 2;

    /// Owned result transferred across the discovery dynamic-library boundary.
    ///
    /// `payload` is either null or allocated by [`CString::into_raw`]. The
    /// caller must release a non-null pointer with [`free`].
    #[repr(C)]
    pub struct DiscoveryResult {
        /// One of [`SUCCESS`], [`ERROR`], or [`PANIC`].
        pub status: u32,
        /// UTF-8 contract JSON or an error message, depending on `status`.
        pub payload: *mut c_char,
    }

    /// Run contract construction without allowing Rust unwinding across FFI.
    pub fn contract(build: impl FnOnce() -> Result<String, String>) -> DiscoveryResult {
        match catch_unwind(AssertUnwindSafe(build)) {
            Ok(Ok(payload)) => owned(SUCCESS, &payload),
            Ok(Err(error)) => owned(ERROR, &error),
            Err(panic) => {
                mem::forget(panic);
                DiscoveryResult {
                    status: PANIC,
                    payload: ptr::null_mut(),
                }
            }
        }
    }

    fn owned(status: u32, payload: &str) -> DiscoveryResult {
        let payload = payload.replace('\0', "\\0");
        // SAFETY: the replacement removes all interior NUL bytes.
        let payload = unsafe { CString::from_vec_unchecked(payload.into_bytes()) }.into_raw();
        DiscoveryResult { status, payload }
    }

    /// # Safety
    ///
    /// `pointer` must be null or a live pointer returned by [`contract`].
    pub unsafe fn free(pointer: *mut c_char) {
        if !pointer.is_null() {
            // SAFETY: the caller follows this function's contract.
            drop(unsafe { CString::from_raw(pointer) });
        }
    }
}

/// Implementation dependencies re-exported for macro-generated code.
///
/// This module is not a stable user-facing API.
#[doc(hidden)]
pub mod __private {
    pub use crate::discovery::{
        DiscoveryResult, ERROR as DISCOVERY_ERROR, PANIC as DISCOVERY_PANIC,
        SUCCESS as DISCOVERY_SUCCESS, contract as discovery_contract, free as discovery_free,
    };
    pub use inventory;
    pub use serde;
    pub use serde_json;

    #[cfg(not(target_arch = "wasm32"))]
    pub use pyo3;
    #[cfg(target_arch = "wasm32")]
    pub use wasm_bindgen;
}
