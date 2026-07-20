//! Build one Rust application API for Python and TypeScript.

pub mod bridge;
pub mod ir;
pub mod registry;
pub mod runtime;
mod types;

pub use rspyts_macros::{Error, Model, application, export};
pub use types::ContractType;

mod discovery {
    use std::ffi::{CString, c_char};
    use std::mem;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::ptr;

    pub const SUCCESS: u32 = 0;
    pub const ERROR: u32 = 1;
    pub const PANIC: u32 = 2;

    #[repr(C)]
    pub struct DiscoveryResult {
        pub status: u32,
        pub payload: *mut c_char,
    }

    pub fn contract(build: impl FnOnce() -> Result<String, String>) -> DiscoveryResult {
        match catch_unwind(AssertUnwindSafe(build)) {
            Ok(Ok(payload)) => owned(SUCCESS, payload),
            Ok(Err(error)) => owned(ERROR, error),
            Err(panic) => {
                mem::forget(panic);
                DiscoveryResult {
                    status: PANIC,
                    payload: ptr::null_mut(),
                }
            }
        }
    }

    fn owned(status: u32, payload: String) -> DiscoveryResult {
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
