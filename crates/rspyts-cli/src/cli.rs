//! Command-line syntax and argument types.
//!
//! These types describe accepted input only. Execution and reporting remain in
//! `commands`, which keeps parsing independently testable and free of effects.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Top-level command-line parser.
#[derive(Debug, Parser)]
#[command(
    name = "rspyts",
    version,
    about = "Build one Rust API for Python and TypeScript"
)]
pub(crate) struct Cli {
    /// Operation selected by the caller.
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// Supported rspyts operations.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Create a Rust, Python, and TypeScript project.
    Init(InitArgs),
    /// Build the Python and TypeScript packages.
    Build(ProjectArgs),
    /// Rebuild when Rust or Cargo files change.
    Watch(ProjectArgs),
    /// Check that generated language sources match the Rust source.
    Check(ProjectArgs),
}

/// Arguments accepted by [`Command::Init`].
#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    /// New project directory. The final path component is the package name.
    pub(crate) path: PathBuf,
    /// Initial Cargo, Python, and npm package version.
    #[arg(long, default_value = "0.1.0")]
    pub(crate) version: semver::Version,
}

/// Shared arguments for commands that operate on an existing application.
#[derive(Debug, Args)]
pub(crate) struct ProjectArgs {
    /// Path to an rspyts application configuration.
    #[arg(long)]
    pub(crate) config: Option<PathBuf>,
}
