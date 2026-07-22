//! Command-line syntax and argument types.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "rspyts",
    version,
    about = "Build one Rust API for Python and TypeScript"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

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

#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    /// New project directory. The final path component is the package name.
    pub(crate) path: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct ProjectArgs {
    /// Path to an RSPYTS application configuration.
    #[arg(long)]
    pub(crate) config: Option<PathBuf>,
}
