use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use serde_json::json;

mod contract;
mod init;
mod output;
mod project;
mod python;
mod typescript;

use output::source_state;
use project::{Project, build, check};

#[derive(Debug, Parser)]
#[command(
    name = "rspyts",
    version,
    about = "Build one Rust API for Python and TypeScript"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a Rust, Python, and TypeScript project.
    Init(InitArgs),
    /// Build the Python and TypeScript packages.
    Build(ProjectArgs),
    /// Rebuild when Rust or Cargo files change.
    Watch(ProjectArgs),
    /// Check that dist matches the Rust source.
    Check(ProjectArgs),
}

#[derive(Debug, Args)]
struct InitArgs {
    /// New project directory. The final path component is the package name.
    path: PathBuf,
}

#[derive(Debug, Args)]
struct ProjectArgs {
    /// Path to a workspace or binding Cargo.toml.
    #[arg(long, default_value = "Cargo.toml")]
    manifest_path: PathBuf,
}

/// Parse the command line and run the selected rspyts command.
///
/// # Errors
///
/// Returns an error when command input is invalid or a requested operation fails.
pub fn run() -> Result<()> {
    run_from(Cli::parse())
}

fn run_from(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Init(args) => {
            let report = init::create(&args.path)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Build(args) => {
            let project = Project::read(&args.manifest_path)?;
            let report = build(&project)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Check(args) => {
            let project = Project::read(&args.manifest_path)?;
            check(&project)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "status": "ok",
                    "output": project.output(),
                }))?
            );
        }
        Command::Watch(args) => {
            let project = Project::read(&args.manifest_path)?;
            build(&project)?;
            println!("rspyts is watching {}", project.workspace_root.display());
            let mut state = source_state(&project.workspace_root)?;
            loop {
                thread::sleep(Duration::from_millis(500));
                let next = source_state(&project.workspace_root)?;
                if next != state {
                    match build(&project) {
                        Ok(_) => {
                            state = next;
                            println!("rspyts rebuilt {}", project.output().display());
                        }
                        Err(error) => eprintln!("rspyts build failed: {error:#}"),
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rspyts::ir::{Manifest, TypeRef};

    use super::{output, project, typescript};

    #[test]
    fn validates_package_names() {
        assert!(project::validate_python_package("example.client").is_ok());
        assert!(project::validate_python_package("example-client").is_err());
        assert!(project::validate_typescript_package("@example/client").is_ok());
        assert!(project::validate_typescript_package("Example").is_err());
    }

    #[test]
    fn rejects_duplicate_public_names() {
        assert!(project::unique_public_names("Python", ["Thing", "Other"].into_iter()).is_ok());
        assert!(project::unique_public_names("Python", ["Thing", "Thing"].into_iter()).is_err());
    }

    #[test]
    fn describes_changed_byte_ranges() {
        assert_eq!(
            project::byte_difference(b"same--end", b"some++end!"),
            "expected 9 bytes, found 10 bytes; 4 bytes differ; first ranges: [1..2, 4..6]"
        );
    }

    #[test]
    fn uses_bigint_for_wide_types() {
        let manifest = Manifest {
            ir_version: 1,
            package_name: "fixture".into(),
            package_version: "1.0.0".into(),
            module_name: "native".into(),
            types: vec![],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        };
        assert_eq!(
            typescript::type_ref(
                &TypeRef::Int {
                    signed: false,
                    bits: 64,
                },
                &manifest,
            )
            .unwrap(),
            "bigint"
        );
    }

    #[test]
    fn ignores_python_cache_files_during_sync_checks() {
        let directory = tempfile::tempdir().unwrap();
        output::write(&directory.path().join("package.py"), "value = 1\n").unwrap();
        output::write(&directory.path().join("__pycache__/package.pyc"), "cache").unwrap();
        output::write(&directory.path().join("build/package.py"), "build").unwrap();
        output::write(
            &directory.path().join("package.egg-info/PKG-INFO"),
            "metadata",
        )
        .unwrap();

        let files = output::file_tree(directory.path()).unwrap();
        assert_eq!(files.keys().collect::<Vec<_>>(), [Path::new("package.py")]);
    }
}
