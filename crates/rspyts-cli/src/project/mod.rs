//! Cargo project resolution, bridge compilation, and package generation.
//!
//! A [`Project`] is an immutable snapshot of Cargo metadata and application
//! settings for one command invocation. Builds use a synthetic `cdylib` bridge
//! outside the source tree to link every selected package, discover the
//! contract on the native target, and compile the same exports for Wasm.

use std::ffi::{CStr, c_char};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::{Context, Result, bail};
use libloading::{Library, Symbol};
use rspyts::ir::{Manifest, Namespace};
use serde_json::Value;
use tempfile::TempDir;

use crate::cargo::{self, Metadata as CargoMetadata, Package as CargoPackage};
use crate::config::Config;
use crate::contract::{
    named_identities, namespace_refs, namespaces, tagged_variant_name, type_namespace, type_refs,
};
use crate::output::{
    check_generated_files, project_lock, reconcile_generated_files, source_fingerprint,
    write_generated_gitignores,
};
use crate::{python, typescript};

mod validation;

/// Cross-language validation and identifier helpers used by generators.
pub(crate) use validation::*;

const CONTRACT_SYMBOL: &str = "rspyts_discovery_v1_contract";
const FREE_SYMBOL: &str = "rspyts_discovery_v1_contract_free";

/// Fully resolved inputs for one CLI invocation.
///
/// The snapshot is constructed once from `rspyts.toml` and Cargo metadata.
/// Commands that perform a long-running build re-read the configuration before
/// publication so a concurrent edit cannot produce output from mixed inputs.
#[derive(Debug, Clone)]
pub(super) struct Project {
    /// Directory containing `rspyts.toml` and the primary Cargo package.
    pub(super) root: PathBuf,
    /// Cargo workspace root used for source fingerprinting and path remapping.
    pub(super) workspace_root: PathBuf,
    /// Cargo target directory shared by the generated bridge builds.
    target_directory: PathBuf,
    /// Ordered application packages linked into the bridge.
    linked_packages: Vec<LinkedPackage>,
    /// Exact `rspyts` dependency used by every linked package.
    rspyts_dependency: ResolvedDependency,
    /// Public application/package name.
    pub(super) package_name: String,
    /// Version inherited from the primary Cargo package.
    pub(super) package_version: String,
    /// Python distribution and import package name.
    pub(super) python_package: String,
    /// npm package name.
    pub(super) typescript_package: String,
    /// User-owned Python project root.
    python_source: PathBuf,
    /// User-owned TypeScript project root.
    typescript_source: PathBuf,
    /// Configuration snapshot used to construct this project.
    pub(super) config: Config,
}

/// Cargo package linked into the synthetic application bridge.
#[derive(Debug, Clone)]
struct LinkedPackage {
    name: String,
    manifest: PathBuf,
}

/// Reproducible dependency specification for the generated bridge manifest.
#[derive(Debug, Clone)]
enum ResolvedDependency {
    Path(PathBuf),
    CratesIo { version: String },
}

impl Project {
    /// Resolve configuration and Cargo metadata for the package at `path`.
    pub(super) fn read(path: &Path) -> Result<Self> {
        let config = Config::read(path)?;
        let requested_manifest = config.root().join("Cargo.toml");
        let requested_manifest = requested_manifest
            .canonicalize()
            .with_context(|| format!("cannot find Cargo.toml beside {}", config.path.display()))?;
        let metadata = cargo::metadata(&requested_manifest, false)?;
        let selected = select_application_packages(
            &metadata,
            &requested_manifest,
            &config.application.additional_packages,
        )?;
        let primary = selected
            .first()
            .copied()
            .context("an rspyts application must select a primary Cargo package")?;
        let rspyts_dependency = resolve_rspyts_dependency(&metadata, &selected)?;

        let primary_package_name = primary.name.clone();
        let package_name = config
            .application
            .name
            .clone()
            .unwrap_or_else(|| primary_package_name.clone());
        validate_application_name(&package_name)?;
        let package_version = primary.version.clone();
        let python_package = config
            .application
            .python_package
            .clone()
            .unwrap_or_else(|| package_name.replace('-', "_"));
        let typescript_package = config
            .application
            .typescript_package
            .clone()
            .unwrap_or_else(|| package_name.clone());
        validate_python_package(&python_package)?;
        validate_typescript_package(&typescript_package)?;

        let root = config.root().to_path_buf();
        let workspace_root = metadata.workspace_root.clone();
        let target_directory = metadata.target_directory.clone();
        let python_source = root.join("src-py");
        let typescript_source = root.join("src-ts");
        let linked_packages = selected
            .iter()
            .map(|package| LinkedPackage {
                name: package.name.clone(),
                manifest: package.manifest_path.clone(),
            })
            .collect();

        Ok(Self {
            root,
            workspace_root,
            target_directory,
            linked_packages,
            rspyts_dependency,
            package_name,
            package_version,
            python_package,
            typescript_package,
            python_source,
            typescript_source,
            config,
        })
    }

    /// Return the Python source project root.
    pub(super) fn python_source(&self) -> &Path {
        &self.python_source
    }

    /// Return the TypeScript source project root.
    pub(super) fn typescript_source(&self) -> &Path {
        &self.typescript_source
    }

    /// Return the authoritative configuration path.
    pub(super) fn config_path(&self) -> &Path {
        &self.config.path
    }
}

/// Machine-readable result emitted after a successful build.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BuildReport {
    status: &'static str,
    python_source: PathBuf,
    typescript_source: PathBuf,
    python_package: String,
    typescript_package: String,
}

/// Generate, validate, and transactionally publish both language packages.
pub(super) fn build(project: &Project) -> Result<BuildReport> {
    let _lock = project_lock(project)?;
    current_config(project)?;
    let (generated, manifest) = generate(project)?;
    python::validate_project(project, &manifest)?;
    typescript::validate_project(project)?;
    let config = current_config(project)?;
    reconcile_generated_files(
        generated.path(),
        &project.root,
        &source_fingerprint(&project.workspace_root, &config)?,
        &config,
    )?;
    Ok(BuildReport {
        status: "ok",
        python_source: project.python_source.clone(),
        typescript_source: project.typescript_source.clone(),
        python_package: project.python_package.clone(),
        typescript_package: project.typescript_package.clone(),
    })
}

/// Verify that every owned output matches the current Rust sources.
pub(super) fn check(project: &Project) -> Result<()> {
    let _lock = project_lock(project)?;
    current_config(project)?;
    let (generated, _) = generate(project)?;
    let config = current_config(project)?;
    check_generated_files(
        generated.path(),
        &project.root,
        &source_fingerprint(&project.workspace_root, &config)?,
        &config,
    )
}

/// Re-read configuration and reject application changes made mid-command.
fn current_config(project: &Project) -> Result<Config> {
    let config = Config::read(project.config_path())?;
    if config.application != project.config.application {
        bail!("[application] in rspyts.toml changed during the command; run it again");
    }
    Ok(config)
}

/// Build both targets and stage generated packages in a temporary directory.
fn generate(project: &Project) -> Result<(TempDir, Manifest)> {
    let temporary = tempfile::Builder::new()
        .prefix(".rspyts-")
        .tempdir_in(&project.root)?;
    let bridge = prepare_bridge(project)?;
    let native = compile(project, &bridge, CompileKind::Native)?;
    let manifest = read_contract(&native, &bridge.package_name)?;
    validate_contract(project, &manifest)?;
    let wasm = compile(project, &bridge, CompileKind::Wasm)?;

    python::emit(project, &manifest, &native, temporary.path())?;
    typescript::emit(project, &manifest, &wasm, temporary.path())?;
    if project.config.application.gitignore {
        write_generated_gitignores(temporary.path())?;
    }
    Ok((temporary, manifest))
}

// Bridge generation and compilation

/// Target-specific form of the generated application bridge.
#[derive(Clone, Copy)]
enum CompileKind {
    Native,
    Wasm,
}

/// Synthetic Cargo package that links all selected application crates.
struct Bridge {
    manifest: PathBuf,
    package_id: String,
    package_name: String,
}

/// Materialize the stable synthetic bridge package under Cargo's target tree.
fn prepare_bridge(project: &Project) -> Result<Bridge> {
    let key = stable_key(project.config_path().to_string_lossy().as_bytes());
    let root = project
        .target_directory
        .join("rspyts")
        .join(format!("{key:016x}"))
        .join("bridge");
    let source_root = root.join("src");
    fs::create_dir_all(&source_root)?;
    let package_name = format!("rspyts-bridge-{key:016x}");

    let mut manifest = format!(
        "[package]\nname = {}\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\n",
        toml_string(&package_name)
    );
    writeln!(
        manifest,
        "rspyts = {}",
        bridge_dependency(&project.rspyts_dependency)
    )?;
    for (index, package) in project.linked_packages.iter().enumerate() {
        let root = package
            .manifest
            .parent()
            .context("linked Cargo manifest has no parent")?;
        writeln!(
            manifest,
            "rspyts_application_{index} = {{ package = {}, path = {} }}",
            toml_string(&package.name),
            toml_string(&root.to_string_lossy())
        )?;
    }
    manifest.push_str("\n[workspace]\n");

    let mut source = String::from("rspyts::application!(\n");
    for index in 0..project.linked_packages.len() {
        writeln!(source, "    rspyts_application_{index},")?;
    }
    source.push_str(");\n");
    write_if_changed(&root.join("Cargo.toml"), &manifest)?;
    write_if_changed(&source_root.join("lib.rs"), &source)?;

    let bridge_manifest = root.join("Cargo.toml");
    let metadata = cargo::metadata(&bridge_manifest, true)
        .context("failed to inspect the generated rspyts bridge")?;
    let package = metadata
        .packages
        .first()
        .context("generated rspyts bridge metadata has no package")?;
    Ok(Bridge {
        manifest: bridge_manifest,
        package_id: package.id.clone(),
        package_name,
    })
}

/// Compile one bridge target and return Cargo's reported `cdylib` artifact.
fn compile(project: &Project, bridge: &Bridge, kind: CompileKind) -> Result<PathBuf> {
    let (feature, label) = match kind {
        CompileKind::Native => (Some("rspyts/python-extension"), "Python"),
        CompileKind::Wasm => (None, "WebAssembly"),
    };
    let mut command = ProcessCommand::new(cargo::executable());
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(&bridge.manifest)
        .arg("--release")
        .arg("--message-format=json-render-diagnostics");
    if let Some(feature) = feature {
        command.arg("--features").arg(feature);
    }
    if matches!(kind, CompileKind::Wasm) {
        command.args(["--target", "wasm32-unknown-unknown"]);
    }
    let mut flags = std::env::var("RUSTFLAGS").unwrap_or_default();
    append_rust_flag(
        &mut flags,
        &format!(
            "--remap-path-prefix={}=/workspace",
            project.workspace_root.display()
        ),
    );
    if let Some(cargo_home) = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
    {
        append_rust_flag(
            &mut flags,
            &format!("--remap-path-prefix={}=/cargo", cargo_home.display()),
        );
    }
    if matches!(kind, CompileKind::Native) && cfg!(target_os = "macos") {
        append_rust_flag(&mut flags, "-C");
        append_rust_flag(&mut flags, "link-arg=-undefined");
        append_rust_flag(&mut flags, "-C");
        append_rust_flag(&mut flags, "link-arg=dynamic_lookup");
        append_rust_flag(&mut flags, "-C");
        append_rust_flag(&mut flags, "link-arg=-Wl,-install_name,@rpath/native.so");
    }
    command
        .env("RUSTFLAGS", flags)
        .env("CARGO_TARGET_DIR", &project.target_directory)
        .env("RSPYTS_APPLICATION_NAME", &project.package_name)
        .env("RSPYTS_APPLICATION_VERSION", &project.package_version);
    let output = command
        .output()
        .with_context(|| format!("failed to compile {label}"))?;
    if !output.status.success() {
        bail!(
            "Cargo failed to compile the {label}\n{}{}",
            cargo_diagnostics(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    artifact_from_messages(&output.stdout, &bridge.package_id, kind).with_context(|| {
        format!(
            "Cargo did not report the {label} cdylib for `{}`",
            project.package_name
        )
    })
}

/// Append one rustc flag while preserving flags supplied by the caller.
fn append_rust_flag(flags: &mut String, value: &str) {
    if !flags.is_empty() {
        flags.push(' ');
    }
    flags.push_str(value);
}

/// Find the requested bridge artifact in Cargo's JSON message stream.
fn artifact_from_messages(bytes: &[u8], package_id: &str, kind: CompileKind) -> Option<PathBuf> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|message| {
            if message["reason"] != "compiler-artifact" || message["package_id"] != package_id {
                return None;
            }
            let crate_types = message["target"]["crate_types"].as_array()?;
            if !crate_types.iter().any(|value| value == "cdylib") {
                return None;
            }
            message["filenames"]
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .find(|path| match kind {
                    CompileKind::Native => path
                        .extension()
                        .and_then(|value| value.to_str())
                        .is_some_and(|value| matches!(value, "dylib" | "so" | "dll")),
                    CompileKind::Wasm => path.extension().is_some_and(|value| value == "wasm"),
                })
        })
}

/// Extract rendered compiler diagnostics from Cargo's JSON message stream.
fn cargo_diagnostics(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|message| message["message"]["rendered"].as_str().map(str::to_owned))
        .collect()
}

// Discovery ABI

/// Load the native bridge and deserialize its discovered application contract.
fn read_contract(library_path: &Path, package_name: &str) -> Result<Manifest> {
    type ContractFn = unsafe extern "C" fn() -> rspyts::__private::DiscoveryResult;
    type FreeFn = unsafe extern "C" fn(*mut c_char);

    // SAFETY: the library remains live while both symbols and the returned payload are used.
    let library = unsafe { Library::new(library_path) }
        .with_context(|| format!("failed to load {}", library_path.display()))?;
    let contract_name = format!("{CONTRACT_SYMBOL}__{package_name}\0");
    let free_name = format!("{FREE_SYMBOL}__{package_name}\0");
    // SAFETY: the application macro emits these exact ABI signatures.
    let contract: Symbol<'_, ContractFn> = unsafe { library.get(contract_name.as_bytes()) }
        .with_context(|| {
            format!(
                "generated rspyts bridge is missing `{}`",
                contract_name.trim_end_matches('\0')
            )
        })?;
    // SAFETY: the application macro emits these exact ABI signatures.
    let free: Symbol<'_, FreeFn> = unsafe { library.get(free_name.as_bytes()) }
        .with_context(|| format!("missing `{}`", free_name.trim_end_matches('\0')))?;
    // SAFETY: the loaded function follows the discovery ABI.
    let result = unsafe { contract() };
    if result.payload.is_null() {
        bail!("the generated rspyts bridge panicked during contract discovery");
    }
    // SAFETY: the discovery ABI returns a live NUL-terminated string.
    let payload = unsafe { CStr::from_ptr(result.payload) }
        .to_bytes()
        .to_vec();
    // SAFETY: the payload came from this library and is freed once.
    unsafe { free(result.payload) };
    match result.status {
        rspyts::__private::DISCOVERY_SUCCESS => Ok(serde_json::from_slice(&payload)?),
        rspyts::__private::DISCOVERY_ERROR => {
            bail!(
                "invalid rspyts application: {}",
                String::from_utf8_lossy(&payload)
            )
        }
        status => bail!("rspyts discovery returned unknown status {status}"),
    }
}

// Cargo dependency rendering

/// Select and validate the primary and configured application packages.
fn select_application_packages<'a>(
    metadata: &'a CargoMetadata,
    requested_manifest: &Path,
    additional_packages: &[String],
) -> Result<Vec<&'a CargoPackage>> {
    let primary = metadata
        .packages
        .iter()
        .find(|package| {
            package
                .manifest_path
                .canonicalize()
                .is_ok_and(|path| path == requested_manifest)
        })
        .context("rspyts.toml must be beside a Cargo package manifest")?;
    if !metadata.workspace_members.contains(&primary.id) {
        bail!("the Cargo package beside rspyts.toml must be a workspace member");
    }

    let mut selected = vec![primary];
    for name in additional_packages {
        if selected.iter().any(|package| &package.name == name) {
            bail!("rspyts application package `{name}` is listed more than once");
        }
        let mut matches = metadata.packages.iter().filter(|package| {
            &package.name == name && metadata.workspace_members.contains(&package.id)
        });
        let package = matches.next().with_context(|| {
            format!("additional rspyts package `{name}` is not a workspace member")
        })?;
        if matches.next().is_some() {
            bail!("additional rspyts package name `{name}` is ambiguous");
        }
        selected.push(package);
    }
    for package in &selected {
        if !has_library_target(package) {
            bail!(
                "rspyts application package `{}` must have a library target",
                package.name
            );
        }
    }
    Ok(selected)
}

/// Require every application package to use the same direct `rspyts` package.
fn resolve_rspyts_dependency(
    metadata: &CargoMetadata,
    selected: &[&CargoPackage],
) -> Result<ResolvedDependency> {
    let mut identities = selected
        .iter()
        .map(|package| direct_rspyts_id(metadata, &package.id));
    let identity = identities
        .next()
        .context("an rspyts application must select at least one Cargo package")??;
    for candidate in identities {
        if candidate? != identity {
            bail!("all rspyts application packages must resolve the same `rspyts` package");
        }
    }
    let package = metadata
        .packages
        .iter()
        .find(|package| package.id == identity)
        .context("Cargo metadata omitted the resolved rspyts package")?;
    resolved_dependency(package)
}

/// Return whether a Cargo package exposes a linkable library target.
fn has_library_target(package: &CargoPackage) -> bool {
    package.targets.iter().any(|target| {
        target.kind.iter().any(|kind| kind == "lib")
            || target
                .crate_types
                .iter()
                .any(|kind| matches!(kind.as_str(), "lib" | "rlib" | "cdylib"))
    })
}

/// Resolve the package ID of a package's direct normal `rspyts` dependency.
fn direct_rspyts_id(metadata: &CargoMetadata, package_id: &str) -> Result<String> {
    let nodes = &metadata
        .resolve
        .as_ref()
        .context("Cargo metadata has no dependency graph")?;
    let node = nodes
        .nodes
        .iter()
        .find(|node| node.id == package_id)
        .context("Cargo metadata omitted an application package from its dependency graph")?;
    let mut matches = node
        .deps
        .iter()
        .filter(|dependency| dependency.dep_kinds.iter().any(|kind| kind.kind.is_none()))
        .filter(|dependency_id| {
            metadata
                .packages
                .iter()
                .any(|package| package.id == dependency_id.pkg && package.name == "rspyts")
        })
        .map(|dependency| dependency.pkg.as_str());
    let dependency = matches
        .next()
        .context("each rspyts application package must directly depend on `rspyts`")?;
    if matches.next().is_some() {
        bail!("an rspyts application package resolves multiple direct `rspyts` dependencies");
    }
    Ok(dependency.to_owned())
}

/// Convert Cargo's resolved package into an exact bridge dependency.
fn resolved_dependency(package: &CargoPackage) -> Result<ResolvedDependency> {
    if let Some(source) = package.source.as_deref() {
        if matches!(
            source,
            "registry+https://github.com/rust-lang/crates.io-index"
                | "registry+sparse+https://index.crates.io/"
        ) {
            return Ok(ResolvedDependency::CratesIo {
                version: package.version.clone(),
            });
        }
        bail!("the selected rspyts package uses unsupported Cargo source `{source}`");
    }
    let root = package
        .manifest_path
        .parent()
        .context("resolved rspyts manifest has no parent")?
        .canonicalize()
        .context("cannot resolve the selected rspyts package source")?;
    Ok(ResolvedDependency::Path(root))
}

/// Render an exact Cargo dependency specification for the synthetic bridge.
fn bridge_dependency(dependency: &ResolvedDependency) -> String {
    match dependency {
        ResolvedDependency::Path(root) => {
            format!("{{ path = {} }}", toml_string(&root.to_string_lossy()))
        }
        ResolvedDependency::CratesIo { version } => {
            format!("{{ version = {} }}", toml_string(&format!("={version}")))
        }
    }
}

/// Compute a stable FNV-1a key for target-directory isolation.
fn stable_key(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Quote a TOML string through serde's compatible JSON string encoder.
fn toml_string(value: &str) -> String {
    serde_json::to_string(value).expect("serializing a string cannot fail")
}

/// Replace a generated bridge file only when its bytes changed.
fn write_if_changed(path: &Path, source: &str) -> Result<()> {
    if fs::read(path).is_ok_and(|current| current == source.as_bytes()) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, source).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crates_io_dependency_keeps_its_cargo_identity() {
        let package: CargoPackage = serde_json::from_value(serde_json::json!({
            "id": "rspyts 3.0.1 (registry+https://github.com/rust-lang/crates.io-index)",
            "name": "rspyts",
            "manifest_path": "/cargo/registry/rspyts-3.0.1/Cargo.toml",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "version": "3.0.1",
            "targets": [],
        }))
        .expect("deserialize package metadata");

        let dependency = resolved_dependency(&package).expect("resolve crates.io dependency");

        assert_eq!(bridge_dependency(&dependency), "{ version = \"=3.0.1\" }");
    }

    #[test]
    fn path_dependency_keeps_its_canonical_path() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../rspyts/Cargo.toml")
            .canonicalize()
            .expect("canonical rspyts manifest");
        let package: CargoPackage = serde_json::from_value(serde_json::json!({
            "id": "path+file:///workspace/crates/rspyts#3.0.1",
            "name": "rspyts",
            "manifest_path": manifest,
            "source": null,
            "version": "3.0.1",
            "targets": [],
        }))
        .expect("deserialize package metadata");

        let dependency = resolved_dependency(&package).expect("resolve path dependency");
        let rendered = bridge_dependency(&dependency);

        assert!(rendered.starts_with("{ path = "));
        assert!(rendered.contains("/crates/rspyts"));
    }
}
