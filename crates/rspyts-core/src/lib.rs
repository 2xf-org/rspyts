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
#[doc(hidden)]
pub mod wire;

pub use bridged::{Bridged, Buf, BufElem, Bytes, SliceElem};
pub use error::{BridgeErr, BridgeError};
use sha2::{Digest, Sha256};

/// The ABI major version exposed via `rspyts_abi_version()`.
pub const ABI_VERSION: u32 = 3;

/// The ABI minor version recorded alongside [`ABI_VERSION`].
pub const ABI_MINOR: u32 = 0;

/// The ABI version string embedded in manifests.
pub const ABI_VERSION_STR: &str = "3.0";

/// Hash exact compact manifest JSON bytes for [`manifest_fingerprint`].
fn contract_fingerprint(json: &[u8]) -> String {
    let digest = Sha256::digest(json);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    hex
}

/// Serialize and fingerprint an in-memory manifest exactly as
/// [`envelope::encode_ok`] serializes the manifest export.
///
/// Both the CLI's generated-file provenance and the module's runtime
/// fingerprint export call this function, so code generation and load-time
/// verification cannot drift onto different hashing implementations.
pub fn manifest_fingerprint(manifest: &ir::Manifest) -> String {
    let json = serde_json::to_vec(manifest)
        .expect("rspyts: the in-memory manifest must always serialize as JSON");
    contract_fingerprint(&json)
}

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

    #[test]
    fn manifest_fingerprint_is_stable_lowercase_sha256() {
        let manifest = ir::Manifest {
            abi: ABI_VERSION_STR.to_string(),
            crate_name: "demo".to_string(),
            crate_version: "1.2.3".to_string(),
            types: Vec::new(),
            constants: Vec::new(),
            functions: Vec::new(),
            classes: Vec::new(),
        };
        let first = manifest_fingerprint(&manifest);
        assert_eq!(first.len(), 64);
        assert!(
            first
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        assert_eq!(first, manifest_fingerprint(&manifest));

        let mut changed = manifest;
        changed.crate_version = "1.2.4".to_string();
        assert_ne!(first, manifest_fingerprint(&changed));

        let json = serde_json::to_vec(&changed).unwrap();
        assert_eq!(manifest_fingerprint(&changed), contract_fingerprint(&json));
    }
}
