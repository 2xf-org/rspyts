//! Command-line entry points for the rspyts build orchestrator.
//!
//! Argument syntax lives in [`cli`], while command behavior lives in
//! [`commands`]. This module owns only the process and embeddable I/O
//! boundaries.

use std::ffi::OsString;
use std::io::{self, Write};

use anyhow::Result;
use clap::Parser;

mod cli;
mod commands;
mod config;
mod contract;
mod init;
mod output;
mod project;
mod python;
mod typescript;

use cli::Cli;

/// Parse the current process arguments and run the selected command.
///
/// Normal command output is written to stdout. Recoverable watch failures are
/// written to stderr; fatal errors are returned to the binary entry point.
///
/// # Errors
///
/// Returns an error when command input is invalid or a requested operation
/// fails.
pub fn run() -> Result<()> {
    let stdout = io::stdout();
    let stderr = io::stderr();
    commands::execute(Cli::parse().command, &mut stdout.lock(), &mut stderr.lock())
}

/// Parse explicit arguments and run a command with caller-provided output.
///
/// This entry point is intended for integrations that need deterministic
/// argument and output boundaries without replacing global process state.
/// Include the binary name as the first argument, just as with
/// [`std::env::args_os`].
///
/// # Errors
///
/// Returns a clap parsing error for invalid arguments, or the command error
/// when execution fails.
pub fn run_with<I, T>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    commands::execute(Cli::try_parse_from(args)?.command, stdout, stderr)
}
