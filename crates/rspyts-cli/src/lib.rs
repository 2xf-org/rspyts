mod build;
mod config;
mod diff;
mod emit;
mod load;
mod resolve;
mod validate;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::build::{BuildOptions, BuildReport};
use crate::config::Project;
use crate::diff::ContractDiff;
use crate::load::load_contract;

const LOCK_VERSION: u32 = 2;

fn atomic_sibling(path: &Path, suffix: &str) -> Result<PathBuf> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| format!("{} has no valid UTF-8 file name", path.display()))?;
    let hidden_prefix = if name.starts_with('.') { "" } else { "." };
    Ok(parent.join(format!("{hidden_prefix}{name}.{suffix}")))
}

#[derive(Debug, Parser)]
#[command(
    name = "rspyts",
    version,
    about = "Compile one Rust API for Python and TypeScript"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Build every configured host package below .rspyts.
    Build(BuildArgs),
    /// Build and validate the contract, optionally against rspyts.lock.
    Check(CheckArgs),
    /// Accept the compiled contract as rspyts.lock.
    Lock(ProjectArgs),
    /// Print the compiled contract and its fingerprint.
    Inspect(ProjectArgs),
    /// Remove the generated .rspyts directory.
    Clean(ProjectArgs),
}

#[derive(Debug, Args)]
struct ProjectArgs {
    /// Path to rspyts.toml.
    #[arg(long, default_value = "rspyts.toml")]
    config: PathBuf,
}

#[derive(Debug, Args)]
struct BuildArgs {
    #[command(flatten)]
    project: ProjectArgs,
    /// Override the .rspyts staging directory (for package build frontends).
    #[arg(long)]
    staging: Option<PathBuf>,
    /// Build only one configured host package.
    #[arg(long, value_enum, default_value_t = BuildTarget::All)]
    target: BuildTarget,
}

#[derive(Debug, Args)]
struct CheckArgs {
    #[command(flatten)]
    project: ProjectArgs,
    /// Require the compiled contract to exactly match rspyts.lock.
    #[arg(long)]
    locked: bool,
    /// Check only one configured host package.
    #[arg(long, value_enum, default_value_t = BuildTarget::All)]
    target: BuildTarget,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum BuildTarget {
    Python,
    Typescript,
    #[default]
    All,
}

impl BuildTarget {
    pub(crate) fn includes_python(self) -> bool {
        matches!(self, Self::Python | Self::All)
    }

    pub(crate) fn includes_typescript(self) -> bool {
        matches!(self, Self::Typescript | Self::All)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Inspection<'a> {
    schema_version: u32,
    fingerprint: String,
    manifest: &'a rspyts::ir::Manifest,
    dependencies: &'a BTreeMap<String, LockedDependency>,
    hosts: &'a LockedHosts,
}

#[derive(Debug, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ContractLock {
    schema_version: u32,
    fingerprint: String,
    hosts: LockedHosts,
    dependencies: BTreeMap<String, LockedDependency>,
    manifest: rspyts::ir::Manifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LockedDependency {
    #[serde(rename = "crate")]
    pub owner: rspyts::ir::CargoPackageId,
    pub fingerprint: String,
    pub python: Option<String>,
    pub typescript: Option<String>,
    pub types: Vec<rspyts::ir::TypeDef>,
    pub errors: Vec<rspyts::ir::ErrorDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LockedHosts {
    pub python: Option<String>,
    pub typescript: Option<LockedTypeScriptHost>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LockedTypeScriptHost {
    pub package: String,
    pub mode: crate::config::TypeScriptMode,
}

pub fn run() -> Result<()> {
    run_from(Cli::parse())
}

fn run_from(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Build(args) => {
            let project = Project::read(&args.project.config)?;
            let report = build::build(
                &project,
                BuildOptions {
                    staging: args.staging,
                    target: args.target,
                },
            )?;
            print_json(&report)
        }
        Command::Check(args) => {
            let project = Project::read(&args.project.config)?;
            let report = build::build(
                &project,
                BuildOptions {
                    staging: None,
                    target: args.target,
                },
            )?;
            if args.locked {
                check_lock(&project, &report)?;
            }
            print_json(&report)
        }
        Command::Lock(args) => {
            let project = Project::read(&args.config)?;
            let loaded = load_contract(&project)?;
            validate::manifest(&loaded.manifest)?;
            let resolved = resolve::contract(&project, loaded.manifest)?;
            let lock = create_lock(resolved)?;
            write_atomic_file(&project.lock_path(), &pretty_json_line(&lock)?)?;
            print_json(&lock)
        }
        Command::Inspect(args) => {
            let project = Project::read(&args.config)?;
            let loaded = load_contract(&project)?;
            validate::manifest(&loaded.manifest)?;
            let resolved = resolve::contract(&project, loaded.manifest)?;
            let inspection = Inspection {
                schema_version: LOCK_VERSION,
                fingerprint: fingerprint(
                    &resolved.manifest,
                    &resolved.hosts,
                    &resolved.dependencies,
                )?,
                manifest: &resolved.manifest,
                dependencies: &resolved.dependencies,
                hosts: &resolved.hosts,
            };
            let rendered = serde_json::to_string_pretty(&inspection)?;
            println!("{rendered}");
            Ok(())
        }
        Command::Clean(args) => {
            let project = Project::read(&args.config)?;
            let output = project.output_dir();
            if output.exists() {
                fs::remove_dir_all(&output)
                    .with_context(|| format!("failed to remove {}", output.display()))?;
            }
            print_json(&CleanReport {
                schema_version: 1,
                removed: output,
            })
        }
    }
}

fn create_lock(resolved: resolve::ResolvedContract) -> Result<ContractLock> {
    Ok(ContractLock {
        schema_version: LOCK_VERSION,
        fingerprint: fingerprint(&resolved.manifest, &resolved.hosts, &resolved.dependencies)?,
        hosts: resolved.hosts,
        dependencies: resolved.dependencies,
        // Keep the complete compiler manifest in the lock. Package versions and
        // documentation are excluded only while hashing/comparing semantics.
        manifest: resolved.manifest,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CleanReport {
    schema_version: u32,
    removed: PathBuf,
}

fn check_lock(project: &Project, report: &BuildReport) -> Result<()> {
    let path = project.lock_path();
    let metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("locked check requires {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "contract lock must be a regular non-symlink file: {}",
            path.display()
        );
    }
    let source = fs::read_to_string(&path)
        .with_context(|| format!("locked check requires {}", path.display()))?;
    let lock: ContractLock = serde_json::from_str(&source)
        .with_context(|| format!("invalid contract lock {}", path.display()))?;
    if lock.schema_version != LOCK_VERSION {
        bail!(
            "unsupported rspyts.lock schema {}; expected {LOCK_VERSION}",
            lock.schema_version
        );
    }
    validate::manifest(&lock.manifest).context("rspyts.lock contains an invalid manifest")?;
    let locked_fingerprint = fingerprint(&lock.manifest, &lock.hosts, &lock.dependencies)?;
    if locked_fingerprint != lock.fingerprint {
        bail!(
            "contract lock fingerprint mismatch: recorded {}, computed {locked_fingerprint}",
            lock.fingerprint
        );
    }
    let semantic_current = semantic_manifest(&report.manifest);
    if semantic_manifest(&lock.manifest) == semantic_current
        && lock.dependencies == report.dependencies
        && lock.hosts == report.hosts
        && lock.fingerprint == report.fingerprint
    {
        return Ok(());
    }

    let diff = ContractDiff::between(&lock.manifest, &report.manifest);
    bail!(
        "compiled contract does not match {}\n{}",
        path.display(),
        diff
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FingerprintInput<'a> {
    schema_version: u32,
    hosts: &'a LockedHosts,
    manifest: rspyts::ir::Manifest,
    dependencies: &'a BTreeMap<String, LockedDependency>,
}

pub(crate) fn fingerprint(
    manifest: &rspyts::ir::Manifest,
    hosts: &LockedHosts,
    dependencies: &BTreeMap<String, LockedDependency>,
) -> Result<String> {
    let canonical = serde_json::to_vec(&FingerprintInput {
        schema_version: LOCK_VERSION,
        hosts,
        manifest: semantic_manifest(manifest),
        dependencies,
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(canonical)))
}

fn semantic_manifest(manifest: &rspyts::ir::Manifest) -> rspyts::ir::Manifest {
    let mut semantic = manifest.clone();
    semantic.crate_version.clear();
    semantic.types = semantic.types.iter().map(semantic_type_def).collect();
    semantic.errors = semantic.errors.iter().map(semantic_error_def).collect();
    for import in &mut semantic.imports {
        import.types = import.types.iter().map(semantic_type_def).collect();
        import.errors = import.errors.iter().map(semantic_error_def).collect();
        import
            .types
            .sort_by(|left, right| (&left.owner, &left.id).cmp(&(&right.owner, &right.id)));
        import
            .errors
            .sort_by(|left, right| (&left.owner, &left.id).cmp(&(&right.owner, &right.id)));
    }
    for function in &mut semantic.functions {
        function.docs = None;
    }
    for resource in &mut semantic.resources {
        resource.docs = None;
        for constructor in &mut resource.constructors {
            constructor.docs = None;
        }
        for method in &mut resource.methods {
            method.docs = None;
        }
    }
    for constant in &mut semantic.constants {
        constant.docs = None;
        canonicalize_json(&mut constant.value);
    }
    semantic
        .imports
        .sort_by(|left, right| left.owner.cmp(&right.owner));
    semantic
        .types
        .sort_by(|left, right| (&left.owner, &left.id).cmp(&(&right.owner, &right.id)));
    semantic
        .errors
        .sort_by(|left, right| (&left.owner, &left.id).cmp(&(&right.owner, &right.id)));
    semantic.functions.sort_by(|left, right| {
        (&left.owner, &left.host_name, &left.rust_name).cmp(&(
            &right.owner,
            &right.host_name,
            &right.rust_name,
        ))
    });
    semantic
        .resources
        .sort_by(|left, right| (&left.owner, &left.id).cmp(&(&right.owner, &right.id)));
    semantic.constants.sort_by(|left, right| {
        (&left.owner, &left.host_name, &left.rust_name).cmp(&(
            &right.owner,
            &right.host_name,
            &right.rust_name,
        ))
    });
    semantic
}

pub(crate) fn semantic_type_def(item: &rspyts::ir::TypeDef) -> rspyts::ir::TypeDef {
    let mut item = item.clone();
    item.docs = None;
    match &mut item.shape {
        rspyts::ir::TypeShape::Struct { fields } => clear_field_docs(fields),
        rspyts::ir::TypeShape::StringEnum { variants }
        | rspyts::ir::TypeShape::TaggedEnum { variants, .. } => {
            for variant in variants {
                variant.docs = None;
                clear_field_docs(&mut variant.fields);
            }
        }
        rspyts::ir::TypeShape::Alias { .. } => {}
    }
    item
}

pub(crate) fn semantic_error_def(item: &rspyts::ir::ErrorDef) -> rspyts::ir::ErrorDef {
    let mut item = item.clone();
    item.docs = None;
    for variant in &mut item.variants {
        variant.docs = None;
        clear_field_docs(&mut variant.fields);
    }
    item
}

fn canonicalize_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                canonicalize_json(item);
            }
        }
        serde_json::Value::Object(items) => {
            let mut sorted = std::mem::take(items).into_iter().collect::<Vec<_>>();
            sorted.sort_by(|left, right| left.0.cmp(&right.0));
            for (_, value) in &mut sorted {
                canonicalize_json(value);
            }
            items.extend(sorted);
        }
        _ => {}
    }
}

fn clear_field_docs(fields: &mut [rspyts::ir::FieldDef]) {
    for field in fields {
        field.docs = None;
    }
}

fn pretty_json_line<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

fn write_atomic_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)?;
    let temporary = atomic_sibling(path, &format!("tmp-{}", std::process::id()))?;
    fs::write(&temporary, bytes)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    let backup = atomic_sibling(path, &format!("old-{}", std::process::id()))?;
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    let had_existing = path.exists();
    if had_existing {
        fs::rename(path, &backup)
            .with_context(|| format!("failed to stage replacement of {}", path.display()))?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        if had_existing {
            let _ = fs::rename(&backup, path);
        }
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("failed to replace {}", path.display()));
    }
    if had_existing {
        fs::remove_file(backup)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use rspyts::ir::Manifest;

    use super::*;

    fn no_hosts() -> LockedHosts {
        LockedHosts {
            python: None,
            typescript: None,
        }
    }

    fn empty_dependencies() -> BTreeMap<String, LockedDependency> {
        BTreeMap::new()
    }

    fn test_fingerprint(manifest: &Manifest) -> String {
        fingerprint(manifest, &no_hosts(), &empty_dependencies()).unwrap()
    }

    #[test]
    fn hidden_lock_uses_single_dot_siblings_and_is_replaced_atomically() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-lock-atomic-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let lock = root.join(".rspyts.lock");
        fs::write(&lock, "old").unwrap();
        let temporary = atomic_sibling(&lock, &format!("tmp-{}", std::process::id())).unwrap();
        let backup = atomic_sibling(&lock, &format!("old-{}", std::process::id())).unwrap();
        assert_eq!(
            temporary.file_name().unwrap().to_string_lossy(),
            format!(".rspyts.lock.tmp-{}", std::process::id())
        );
        assert_eq!(
            backup.file_name().unwrap().to_string_lossy(),
            format!(".rspyts.lock.old-{}", std::process::id())
        );

        write_atomic_file(&lock, b"new").unwrap();

        assert_eq!(fs::read_to_string(&lock).unwrap(), "new");
        assert!(!temporary.exists());
        assert!(!backup.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn visible_atomic_outputs_are_hidden_siblings() {
        let path = Path::new("rspyts.lock");
        assert_eq!(
            atomic_sibling(path, "tmp-123").unwrap(),
            PathBuf::from(".rspyts.lock.tmp-123")
        );
        assert_eq!(
            atomic_sibling(path, "old-123").unwrap(),
            PathBuf::from(".rspyts.lock.old-123")
        );
    }

    #[test]
    fn build_and_check_parse_host_targets() {
        let build = Cli::try_parse_from(["rspyts", "build", "--target", "python"])
            .expect("python build target should parse");
        assert!(matches!(
            build.command,
            Command::Build(BuildArgs {
                target: BuildTarget::Python,
                ..
            })
        ));

        let check = Cli::try_parse_from(["rspyts", "check", "--target", "typescript"])
            .expect("TypeScript check target should parse");
        assert!(matches!(
            check.command,
            Command::Check(CheckArgs {
                target: BuildTarget::Typescript,
                ..
            })
        ));

        let default = Cli::try_parse_from(["rspyts", "build"]).unwrap();
        assert!(matches!(
            default.command,
            Command::Build(BuildArgs {
                target: BuildTarget::All,
                ..
            })
        ));
    }

    #[test]
    fn fingerprints_are_stable() {
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "sample".into(),
            imports: vec![],
            types: vec![],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        };
        assert_eq!(test_fingerprint(&manifest), test_fingerprint(&manifest));
        assert!(test_fingerprint(&manifest).starts_with("sha256:"));
    }

    #[test]
    fn lock_retains_the_compiled_package_version() {
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.2.3".into(),
            module_name: "sample".into(),
            imports: vec![],
            types: vec![],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        };
        let lock = create_lock(resolve::ResolvedContract {
            manifest,
            dependencies: BTreeMap::new(),
            hosts: no_hosts(),
            foreign_types: BTreeMap::new(),
            foreign_errors: BTreeMap::new(),
        })
        .unwrap();

        assert_eq!(lock.manifest.crate_version, "1.2.3");
        let encoded = serde_json::to_value(&lock).unwrap();
        assert_eq!(encoded["manifest"]["crateVersion"], "1.2.3");

        let first = pretty_json_line(&lock).unwrap();
        let second = pretty_json_line(&lock).unwrap();
        assert_eq!(first, second);
        assert!(first.ends_with(b"\n"));
        assert!(String::from_utf8_lossy(&first).contains("\n  \"schemaVersion\""));
        let decoded: ContractLock = serde_json::from_slice(&first).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
    }

    #[test]
    fn documentation_and_package_version_do_not_change_semantic_fingerprint() {
        let mut before = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "sample".into(),
            imports: vec![],
            types: vec![rspyts::ir::TypeDef {
                owner: rspyts::ir::CargoPackageId::new("sample"),
                id: "sample::Value".into(),
                name: "Value".into(),
                docs: None,
                shape: rspyts::ir::TypeShape::Struct { fields: vec![] },
            }],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        };
        let expected = test_fingerprint(&before);
        before.crate_version = "1.1.0".into();
        before.types[0].docs = Some("Better docs".into());
        assert_eq!(test_fingerprint(&before), expected);
    }

    #[test]
    fn locked_check_accepts_documentation_and_package_version_changes() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-lock-semantic-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("rust/src")).unwrap();
        fs::write(
            root.join("rust/Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(root.join("rust/src/lib.rs"), "").unwrap();
        fs::write(
            root.join("rspyts.toml"),
            "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
        )
        .unwrap();
        let project = Project::read(&root.join("rspyts.toml")).unwrap();
        let mut manifest = Manifest {
            ir_version: 4,
            crate_name: "fixture".into(),
            crate_version: "1.0.0".into(),
            module_name: "fixture".into(),
            imports: vec![],
            types: vec![rspyts::ir::TypeDef {
                owner: rspyts::ir::CargoPackageId::new("fixture"),
                id: "fixture::Value".into(),
                name: "Value".into(),
                docs: None,
                shape: rspyts::ir::TypeShape::Struct { fields: vec![] },
            }],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        };
        let hosts = LockedHosts {
            python: None,
            typescript: Some(LockedTypeScriptHost {
                package: "fixture".into(),
                mode: crate::config::TypeScriptMode::Static,
            }),
        };
        let dependencies = empty_dependencies();
        let mut lock = ContractLock {
            schema_version: LOCK_VERSION,
            fingerprint: fingerprint(&manifest, &hosts, &dependencies).unwrap(),
            hosts: hosts.clone(),
            dependencies: dependencies.clone(),
            manifest: manifest.clone(),
        };
        fs::write(
            project.lock_path(),
            pretty_json_line(&lock).expect("serialize lock"),
        )
        .unwrap();

        manifest.crate_version = "2.0.0".into();
        manifest.types[0].docs = Some("New documentation".into());
        let report = BuildReport {
            schema_version: 1,
            status: "ok",
            fingerprint: fingerprint(&manifest, &hosts, &dependencies).unwrap(),
            contract: root.join(".rspyts/contract.json"),
            staging: root.join(".rspyts"),
            python: None,
            typescript: None,
            manifest,
            dependencies,
            hosts,
        };
        check_lock(&project, &report).unwrap();

        lock.fingerprint = "sha256:tampered".into();
        fs::write(
            project.lock_path(),
            pretty_json_line(&lock).expect("serialize tampered lock"),
        )
        .unwrap();
        assert!(
            check_lock(&project, &report)
                .unwrap_err()
                .to_string()
                .contains("fingerprint mismatch")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
