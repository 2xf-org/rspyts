//! Core machinery for the [rspyts](https://github.com/2xf-org/rspyts) bridge.
//!
//! This crate contains everything that is shared between the proc macros,
//! the generated shims, and the `rspyts` CLI:
//!
//! - [`ir`] — the intermediate representation (manifest) describing every
//!   bridged type, function, and class. The CLI consumes this to emit
//!   Python, TypeScript, and JSON Schema.
//! - [`envelope`] — the symmetric request/response envelope format (ABI §4),
//!   typed binary attachments, and the
//!   `rspyts_alloc`/`rspyts_free` allocation rules (ABI §2).
//! - [`bridged`] — the [`bridged::Bridged`] trait mapping Rust
//!   types onto the portable type system, plus [`bridged::Buf`] and
//!   [`bridged::Bytes`] for owned binary inputs and returns.
//! - [`registry`] — `inventory`-based registration and deterministic
//!   manifest assembly.
//! - [`handles`] — the slab behind opaque class handles.
//! - [`shim`] — the panic-contained entry points that macro-generated
//!   `extern "C"` functions delegate to.
//!
//! Application code should depend on the `rspyts` facade crate instead of
//! this one. Everything here is semver-exempt plumbing (`__private` in the
//! facade); the stable contracts are `docs/design/abi.md` and
//! `docs/design/type-system.md`.

pub mod bridged;
pub mod envelope;
pub mod error;
pub mod handles;
pub mod ir;
pub mod registry;
pub mod shim;

pub use bridged::{Bridged, Buf, BufElem, Bytes, I64, Json, SliceElem, U64};
pub use error::{BridgeErr, BridgeError};

/// The ABI major version exposed via `rspyts_abi_version()`.
pub const ABI_VERSION: u32 = 2;

/// The ABI minor version recorded alongside [`ABI_VERSION`].
pub const ABI_MINOR: u32 = 0;

/// The ABI version string embedded in manifests.
pub const ABI_VERSION_STR: &str = "2.0";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_string_matches_numeric_components() {
        let (major, minor) = ABI_VERSION_STR
            .split_once('.')
            .expect("ABI version string contains one dot");
        assert_eq!(major.parse::<u32>().unwrap(), ABI_VERSION);
        assert_eq!(minor.parse::<u32>().unwrap(), ABI_MINOR);
    }
}
