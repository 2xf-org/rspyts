//! The `rspyts` binary (codegen.md §2).
//!
//! ```text
//! rspyts generate [build options]   build host → dlopen → emit
//! rspyts check    [build options]   same, but diff (CI gate)
//! rspyts build    [build options]   build and stage selected artifacts
//! rspyts manifest [build options]   print the canonical manifest JSON
//! rspyts init                       write a starter rspyts.toml
//! ```
//!
//! Exit codes: 0 ok · 1 drift (`check`) · 2 usage/config error ·
//! 3 build or load failure. clap's own usage errors also exit 2.

mod build;
mod config;
mod emit;
mod load;
mod validate;

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
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
    /// Build the crate and (re)write all configured outputs.
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
    /// Build and stage the host or explicit Rust targets.
    Build {
        /// Path to rspyts.toml (default: ./rspyts.toml).
        #[arg(long, default_value = "rspyts.toml")]
        config: PathBuf,
        #[command(flatten)]
        build: CargoOptions,
        /// Artifact to build: `host` or a Rust target triple. Repeat to build several. Defaults to `host`.
        #[arg(long = "target", value_name = "host|TRIPLE", value_delimiter = ',')]
        targets: Vec<String>,
        /// Stage the selected artifact in this directory instead of the default package/Cargo location. Requires one target.
        #[arg(long, value_name = "DIR")]
        out_dir: Option<PathBuf>,
        /// Select human-readable text or a versioned JSON report on stdout.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        output_format: OutputFormat,
    },
    /// Build the host module and print its complete canonical manifest JSON.
    Manifest {
        /// Path to rspyts.toml (default: ./rspyts.toml).
        #[arg(long, default_value = "rspyts.toml")]
        config: PathBuf,
        #[command(flatten)]
        build: CargoOptions,
    },
    /// Write a commented starter rspyts.toml.
    Init {
        /// Directory to write into (default: current directory).
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    #[default]
    Text,
    Json,
}

#[derive(Args, Clone, Debug, Default)]
struct CargoOptions {
    /// Replace configured Cargo features (comma/space-separated).
    #[arg(long, value_name = "FEATURES")]
    features: Option<String>,
    /// Disable Cargo default features.
    #[arg(long)]
    no_default_features: bool,
    /// Build with Cargo's release profile.
    #[arg(long)]
    release: bool,
    /// Pass --locked to Cargo.
    #[arg(long)]
    locked: bool,
}

impl CargoOptions {
    fn overrides(&self) -> anyhow::Result<build::BuildOverrides> {
        let features = self
            .features
            .as_deref()
            .map(build::parse_features)
            .transpose()?;
        Ok(build::BuildOverrides {
            features,
            no_default_features: self.no_default_features.then_some(true),
            profile: self.release.then(|| "release".to_string()),
            locked: self.locked.then_some(true),
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
            out_dir,
            output_format,
        } => run_build(&config, &build, targets, out_dir.as_deref(), output_format),
        Command::Manifest { config, build } => run_manifest(&config, &build),
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
        .contract
        .build_options(cli_build.overrides().map_err(Failure::config)?)
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
    let host_artifact =
        build::build_host_cdylib(&cfg.crate_dir, &meta.name, &options).map_err(Failure::build)?;
    let loaded = load::load_manifest(&host_artifact).map_err(Failure::build)?;
    validate::validate(&loaded.manifest).map_err(Failure::build)?;
    eprintln!(
        "loaded manifest: {} type(s), {} constant(s), {} function(s), {} class(es)",
        loaded.manifest.types.len(),
        loaded.manifest.constants.len(),
        loaded.manifest.functions.len(),
        loaded.manifest.classes.len()
    );

    let hash = emit::util::manifest_hash_hex(&loaded.manifest);
    let plan = emit::plan(&cfg, &loaded.manifest, &hash);
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
    out_dir: Option<&Path>,
    output_format: OutputFormat,
) -> Result<(), Failure> {
    let cfg = config::load(config_path).map_err(Failure::config)?;
    let options = cfg
        .contract
        .build_options(cli_build.overrides().map_err(Failure::config)?)
        .map_err(Failure::config)?;
    let mut selection = build::BuildSelection::new(targets).map_err(Failure::config)?;
    if selection.include_host && !selection.targets.is_empty() {
        let host = build::rustc_host().map_err(Failure::build)?;
        selection.deduplicate_included_host(&host);
    }
    if out_dir.is_some() && selection.artifact_count() != 1 {
        return Err(Failure::config(anyhow::anyhow!(
            "`--out-dir` requires exactly one selected build target"
        )));
    }
    let meta = build::crate_meta(&cfg.crate_dir).map_err(Failure::config)?;
    eprintln!(
        "building and staging `{}` v{} ({}, {} artifact(s))",
        meta.name,
        meta.version,
        options.profile_dir(),
        selection.artifact_count()
    );
    let python_out = cfg.python.as_ref().map(|python| python.out.as_path());
    let artifacts = build::build_and_stage(
        &cfg.crate_dir,
        &meta.name,
        &options,
        &selection,
        python_out,
        out_dir,
    )
    .map_err(Failure::build)?;
    match output_format {
        OutputFormat::Text => {
            for artifact in artifacts {
                println!("staged     {}", artifact.path.display());
            }
        }
        OutputFormat::Json => write_build_report(&cfg, &meta, &options, &artifacts)?,
    }
    Ok(())
}

const BUILD_REPORT_FORMAT_VERSION: u8 = 1;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildReport<'a> {
    format_version: u8,
    #[serde(rename = "crate")]
    krate: CrateReport<'a>,
    build: ResolvedBuildReport<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    python: Option<PythonReport<'a>>,
    artifacts: &'a [build::StagedArtifact],
}

#[derive(Debug, Serialize)]
struct CrateReport<'a> {
    name: &'a str,
    version: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedBuildReport<'a> {
    features: &'a [String],
    no_default_features: bool,
    profile: &'a str,
    locked: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PythonReport<'a> {
    out: &'a Path,
}

fn build_report<'a>(
    cfg: &'a config::Config,
    meta: &'a build::CrateMeta,
    options: &'a build::BuildOptions,
    artifacts: &'a [build::StagedArtifact],
) -> BuildReport<'a> {
    BuildReport {
        format_version: BUILD_REPORT_FORMAT_VERSION,
        krate: CrateReport {
            name: &meta.name,
            version: &meta.version,
        },
        build: ResolvedBuildReport {
            features: &options.features,
            no_default_features: options.no_default_features,
            profile: &options.profile,
            locked: options.locked,
        },
        python: cfg
            .python
            .as_ref()
            .map(|python| PythonReport { out: &python.out }),
        artifacts,
    }
}

fn write_build_report(
    cfg: &config::Config,
    meta: &build::CrateMeta,
    options: &build::BuildOptions,
    artifacts: &[build::StagedArtifact],
) -> Result<(), Failure> {
    let stdout = std::io::stdout();
    let mut output = std::io::BufWriter::new(stdout.lock());
    serde_json::to_writer_pretty(&mut output, &build_report(cfg, meta, options, artifacts))
        .map_err(|error| {
            Failure::build(anyhow::Error::new(error).context("cannot serialize build report"))
        })?;
    writeln!(output).map_err(|error| {
        Failure::build(anyhow::Error::new(error).context("cannot write build report to stdout"))
    })?;
    output.flush().map_err(|error| {
        Failure::build(anyhow::Error::new(error).context("cannot flush build report to stdout"))
    })?;
    Ok(())
}

fn run_manifest(config_path: &Path, cli_build: &CargoOptions) -> Result<(), Failure> {
    let cfg = config::load(config_path).map_err(Failure::config)?;
    let options = cfg
        .contract
        .build_options(cli_build.overrides().map_err(Failure::config)?)
        .map_err(Failure::config)?;
    let meta = build::crate_meta(&cfg.crate_dir).map_err(Failure::config)?;
    eprintln!(
        "building `{}` v{} ({})",
        meta.name,
        meta.version,
        options.profile_dir()
    );
    let host_artifact =
        build::build_host_cdylib(&cfg.crate_dir, &meta.name, &options).map_err(Failure::build)?;
    let loaded = load::load_manifest(&host_artifact).map_err(Failure::build)?;
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

fn run_init(dir: &Path) -> Result<(), Failure> {
    let path = config::init(dir).map_err(Failure::config)?;
    println!("wrote      {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn positive_build_options_and_targets_parse() {
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
        let overrides = build.overrides().unwrap();
        assert_eq!(
            overrides.features,
            Some(vec!["serde".into(), "fast".into()])
        );
        assert_eq!(overrides.profile.as_deref(), Some("release"));
        assert_eq!(overrides.locked, Some(true));
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn build_output_format_defaults_to_text_and_accepts_json() {
        let cli = Cli::try_parse_from(["rspyts", "build"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Build {
                output_format: OutputFormat::Text,
                ..
            }
        ));

        let cli = Cli::try_parse_from(["rspyts", "build", "--output-format", "json"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Build {
                output_format: OutputFormat::Json,
                ..
            }
        ));
        assert!(Cli::try_parse_from(["rspyts", "build", "--output-format", "xml"]).is_err());
    }

    #[test]
    fn target_and_output_selection_are_positive() {
        let cli = Cli::try_parse_from([
            "rspyts",
            "build",
            "--target",
            "wasm32-unknown-unknown",
            "--out-dir",
            "dist",
        ])
        .unwrap();
        let Command::Build {
            targets, out_dir, ..
        } = cli.command
        else {
            panic!("expected build command")
        };
        assert_eq!(targets, ["wasm32-unknown-unknown"]);
        assert_eq!(out_dir.as_deref(), Some(Path::new("dist")));

        for removed in [
            ["rspyts", "check", "--no-python-stage"].as_slice(),
            ["rspyts", "manifest", "--no-python-stage"].as_slice(),
            ["rspyts", "build", "--no-host"].as_slice(),
            ["rspyts", "build", "--no-targets"].as_slice(),
            ["rspyts", "check", "--unlocked"].as_slice(),
            ["rspyts", "check", "--default-features"].as_slice(),
        ] {
            assert!(Cli::try_parse_from(removed).is_err(), "{removed:?}");
        }
    }

    #[test]
    fn json_build_report_has_a_versioned_machine_readable_shape() {
        let cfg = config::Config {
            crate_dir: PathBuf::from("/workspace/crate"),
            python: Some(config::PythonConfig {
                out: PathBuf::from("/workspace/python/generated"),
                imports: Default::default(),
            }),
            typescript: None,
            schema: None,
            contract: build::ContractOptions::default(),
        };
        let meta = build::CrateMeta {
            name: "demo-crate".into(),
            version: "1.2.3".into(),
        };
        let options = build::BuildOptions::new(
            vec!["serde".into(), "fast".into()],
            true,
            "release".into(),
            true,
        )
        .unwrap();
        let artifacts = vec![
            build::StagedArtifact {
                kind: build::ArtifactKind::Native,
                target: "aarch64-apple-darwin".into(),
                path: PathBuf::from("/workspace/python/generated/lib/libdemo_crate.dylib"),
            },
            build::StagedArtifact {
                kind: build::ArtifactKind::Target,
                target: "wasm32-unknown-unknown".into(),
                path: PathBuf::from(
                    "/workspace/target/rspyts/wasm32-unknown-unknown/release/demo_crate.wasm",
                ),
            },
        ];

        assert_eq!(
            serde_json::to_value(build_report(&cfg, &meta, &options, &artifacts)).unwrap(),
            json!({
                "formatVersion": 1,
                "crate": {"name": "demo-crate", "version": "1.2.3"},
                "build": {
                    "features": ["serde", "fast"],
                    "noDefaultFeatures": true,
                    "profile": "release",
                    "locked": true
                },
                "python": {
                    "out": "/workspace/python/generated"
                },
                "artifacts": [
                    {
                        "kind": "native",
                        "target": "aarch64-apple-darwin",
                        "path": "/workspace/python/generated/lib/libdemo_crate.dylib"
                    },
                    {
                        "kind": "target",
                        "target": "wasm32-unknown-unknown",
                        "path": "/workspace/target/rspyts/wasm32-unknown-unknown/release/demo_crate.wasm"
                    }
                ]
            })
        );
    }

    #[test]
    fn json_build_report_omits_disabled_python_output() {
        let cfg = config::Config {
            crate_dir: PathBuf::from("/workspace/crate"),
            python: None,
            typescript: None,
            schema: None,
            contract: build::ContractOptions::default(),
        };
        let meta = build::CrateMeta {
            name: "demo".into(),
            version: "0.1.0".into(),
        };
        let options = build::BuildOptions::default();

        let value = serde_json::to_value(build_report(&cfg, &meta, &options, &[])).unwrap();
        assert!(value.get("python").is_none());
    }

    #[test]
    fn removed_diff_command_is_rejected() {
        assert!(Cli::try_parse_from(["rspyts", "diff", "old.json", "new.json"]).is_err());
    }
}
