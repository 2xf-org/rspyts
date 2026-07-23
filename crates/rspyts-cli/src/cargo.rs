//! Typed access to Cargo subprocesses and metadata.
//!
//! Cargo's JSON document is deserialized at this boundary so the rest of the
//! CLI operates on named fields instead of string-indexed values. Only the
//! subset required by project discovery and bridge construction is modeled;
//! Serde intentionally ignores additional fields added by Cargo.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Typed subset of a Cargo metadata document used by rspyts.
#[derive(Deserialize)]
pub(super) struct Metadata {
    /// Packages included in the metadata query.
    pub(super) packages: Vec<Package>,
    /// Package IDs belonging to the active workspace.
    pub(super) workspace_members: Vec<String>,
    /// Canonical Cargo workspace root.
    pub(super) workspace_root: PathBuf,
    /// Effective Cargo target directory.
    pub(super) target_directory: PathBuf,
    /// Resolved dependency graph, omitted by `--no-deps` queries.
    pub(super) resolve: Option<Resolve>,
}

/// Cargo package fields required by rspyts.
#[derive(Deserialize)]
pub(super) struct Package {
    /// Opaque Cargo package ID.
    pub(super) id: String,
    /// Cargo package name.
    pub(super) name: String,
    /// Cargo package version.
    pub(super) version: String,
    /// Package manifest path.
    pub(super) manifest_path: PathBuf,
    /// Registry/source identity, or `None` for a local package.
    pub(super) source: Option<String>,
    /// Compilation targets declared by the package.
    pub(super) targets: Vec<Target>,
}

/// Cargo target fields used to identify linkable libraries.
#[derive(Deserialize)]
pub(super) struct Target {
    /// Cargo target kinds such as `lib` or `bin`.
    pub(super) kind: Vec<String>,
    /// rustc crate types emitted by the target.
    pub(super) crate_types: Vec<String>,
}

/// Resolved Cargo dependency graph.
#[derive(Deserialize)]
pub(super) struct Resolve {
    /// One dependency node per resolved package.
    pub(super) nodes: Vec<Node>,
}

/// Dependencies resolved for one Cargo package.
#[derive(Deserialize)]
pub(super) struct Node {
    /// Opaque Cargo package ID for this node.
    pub(super) id: String,
    /// Named outgoing dependency edges.
    pub(super) deps: Vec<Dependency>,
}

/// One named edge in Cargo's resolved dependency graph.
#[derive(Deserialize)]
pub(super) struct Dependency {
    /// Opaque package ID of the dependency target.
    pub(super) pkg: String,
    /// Build contexts in which this dependency applies.
    pub(super) dep_kinds: Vec<DependencyKind>,
}

/// Build context for one Cargo dependency edge.
#[derive(Deserialize)]
pub(super) struct DependencyKind {
    /// `None` for a normal dependency; otherwise `dev` or `build`.
    pub(super) kind: Option<String>,
}

/// Read Cargo metadata for one manifest.
///
/// # Errors
///
/// Returns an error when Cargo cannot run, rejects the project, or emits a
/// metadata document that does not satisfy the modeled schema.
pub(super) fn metadata(manifest: &Path, no_dependencies: bool) -> Result<Metadata> {
    let mut command = Command::new(executable());
    command
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(manifest);
    if no_dependencies {
        command.arg("--no-deps");
    }
    let output = command.output().context("failed to run cargo metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout).context("Cargo returned invalid metadata")
}

/// Return Cargo's executable, honoring Cargo's subprocess environment.
pub(super) fn executable() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}
