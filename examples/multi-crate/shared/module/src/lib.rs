//! shared-types-module — the standalone compiled module for `shared-types`.
//!
//! `shared-types` itself carries no `rspyts::export!()` because it is
//! linked into other bridged modules and each compiled module has exactly
//! one exporter. This leaf crate exists purely so `rspyts generate` has a
//! cdylib to load when generating shared-types' own Python and TypeScript
//! packages: the re-export links the dependency (pulling its type
//! registrations into the manifest) and the macro adds the module symbols.

pub use shared_types::*;

rspyts::export!();
