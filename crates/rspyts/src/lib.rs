//! Generate Python package source and TypeScript packages from one Rust API.

pub mod backend;
pub mod codec;
pub mod ir;
pub mod registry;
pub mod runtime;
mod types;
mod wire;

pub use rspyts_macros::{Error, Type, export, module};
pub use types::ContractType;

mod discovery {
    use std::ffi::{CString, c_char};
    use std::mem;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::ptr;

    /// The discovery payload is a serialized contract manifest.
    pub const SUCCESS: u32 = 0;
    /// The discovery payload is a UTF-8 diagnostic from an ordinary contract error.
    pub const ERROR: u32 = 1;
    /// Contract discovery panicked. The payload is null because allocating a diagnostic
    /// after catching a panic could itself unwind across the FFI boundary.
    pub const PANIC: u32 = 2;
    /// The declared module can produce a native Python extension.
    pub const PYTHON: u32 = 1 << 0;
    /// The declared module can produce a WebAssembly TypeScript runtime.
    pub const TYPESCRIPT: u32 = 1 << 1;

    /// An owned result returned across the native discovery ABI.
    ///
    /// A non-null `payload` returned by
    /// `rspyts_discovery_v1_contract__<package>` must be released exactly once with
    /// `rspyts_discovery_v1_contract_free__<package>`, regardless of `status`.
    #[repr(C)]
    pub struct DiscoveryResult {
        pub status: u32,
        pub capabilities: u32,
        pub payload: *mut c_char,
    }

    /// Runs contract discovery behind an unwind boundary and returns an owned payload.
    pub fn contract<F>(capabilities: u32, build: F) -> DiscoveryResult
    where
        F: FnOnce() -> Result<String, String>,
    {
        match catch_unwind(AssertUnwindSafe(|| match build() {
            Ok(payload) => owned(SUCCESS, capabilities, payload),
            Err(error) => owned(ERROR, capabilities, error),
        })) {
            Ok(result) => result,
            Err(panic) => {
                // A user registration can panic with a payload whose destructor also
                // panics. Dropping that payload here would start a second unwind outside
                // this catch and cross the generated `extern "C"` boundary.
                mem::forget(panic);
                DiscoveryResult {
                    status: PANIC,
                    capabilities,
                    payload: ptr::null_mut(),
                }
            }
        }
    }

    fn owned(status: u32, capabilities: u32, payload: String) -> DiscoveryResult {
        // JSON manifests never contain an interior NUL. Diagnostics can contain authored
        // identifiers, so make those representable as a C string without introducing a
        // second fallible/panicking conversion inside the ABI boundary.
        let payload = payload.replace('\0', "\\0");
        // SAFETY: the replacement above guarantees there are no interior NUL bytes.
        let payload = unsafe { CString::from_vec_unchecked(payload.into_bytes()) }.into_raw();
        DiscoveryResult {
            status,
            capabilities,
            payload,
        }
    }

    /// Releases a payload returned by [`contract`]. Null is accepted as a no-op.
    ///
    /// # Safety
    ///
    /// A non-null pointer must have been returned by [`contract`] and must not have
    /// already been released.
    pub unsafe fn free(pointer: *mut c_char) {
        if !pointer.is_null() {
            // SAFETY: upheld by the caller as documented above.
            drop(unsafe { CString::from_raw(pointer) });
        }
    }

    #[cfg(test)]
    mod tests {
        use std::ffi::CStr;
        use std::panic::panic_any;

        use super::*;

        fn payload(result: &DiscoveryResult) -> &str {
            assert!(!result.payload.is_null());
            // SAFETY: test results are live owned C strings until each test frees them.
            unsafe { CStr::from_ptr(result.payload) }.to_str().unwrap()
        }

        #[test]
        fn success_returns_an_owned_manifest_payload() {
            let result = contract(PYTHON | TYPESCRIPT, || Ok(r#"{"irVersion":6}"#.to_owned()));
            assert_eq!(result.status, SUCCESS);
            assert_eq!(result.capabilities, PYTHON | TYPESCRIPT);
            assert_eq!(payload(&result), r#"{"irVersion":6}"#);
            // SAFETY: the pointer came from `contract` and is released exactly once.
            unsafe { free(result.payload) };
        }

        #[test]
        fn ordinary_errors_return_an_owned_diagnostic_payload() {
            let result = contract(PYTHON, || {
                Err("duplicate type identity `fixture::Item`".to_owned())
            });
            assert_eq!(result.status, ERROR);
            assert_eq!(result.capabilities, PYTHON);
            assert_eq!(payload(&result), "duplicate type identity `fixture::Item`");
            // SAFETY: the pointer came from `contract` and is released exactly once.
            unsafe { free(result.payload) };
        }

        #[test]
        fn panics_are_caught_and_null_free_is_safe() {
            let result = contract(TYPESCRIPT, || -> Result<String, String> {
                panic!("broken registry")
            });
            assert_eq!(result.status, PANIC);
            assert_eq!(result.capabilities, TYPESCRIPT);
            assert!(result.payload.is_null());
            // SAFETY: null is explicitly accepted by `free`.
            unsafe { free(result.payload) };
            // SAFETY: a direct null free is also explicitly accepted.
            unsafe { free(ptr::null_mut()) };
        }

        #[test]
        fn diagnostics_with_nuls_remain_owned_c_strings() {
            let result = contract(0, || Err("invalid\0identity".to_owned()));
            assert_eq!(result.status, ERROR);
            assert_eq!(payload(&result), "invalid\\0identity");
            // SAFETY: the pointer came from `contract` and is released exactly once.
            unsafe { free(result.payload) };
        }

        #[test]
        fn panic_payload_destructors_cannot_restart_unwinding() {
            struct PanicOnDrop;

            impl Drop for PanicOnDrop {
                fn drop(&mut self) {
                    panic!("panic payload destructor must never run");
                }
            }

            let result = catch_unwind(AssertUnwindSafe(|| {
                contract(0, || -> Result<String, String> { panic_any(PanicOnDrop) })
            }))
            .expect("discovery must contain even panic payloads with panicking destructors");
            assert_eq!(result.status, PANIC);
            assert!(result.payload.is_null());
        }
    }
}

#[cfg(all(feature = "python", not(target_arch = "wasm32")))]
#[doc(hidden)]
#[macro_export]
macro_rules! __python {
    ($($tokens:tt)*) => { $($tokens)* };
}

#[cfg(not(all(feature = "python", not(target_arch = "wasm32"))))]
#[doc(hidden)]
#[macro_export]
macro_rules! __python {
    ($($tokens:tt)*) => {};
}

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __typescript {
    ($($tokens:tt)*) => { $($tokens)* };
}

#[cfg(not(all(feature = "wasm", target_arch = "wasm32")))]
#[doc(hidden)]
#[macro_export]
macro_rules! __typescript {
    ($($tokens:tt)*) => {};
}

#[doc(hidden)]
pub mod __private {
    pub use crate::discovery::{
        DiscoveryResult, ERROR as DISCOVERY_ERROR, PANIC as DISCOVERY_PANIC,
        PYTHON as DISCOVERY_PYTHON, SUCCESS as DISCOVERY_SUCCESS,
        TYPESCRIPT as DISCOVERY_TYPESCRIPT, contract as discovery_contract, free as discovery_free,
    };
    pub use crate::wire::{BoundaryError, BufferDtype, BufferValue, WireValue};
    pub use inventory;
    pub use serde;
    pub use serde_json;

    #[cfg(all(feature = "python", not(target_arch = "wasm32")))]
    pub use pyo3;
    #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
    pub use wasm_bindgen;
}
