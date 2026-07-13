//! The `rspyts` binary (codegen.md §2).
//!
//! ```text
//! rspyts generate [--config <path>] [--release]   build → dlopen → emit
//! rspyts check    [--config <path>] [--release]   same, but diff (CI gate)
//! rspyts init     [--dir <path>]                  write a starter rspyts.toml
//! ```
//!
//! Exit codes: 0 ok · 1 drift (`check`) · 2 usage/config error ·
//! 3 build or load failure. clap's own usage errors also exit 2.

mod build;
mod config;
mod emit;
mod exclude;
mod load;
mod validate;

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "rspyts",
    version,
    about = "Generate Python, TypeScript, and JSON Schema surfaces from a bridged Rust crate."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build the crate and (re)write all enabled outputs.
    Generate {
        /// Path to rspyts.toml (default: ./rspyts.toml).
        #[arg(long, default_value = "rspyts.toml")]
        config: PathBuf,
        /// Build the bridged crate with --release.
        #[arg(long)]
        release: bool,
    },
    /// Build the crate and fail (exit 1) if outputs are out of date.
    Check {
        /// Path to rspyts.toml (default: ./rspyts.toml).
        #[arg(long, default_value = "rspyts.toml")]
        config: PathBuf,
        /// Build the bridged crate with --release.
        #[arg(long)]
        release: bool,
    },
    /// Write a commented starter rspyts.toml.
    Init {
        /// Directory to write into (default: current directory).
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
}

/// A failure tagged with its spec-mandated exit code.
struct Failure {
    code: u8,
    error: anyhow::Error,
}

impl Failure {
    fn config(error: anyhow::Error) -> Self {
        Self { code: 2, error }
    }
    fn build(error: anyhow::Error) -> Self {
        Self { code: 3, error }
    }
    fn drift() -> Self {
        Self {
            code: 1,
            error: anyhow::anyhow!("generated code is out of date; run `rspyts generate`"),
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Generate { config, release } => run_pipeline(&config, release, Mode::Generate),
        Command::Check { config, release } => run_pipeline(&config, release, Mode::Check),
        Command::Init { dir } => run_init(&dir),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("error: {:#}", failure.error);
            ExitCode::from(failure.code)
        }
    }
}

enum Mode {
    Generate,
    Check,
}

/// The shared generate/check pipeline: config → cargo build → dlopen →
/// manifest → validate → render → write or diff.
fn run_pipeline(config_path: &Path, release: bool, mode: Mode) -> Result<(), Failure> {
    let cfg = config::load(config_path).map_err(Failure::config)?;
    if cfg.python.is_none() && cfg.typescript.is_none() && cfg.schema.is_none() {
        return Err(Failure::config(anyhow::anyhow!(
            "`{}` enables no outputs — add a [python], [typescript], or [schema] section",
            config_path.display()
        )));
    }

    let meta = build::crate_meta(&cfg.crate_dir).map_err(Failure::config)?;
    eprintln!(
        "building `{}` v{} ({})",
        meta.name,
        meta.version,
        if release { "release" } else { "debug" }
    );
    let artifact =
        build::build_cdylib(&cfg.crate_dir, &meta.name, release).map_err(Failure::build)?;
    let loaded = load::load_manifest(&artifact).map_err(Failure::build)?;
    validate::validate(&loaded.manifest).map_err(Failure::build)?;
    eprintln!(
        "loaded manifest: {} type(s), {} constant(s), {} function(s), {} class(es)",
        loaded.manifest.types.len(),
        loaded.manifest.constants.len(),
        loaded.manifest.functions.len(),
        loaded.manifest.classes.len()
    );

    let hash = emit::util::manifest_hash_hex(&loaded.json);
    // A bad exclude list is a config problem, hence exit code 2.
    let plan = emit::plan(&cfg, &loaded.manifest, &hash).map_err(Failure::config)?;
    match mode {
        Mode::Generate => emit::write(&plan).map_err(Failure::build),
        Mode::Check => match emit::check(&plan).map_err(Failure::build)? {
            true => Err(Failure::drift()),
            false => {
                eprintln!("generated code is up to date");
                Ok(())
            }
        },
    }
}

fn run_init(dir: &Path) -> Result<(), Failure> {
    let path = config::init(dir).map_err(Failure::config)?;
    println!("wrote      {}", path.display());
    Ok(())
}
