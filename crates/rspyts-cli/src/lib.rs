mod build;
mod config;
mod diff;
mod emit;
mod load;
mod resolve;
mod validate;

use std::collections::BTreeMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use atomicwrites::replace_atomic;
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::build::{BuildOptions, BuildReport};
use crate::config::Project;
use crate::diff::ContractDiff;
use crate::load::load_contract;

const LOCK_VERSION: u32 = 3;
static ATOMIC_FILE_ID: AtomicU64 = AtomicU64::new(0);

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
    pub crate_version: String,
    pub fingerprint: String,
    pub python: Option<String>,
    pub typescript: Option<LockedTypeScriptHost>,
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
                    target: args.target,
                },
            )?;
            print_json(&report)
        }
        Command::Check(args) => {
            let project = Project::read(&args.project.config)?;
            let prepared = build::prepare(
                &project,
                BuildOptions {
                    target: args.target,
                },
            )?;
            if args.locked {
                check_lock(&project, prepared.report())?;
            }
            let report = prepared.commit()?;
            print_json(&report)
        }
        Command::Lock(args) => {
            let project = Project::read(&args.config)?;
            let _lock = build::lock_project(&project)?;
            let loaded = load_contract(&project)?;
            validate::manifest(&loaded.manifest)?;
            let resolved = resolve::contract(&project, loaded.manifest)?;
            let lock = create_lock(resolved)?;
            write_atomic_file(&project.lock_path(), &compact_json_line(&lock)?)?;
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
            let output = build::clean(&project)?;
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
    if lock.manifest.crate_version != report.manifest.crate_version {
        bail!(
            "compiled contract crate version `{}` does not match locked version `{}`",
            report.manifest.crate_version,
            lock.manifest.crate_version
        );
    }
    let semantic_locked = semantic_manifest(&lock.manifest);
    let semantic_current = semantic_manifest(&report.manifest);
    if semantic_locked == semantic_current {
        let metadata_changes = lock_metadata_changes(&lock, report);
        if !metadata_changes.is_empty() {
            bail!(
                "compiled contract lock metadata does not match {}\n{}",
                path.display(),
                metadata_changes
                    .iter()
                    .map(|change| format!("  - {change}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        if lock.fingerprint != report.fingerprint {
            bail!(
                "compiled contract fingerprint {} does not match locked fingerprint {}",
                report.fingerprint,
                lock.fingerprint
            );
        }
        return Ok(());
    }

    let diff = ContractDiff::between(&lock.manifest, &report.manifest);
    bail!(
        "compiled contract does not match {}\n{}",
        path.display(),
        diff
    )
}

fn lock_metadata_changes(lock: &ContractLock, report: &BuildReport) -> Vec<String> {
    let mut changes = Vec::new();
    if lock.hosts.python != report.hosts.python {
        changes.push(format!(
            "root Python host changed from {:?} to {:?}",
            lock.hosts.python, report.hosts.python
        ));
    }
    if lock.hosts.typescript != report.hosts.typescript {
        changes.push(format!(
            "root TypeScript host changed from {:?} to {:?}",
            lock.hosts.typescript, report.hosts.typescript
        ));
    }

    let aliases = lock
        .dependencies
        .keys()
        .chain(report.dependencies.keys())
        .collect::<std::collections::BTreeSet<_>>();
    for alias in aliases {
        match (lock.dependencies.get(alias), report.dependencies.get(alias)) {
            (None, Some(_)) => changes.push(format!("added dependency `{alias}`")),
            (Some(_), None) => changes.push(format!("removed dependency `{alias}`")),
            (Some(locked), Some(current)) => {
                if locked.owner != current.owner {
                    changes.push(format!(
                        "dependency `{alias}` Cargo owner changed from `{}` to `{}`",
                        locked.owner, current.owner
                    ));
                }
                if locked.crate_version != current.crate_version {
                    changes.push(format!(
                        "dependency `{alias}` crate version changed from `{}` to `{}`",
                        locked.crate_version, current.crate_version
                    ));
                }
                if locked.fingerprint != current.fingerprint {
                    changes.push(format!(
                        "dependency `{alias}` fingerprint changed from `{}` to `{}`",
                        locked.fingerprint, current.fingerprint
                    ));
                }
                if locked.python != current.python {
                    changes.push(format!(
                        "dependency `{alias}` Python host changed from {:?} to {:?}",
                        locked.python, current.python
                    ));
                }
                if locked.typescript != current.typescript {
                    changes.push(format!(
                        "dependency `{alias}` TypeScript host changed from {:?} to {:?}",
                        locked.typescript, current.typescript
                    ));
                }
                if locked.types != current.types {
                    changes.push(format!("dependency `{alias}` type snapshot changed"));
                }
                if locked.errors != current.errors {
                    changes.push(format!("dependency `{alias}` error snapshot changed"));
                }
            }
            (None, None) => unreachable!(),
        }
    }
    changes
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
        (
            &left.owner,
            &left.host_name,
            &left.rust_name,
            semantic_target_rank(left.target),
        )
            .cmp(&(
                &right.owner,
                &right.host_name,
                &right.rust_name,
                semantic_target_rank(right.target),
            ))
    });
    semantic
        .resources
        .sort_by(|left, right| (&left.owner, &left.id).cmp(&(&right.owner, &right.id)));
    semantic.constants.sort_by(|left, right| {
        (
            &left.owner,
            &left.host_name,
            &left.rust_name,
            semantic_target_rank(left.target),
        )
            .cmp(&(
                &right.owner,
                &right.host_name,
                &right.rust_name,
                semantic_target_rank(right.target),
            ))
    });
    semantic
}

const fn semantic_target_rank(target: rspyts::ir::Target) -> u8 {
    match target {
        rspyts::ir::Target::Both => 0,
        rspyts::ir::Target::Python => 1,
        rspyts::ir::Target::Typescript => 2,
        rspyts::ir::Target::Static => 3,
    }
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

fn compact_json_line<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)?;
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
    validate_atomic_destination(path)?;
    let (temporary, mut file) = create_atomic_sibling_file(path)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(&temporary);
        return Err(error)
            .with_context(|| format!("failed to write atomic output {}", path.display()));
    }
    drop(file);
    if let Err(error) = validate_atomic_destination(path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = replace_atomic(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error)
            .with_context(|| format!("failed to commit atomic output {}", path.display()));
    }
    Ok(())
}

fn create_atomic_sibling_file(path: &Path) -> Result<(PathBuf, fs::File)> {
    for _ in 0..1024 {
        let id = ATOMIC_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let candidate = atomic_sibling(path, &format!("tmp-{}-{id}", std::process::id()))?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to reserve {}", candidate.display()));
            }
        }
    }
    bail!("failed to reserve a temporary file for {}", path.display())
}

fn validate_atomic_destination(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("refusing to replace symlink {}", path.display())
        }
        Ok(metadata) if !metadata.is_file() => {
            bail!("refusing to replace non-file {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to inspect atomic output {}", path.display())),
    }
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

    fn test_lock(crate_version: &str) -> ContractLock {
        let manifest = Manifest {
            ir_version: rspyts::ir::IR_VERSION,
            crate_name: "sample".into(),
            crate_version: crate_version.into(),
            module_name: "sample".into(),
            imports: vec![],
            types: vec![],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        };
        create_lock(resolve::ResolvedContract {
            manifest,
            dependencies: BTreeMap::new(),
            hosts: no_hosts(),
            foreign_types: BTreeMap::new(),
            foreign_errors: BTreeMap::new(),
        })
        .unwrap()
    }

    #[test]
    fn lock_replacement_preserves_existing_temporary_siblings() {
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
        let first_collision = root.join(".rspyts.lock.tmp-123-456");
        let second_collision = root.join(".rspyts.lock.tmp-123-457");
        fs::write(&first_collision, "authored temporary collision").unwrap();
        fs::write(&second_collision, "another temporary collision").unwrap();
        write_atomic_file(&lock, b"new").unwrap();

        assert_eq!(fs::read_to_string(&lock).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(&first_collision).unwrap(),
            "authored temporary collision"
        );
        assert_eq!(
            fs::read_to_string(&second_collision).unwrap(),
            "another temporary collision"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_lock_rejects_a_directory_without_mutating_it() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-lock-directory-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let lock = root.join("rspyts.lock");
        fs::create_dir_all(&lock).unwrap();
        fs::write(lock.join("authored"), "keep").unwrap();

        let error = write_atomic_file(&lock, b"new").unwrap_err();
        assert!(error.to_string().contains("non-file"));
        assert_eq!(fs::read_to_string(lock.join("authored")).unwrap(), "keep");

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn atomic_lock_rejects_a_symlink_without_mutating_its_target() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "rspyts-lock-symlink-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let target = root.join("authored");
        let lock = root.join("rspyts.lock");
        fs::write(&target, "keep").unwrap();
        symlink(&target, &lock).unwrap();

        let error = write_atomic_file(&lock, b"new").unwrap_err();
        assert!(error.to_string().contains("symlink"));
        assert!(
            fs::symlink_metadata(&lock)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_atomic_lock_writes_remain_complete_regular_files() {
        use std::sync::{Arc, Barrier};

        let root = std::env::temp_dir().join(format!(
            "rspyts-lock-concurrent-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let lock = Arc::new(root.join("rspyts.lock"));
        let barrier = Arc::new(Barrier::new(8));
        let writers = (0..8)
            .map(|index| {
                let lock = Arc::clone(&lock);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let value = format!("complete-{index}\n");
                    barrier.wait();
                    write_atomic_file(&lock, value.as_bytes()).unwrap();
                })
            })
            .collect::<Vec<_>>();
        for writer in writers {
            writer.join().unwrap();
        }

        let value = fs::read_to_string(lock.as_ref()).unwrap();
        assert!(
            (0..8).any(|index| value == format!("complete-{index}\n")),
            "unexpected partial lock: {value:?}"
        );
        assert!(fs::symlink_metadata(lock.as_ref()).unwrap().is_file());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn visible_atomic_outputs_are_hidden_siblings() {
        let path = Path::new("generated");
        assert_eq!(
            atomic_sibling(path, "tmp-123").unwrap(),
            PathBuf::from(".generated.tmp-123")
        );
        assert_eq!(
            atomic_sibling(path, "old-123").unwrap(),
            PathBuf::from(".generated.old-123")
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
    fn fingerprints_ignore_order_for_disjoint_same_name_host_exports() {
        let owner = rspyts::ir::CargoPackageId::new("sample");
        let function = |target| rspyts::ir::FunctionDef {
            owner: owner.clone(),
            rust_name: "shared".into(),
            host_name: "shared".into(),
            docs: None,
            target,
            params: vec![],
            returns: rspyts::ir::TypeRef::Unit,
            error: None,
        };
        let constant = |target| rspyts::ir::ConstantDef {
            owner: owner.clone(),
            rust_name: "SHARED".into(),
            host_name: "SHARED".into(),
            docs: None,
            target,
            ty: rspyts::ir::TypeRef::String,
            value: serde_json::Value::String("value".into()),
        };
        let mut left = Manifest {
            ir_version: rspyts::ir::IR_VERSION,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "sample".into(),
            imports: vec![],
            types: vec![],
            errors: vec![],
            functions: vec![
                function(rspyts::ir::Target::Typescript),
                function(rspyts::ir::Target::Python),
            ],
            resources: vec![],
            constants: vec![
                constant(rspyts::ir::Target::Typescript),
                constant(rspyts::ir::Target::Python),
            ],
        };
        let mut right = left.clone();
        right.functions.reverse();
        right.constants.reverse();

        assert_eq!(test_fingerprint(&left), test_fingerprint(&right));
        left.functions.reverse();
        left.constants.reverse();
        assert_eq!(test_fingerprint(&left), test_fingerprint(&right));
    }

    #[test]
    fn fingerprints_distinguish_dynamic_bytes_and_every_fixed_length() {
        let manifest = |target| Manifest {
            ir_version: rspyts::ir::IR_VERSION,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "sample".into(),
            imports: vec![],
            types: vec![rspyts::ir::TypeDef {
                owner: rspyts::ir::CargoPackageId::new("sample"),
                id: "sample::Digest".into(),
                name: "Digest".into(),
                docs: None,
                shape: rspyts::ir::TypeShape::Alias { target },
            }],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        };
        let dynamic = test_fingerprint(&manifest(rspyts::ir::TypeRef::Bytes));
        let fixed_four = test_fingerprint(&manifest(rspyts::ir::TypeRef::FixedBytes { length: 4 }));
        let fixed_eight =
            test_fingerprint(&manifest(rspyts::ir::TypeRef::FixedBytes { length: 8 }));

        assert_ne!(dynamic, fixed_four);
        assert_ne!(fixed_four, fixed_eight);
        assert_ne!(dynamic, fixed_eight);
    }

    #[test]
    fn lock_retains_the_compiled_package_version() {
        let lock = test_lock("1.2.3");

        assert_eq!(lock.manifest.crate_version, "1.2.3");
        let encoded = serde_json::to_value(&lock).unwrap();
        assert_eq!(encoded["manifest"]["crateVersion"], "1.2.3");
    }

    #[test]
    fn lock_serialization_is_compact_deterministic_and_roundtrips() {
        let lock = test_lock("1.2.3");
        let first = compact_json_line(&lock).unwrap();
        let second = compact_json_line(&lock).unwrap();
        assert_eq!(first, second);
        assert!(first.ends_with(b"\n"));
        assert_eq!(first.iter().filter(|byte| **byte == b'\n').count(), 1);
        assert_eq!(
            &first[..first.len() - 1],
            serde_json::to_vec(&lock).unwrap()
        );
        let decoded: ContractLock = serde_json::from_slice(&first).unwrap();
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::to_value(lock).unwrap()
        );
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
    fn dependency_package_version_changes_the_root_fingerprint() {
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
        let mut dependency = LockedDependency {
            owner: rspyts::ir::CargoPackageId::new("dependency"),
            crate_version: "1.0.0".into(),
            fingerprint: "sha256:dependency".into(),
            python: Some("example.dependency".into()),
            typescript: Some(LockedTypeScriptHost {
                package: "@example/dependency".into(),
                mode: crate::config::TypeScriptMode::Static,
            }),
            types: vec![],
            errors: vec![],
        };
        let before = fingerprint(
            &manifest,
            &no_hosts(),
            &BTreeMap::from([("dependency".into(), dependency.clone())]),
        )
        .unwrap();

        dependency.crate_version = "1.0.1".into();
        let after = fingerprint(
            &manifest,
            &no_hosts(),
            &BTreeMap::from([("dependency".into(), dependency)]),
        )
        .unwrap();

        assert_ne!(before, after);
    }

    #[test]
    fn locked_check_accepts_documentation_but_rejects_package_version_changes() {
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
            ir_version: rspyts::ir::IR_VERSION,
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
            compact_json_line(&lock).expect("serialize lock"),
        )
        .unwrap();

        manifest.crate_version = "2.0.0".into();
        manifest.types[0].docs = Some("New documentation".into());
        let report = BuildReport {
            schema_version: 1,
            status: "ok",
            fingerprint: fingerprint(&manifest, &hosts, &dependencies).unwrap(),
            contract: root.join(".rspyts/contract.json"),
            output: root.join(".rspyts"),
            python: None,
            typescript: None,
            manifest,
            dependencies,
            hosts,
        };
        assert_eq!(lock.fingerprint, report.fingerprint);
        let error = check_lock(&project, &report).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("crate version `2.0.0` does not match locked version `1.0.0`")
        );

        let mut report = report;
        report.manifest.crate_version = lock.manifest.crate_version.clone();
        check_lock(&project, &report).unwrap();

        let locked_dependency = LockedDependency {
            owner: rspyts::ir::CargoPackageId::new("dependency"),
            crate_version: "1.0.0".into(),
            fingerprint: "sha256:dependency".into(),
            python: Some("example.dependency".into()),
            typescript: Some(LockedTypeScriptHost {
                package: "@example/dependency".into(),
                mode: crate::config::TypeScriptMode::Static,
            }),
            types: vec![],
            errors: vec![],
        };
        lock.dependencies
            .insert("dependency".into(), locked_dependency.clone());
        lock.fingerprint = fingerprint(&lock.manifest, &lock.hosts, &lock.dependencies).unwrap();
        fs::write(
            project.lock_path(),
            compact_json_line(&lock).expect("serialize lock with dependency"),
        )
        .unwrap();

        let mut current_dependency = locked_dependency;
        current_dependency.crate_version = "2.0.0".into();
        report
            .dependencies
            .insert("dependency".into(), current_dependency.clone());
        report.fingerprint =
            fingerprint(&report.manifest, &report.hosts, &report.dependencies).unwrap();
        let error = check_lock(&project, &report).unwrap_err().to_string();
        assert!(error.contains("dependency `dependency` crate version changed"));
        assert!(!error.contains("no semantic changes"));

        current_dependency.crate_version = "1.0.0".into();
        current_dependency.python = Some("example.renamed".into());
        current_dependency.typescript = Some(LockedTypeScriptHost {
            package: "@example/renamed".into(),
            mode: crate::config::TypeScriptMode::Wasm,
        });
        report
            .dependencies
            .insert("dependency".into(), current_dependency);
        report.fingerprint =
            fingerprint(&report.manifest, &report.hosts, &report.dependencies).unwrap();
        let error = check_lock(&project, &report).unwrap_err().to_string();
        assert!(error.contains("dependency `dependency` Python host changed"));
        assert!(error.contains("dependency `dependency` TypeScript host changed"));
        assert!(!error.contains("no semantic changes"));

        lock.fingerprint = "sha256:tampered".into();
        fs::write(
            project.lock_path(),
            compact_json_line(&lock).expect("serialize tampered lock"),
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
