//! The `rspyts` binary (codegen.md §2).
//!
//! ```text
//! rspyts generate [build options]   build host → dlopen → emit
//! rspyts check    [build options]   same, but diff (CI gate)
//! rspyts build    [build options]   stage host + configured targets
//! rspyts manifest [build options]   print the canonical manifest JSON
//! rspyts diff <old> <new>           classify manifest compatibility
//! rspyts init                       write a starter rspyts.toml
//! ```
//!
//! Exit codes: 0 ok · 1 drift (`check`) · 2 usage/config error ·
//! 3 build or load failure. clap's own usage errors also exit 2.

mod build;
mod config;
mod diff;
mod emit;
mod exclude;
mod load;
mod validate;

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::io::Write;
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
        #[command(flatten)]
        build: CargoOptions,
    },
    /// Build the crate and fail (exit 1) if outputs are out of date.
    Check {
        /// Path to rspyts.toml (default: ./rspyts.toml).
        #[arg(long, default_value = "rspyts.toml")]
        config: PathBuf,
        #[command(flatten)]
        build: CargoOptions,
    },
    /// Build and stage host plus configured target artifacts.
    Build {
        /// Path to rspyts.toml (default: ./rspyts.toml).
        #[arg(long, default_value = "rspyts.toml")]
        config: PathBuf,
        #[command(flatten)]
        build: CargoOptions,
        /// Replace configured build targets (repeat or comma-separate).
        #[arg(long = "target", value_delimiter = ',', conflicts_with = "no_targets")]
        targets: Vec<String>,
        /// Build only the host, overriding configured targets.
        #[arg(long, conflicts_with = "targets")]
        no_targets: bool,
    },
    /// Build the host module and print its complete canonical manifest JSON.
    Manifest {
        /// Path to rspyts.toml (default: ./rspyts.toml).
        #[arg(long, default_value = "rspyts.toml")]
        config: PathBuf,
        #[command(flatten)]
        build: CargoOptions,
    },
    /// Conservatively classify compatibility between two manifest snapshots.
    Diff {
        /// Previous complete manifest JSON.
        old: PathBuf,
        /// New complete manifest JSON.
        new: PathBuf,
        /// Which class of change should make the command exit 1.
        #[arg(long, value_enum, default_value_t = FailOn::Breaking)]
        fail_on: FailOn,
    },
    /// Write a commented starter rspyts.toml.
    Init {
        /// Directory to write into (default: current directory).
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
enum FailOn {
    #[default]
    Breaking,
    Any,
}

#[derive(Args, Clone, Debug, Default)]
struct CargoOptions {
    /// Replace configured Cargo features (comma/space-separated).
    #[arg(long, value_name = "FEATURES", conflicts_with = "no_features")]
    features: Option<String>,
    /// Disable every configured feature.
    #[arg(long, conflicts_with = "features")]
    no_features: bool,
    /// Disable Cargo default features.
    #[arg(long, conflicts_with = "default_features")]
    no_default_features: bool,
    /// Enable Cargo default features, overriding config.
    #[arg(long, conflicts_with = "no_default_features")]
    default_features: bool,
    /// Replace the configured Cargo profile.
    #[arg(long, value_name = "NAME", conflicts_with = "release")]
    profile: Option<String>,
    /// Shorthand for --profile release.
    #[arg(long, conflicts_with = "profile")]
    release: bool,
    /// Pass --locked to Cargo.
    #[arg(long, conflicts_with = "unlocked")]
    locked: bool,
    /// Do not pass --locked, overriding config.
    #[arg(long, conflicts_with = "locked")]
    unlocked: bool,
}

impl CargoOptions {
    fn overrides(&self, targets: Option<Vec<String>>) -> anyhow::Result<build::BuildOverrides> {
        let features = if self.no_features {
            Some(Vec::new())
        } else {
            self.features
                .as_deref()
                .map(build::parse_features)
                .transpose()?
        };
        let no_default_features = if self.no_default_features {
            Some(true)
        } else if self.default_features {
            Some(false)
        } else {
            None
        };
        let profile = if self.release {
            Some("release".to_string())
        } else {
            self.profile.clone()
        };
        let locked = if self.locked {
            Some(true)
        } else if self.unlocked {
            Some(false)
        } else {
            None
        };
        Ok(build::BuildOverrides {
            features,
            no_default_features,
            profile,
            targets,
            locked,
        })
    }
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
        Command::Generate { config, build } => run_pipeline(&config, &build, Mode::Generate),
        Command::Check { config, build } => run_pipeline(&config, &build, Mode::Check),
        Command::Build {
            config,
            build,
            targets,
            no_targets,
        } => run_build(&config, &build, targets, no_targets),
        Command::Manifest { config, build } => run_manifest(&config, &build),
        Command::Diff { old, new, fail_on } => run_diff(&old, &new, fail_on),
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
fn run_pipeline(config_path: &Path, cli_build: &CargoOptions, mode: Mode) -> Result<(), Failure> {
    let cfg = config::load(config_path).map_err(Failure::config)?;
    let options = cfg
        .build
        .with_overrides(cli_build.overrides(None).map_err(Failure::config)?)
        .map_err(Failure::config)?;
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
        options.profile_dir()
    );
    if !options.targets.is_empty() {
        eprintln!(
            "warning: [build].targets is ignored by generate/check; run `rspyts build` to stage {} configured target(s)",
            options.targets.len()
        );
    }
    let artifact =
        build::build_host_cdylib(&cfg.crate_dir, &meta.name, &options).map_err(Failure::build)?;
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

fn run_build(
    config_path: &Path,
    cli_build: &CargoOptions,
    targets: Vec<String>,
    no_targets: bool,
) -> Result<(), Failure> {
    let cfg = config::load(config_path).map_err(Failure::config)?;
    let target_override = if no_targets {
        Some(Vec::new())
    } else if targets.is_empty() {
        None
    } else {
        Some(targets)
    };
    let options = cfg
        .build
        .with_overrides(
            cli_build
                .overrides(target_override)
                .map_err(Failure::config)?,
        )
        .map_err(Failure::config)?;
    let meta = build::crate_meta(&cfg.crate_dir).map_err(Failure::config)?;
    eprintln!(
        "building and staging `{}` v{} ({}, {} additional target(s))",
        meta.name,
        meta.version,
        options.profile_dir(),
        options.targets.len()
    );
    let artifacts =
        build::build_and_stage(&cfg.crate_dir, &meta.name, &options).map_err(Failure::build)?;
    for artifact in artifacts {
        println!("staged     {}", artifact.display());
    }
    Ok(())
}

fn run_manifest(config_path: &Path, cli_build: &CargoOptions) -> Result<(), Failure> {
    let cfg = config::load(config_path).map_err(Failure::config)?;
    let options = cfg
        .build
        .with_overrides(cli_build.overrides(None).map_err(Failure::config)?)
        .map_err(Failure::config)?;
    let meta = build::crate_meta(&cfg.crate_dir).map_err(Failure::config)?;
    eprintln!(
        "building `{}` v{} ({})",
        meta.name,
        meta.version,
        options.profile_dir()
    );
    if !options.targets.is_empty() {
        eprintln!(
            "warning: [build].targets is ignored by manifest; only the host module can be loaded"
        );
    }
    let artifact =
        build::build_host_cdylib(&cfg.crate_dir, &meta.name, &options).map_err(Failure::build)?;
    let loaded = load::load_manifest(&artifact).map_err(Failure::build)?;
    validate::validate(&loaded.manifest).map_err(Failure::build)?;

    let stdout = std::io::stdout();
    let mut output = std::io::BufWriter::new(stdout.lock());
    serde_json::to_writer_pretty(&mut output, &loaded.manifest).map_err(|error| {
        Failure::build(anyhow::Error::new(error).context("cannot serialize manifest"))
    })?;
    writeln!(output).map_err(|error| {
        Failure::build(anyhow::Error::new(error).context("cannot write manifest to stdout"))
    })?;
    Ok(())
}

fn run_diff(old_path: &Path, new_path: &Path, fail_on: FailOn) -> Result<(), Failure> {
    let old = diff::read_manifest(old_path).map_err(Failure::build)?;
    let new = diff::read_manifest(new_path).map_err(Failure::build)?;
    validate::validate(&old).map_err(|error| {
        Failure::build(error.context(format!(
            "old input `{}` is not a valid manifest",
            old_path.display()
        )))
    })?;
    validate::validate(&new).map_err(|error| {
        Failure::build(error.context(format!(
            "new input `{}` is not a valid manifest",
            new_path.display()
        )))
    })?;

    let report = diff::compare(&old, &new);
    print!("{}", report.render());
    let fail_on_any = fail_on == FailOn::Any;
    if diff::exit_code(&report, fail_on_any) == 1 {
        return Err(Failure {
            code: 1,
            error: anyhow::anyhow!(
                "manifest changes violate the selected `{}` compatibility policy",
                if fail_on_any { "any" } else { "breaking" }
            ),
        });
    }
    Ok(())
}

fn run_init(dir: &Path) -> Result<(), Failure> {
    let path = config::init(dir).map_err(Failure::config)?;
    println!("wrote      {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_cli_flags_produce_explicit_false_overrides() {
        let cli = Cli::try_parse_from([
            "rspyts",
            "build",
            "--no-features",
            "--default-features",
            "--unlocked",
            "--no-targets",
        ])
        .unwrap();
        let Command::Build {
            build, no_targets, ..
        } = cli.command
        else {
            panic!("expected build command")
        };
        let overrides = build.overrides(Some(Vec::new())).unwrap();
        assert_eq!(overrides.features, Some(Vec::new()));
        assert_eq!(overrides.no_default_features, Some(false));
        assert_eq!(overrides.locked, Some(false));
        assert!(no_targets);
    }

    #[test]
    fn release_features_and_targets_parse_as_replacements() {
        let cli = Cli::try_parse_from([
            "rspyts",
            "build",
            "--features",
            "serde,fast",
            "--release",
            "--locked",
            "--target",
            "wasm32-unknown-unknown,x86_64-unknown-linux-gnu",
        ])
        .unwrap();
        let Command::Build { build, targets, .. } = cli.command else {
            panic!("expected build command")
        };
        let overrides = build.overrides(Some(targets)).unwrap();
        assert_eq!(
            overrides.features,
            Some(vec!["serde".into(), "fast".into()])
        );
        assert_eq!(overrides.profile.as_deref(), Some("release"));
        assert_eq!(overrides.locked, Some(true));
        assert_eq!(overrides.targets.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn conflicting_cli_overrides_are_rejected_by_clap() {
        for args in [
            vec!["rspyts", "check", "--release", "--profile", "dev"],
            vec!["rspyts", "check", "--locked", "--unlocked"],
            vec![
                "rspyts",
                "build",
                "--target",
                "wasm32-unknown-unknown",
                "--no-targets",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn diff_fail_policy_parses_with_equals_or_space() {
        for args in [
            vec!["rspyts", "diff", "old.json", "new.json", "--fail-on=any"],
            vec!["rspyts", "diff", "old.json", "new.json", "--fail-on", "any"],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(matches!(
                cli.command,
                Command::Diff {
                    fail_on: FailOn::Any,
                    ..
                }
            ));
        }
    }

    #[test]
    fn diff_defaults_to_breaking_and_rejects_unknown_policy() {
        let cli = Cli::try_parse_from(["rspyts", "diff", "old.json", "new.json"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Diff {
                fail_on: FailOn::Breaking,
                ..
            }
        ));
        assert!(
            Cli::try_parse_from(["rspyts", "diff", "old.json", "new.json", "--fail-on=minor"])
                .is_err()
        );
    }
}
