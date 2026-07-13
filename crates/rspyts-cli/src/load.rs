//! Loading the compiled cdylib and retrieving its manifest.
//!
//! The CLI is itself a foreign caller of the module it just built: it
//! `dlopen`s the artifact, verifies `rspyts_abi_version() == 1`, calls
//! `rspyts_manifest()`, decodes the response envelope (ABI §4), and
//! frees the envelope through the module's own `rspyts_free`.

use anyhow::{Context, Result, bail, ensure};
use rspyts_core::envelope;
use rspyts_core::ir::Manifest;
use std::path::Path;

/// The deserialized manifest plus the exact JSON bytes it arrived as
/// (hashed into every generated file header for provenance).
pub struct LoadedManifest {
    pub manifest: Manifest,
    pub json: Vec<u8>,
}

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type ManifestFn = unsafe extern "C" fn() -> *mut u8;
type FreeFn = unsafe extern "C" fn(*mut u8, usize);

/// `dlopen` the module at `path` and pull its manifest out.
pub fn load_manifest(path: &Path) -> Result<LoadedManifest> {
    // SAFETY: we load a module we just built ourselves and only call the
    // four spec-defined exports with their spec-defined signatures. The
    // envelope pointer is decoded and freed exactly once, and the JSON
    // is copied out before the free.
    unsafe {
        let lib = libloading::Library::new(path)
            .with_context(|| format!("cannot load `{}`", path.display()))?;

        let abi_version: libloading::Symbol<AbiVersionFn> =
            lib.get(b"rspyts_abi_version\0").context(
                "module does not export `rspyts_abi_version` — is `rspyts::export!()` present?",
            )?;
        let version = abi_version();
        ensure!(
            version == rspyts_core::ABI_VERSION,
            "module reports ABI version {version}; this rspyts CLI supports version {}",
            rspyts_core::ABI_VERSION
        );

        let manifest_fn: libloading::Symbol<ManifestFn> = lib
            .get(b"rspyts_manifest\0")
            .context("module does not export `rspyts_manifest`")?;
        let free_fn: libloading::Symbol<FreeFn> = lib
            .get(b"rspyts_free\0")
            .context("module does not export `rspyts_free`")?;

        let ptr = manifest_fn();
        ensure!(!ptr.is_null(), "`rspyts_manifest` returned a null envelope");
        let total = envelope::total_len(ptr);
        let decoded = envelope::decode(ptr);
        let status = decoded.status;
        let json = decoded.json.to_vec();
        free_fn(ptr, total);

        if status != envelope::STATUS_OK {
            let payload = String::from_utf8_lossy(&json).into_owned();
            bail!("`rspyts_manifest` failed (status {status}): {payload}");
        }
        let manifest: Manifest = serde_json::from_slice(&json)
            .context("cannot deserialize the module manifest — CLI/crate version mismatch?")?;
        Ok(LoadedManifest { manifest, json })
    }
}
