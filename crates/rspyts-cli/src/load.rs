use std::collections::BTreeSet;
use std::ffi::{CStr, c_char};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use libloading::{Library, Symbol};
use serde_json::Value;

use crate::config::{Project, TypeScriptMode};

const WASM_TARGET: &str = "wasm32-unknown-unknown";
const DISCOVERY_CONTRACT_SYMBOL: &str = "rspyts_discovery_v1_contract";
const DISCOVERY_FREE_SYMBOL: &str = "rspyts_discovery_v1_contract_free";
const LEGACY_DISCOVERY_CONTRACT_SYMBOL: &str = "rspyts_contract";
const LEGACY_DISCOVERY_FREE_SYMBOL: &str = "rspyts_contract_free";

type ContractFunction = unsafe extern "C" fn() -> rspyts::__private::DiscoveryResult;
type ContractFreeFunction = unsafe extern "C" fn(*mut c_char);

pub struct LoadedContract {
    pub manifest: rspyts::ir::Manifest,
}

pub fn load_contract(project: &Project) -> Result<LoadedContract> {
    let probe_features = project
        .probe_features()
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let package = package_identity(project, CompileTarget::Probe, &probe_features, false)?;
    let artifact = compile(
        project,
        &package,
        CompileTarget::Probe,
        &probe_features,
        false,
    )?;
    let discovery = unsafe { read_contract(&artifact, &package.name)? };
    let manifest = serde_json::from_slice(&discovery.bytes)
        .with_context(|| format!("{} exported an invalid rspyts contract", artifact.display()))?;
    validate_module_capabilities(project, discovery.capabilities)?;
    Ok(LoadedContract { manifest })
}

struct DiscoveryPayload {
    bytes: Vec<u8>,
    capabilities: u32,
}

fn validate_module_capabilities(project: &Project, capabilities: u32) -> Result<()> {
    let known = rspyts::__private::DISCOVERY_PYTHON | rspyts::__private::DISCOVERY_TYPESCRIPT;
    if capabilities & !known != 0 {
        bail!(
            "Rust module reported unknown rspyts discovery capabilities: {:#x}",
            capabilities & !known
        );
    }
    if project.python.is_some() && capabilities & rspyts::__private::DISCOVERY_PYTHON == 0 {
        bail!(
            "Python generation requires a Python-capable Rust module; use `rspyts::module!(name)` or `rspyts::module!(name, python)`"
        );
    }
    if project
        .typescript
        .as_ref()
        .is_some_and(|config| config.mode == TypeScriptMode::Wasm)
        && capabilities & rspyts::__private::DISCOVERY_TYPESCRIPT == 0
    {
        bail!(
            "TypeScript WASM mode requires a TypeScript-capable Rust module; use `rspyts::module!(name)` or `rspyts::module!(name, typescript)`"
        );
    }
    Ok(())
}

pub fn compile_wasm(project: &Project) -> Result<PathBuf> {
    let package = package_identity(project, CompileTarget::Wasm, &["wasm"], true)?;
    compile(project, &package, CompileTarget::Wasm, &["wasm"], true)
}

#[derive(Clone, Copy)]
enum CompileTarget {
    Probe,
    Wasm,
}

fn compile(
    project: &Project,
    package: &PackageIdentity,
    target: CompileTarget,
    backend_features: &[&str],
    include_common_features: bool,
) -> Result<PathBuf> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .arg("build")
        .arg("--locked")
        .arg("--manifest-path")
        .arg(project.cargo_manifest())
        .arg("--package")
        .arg(&package.name)
        .arg("--lib")
        .arg("--release")
        .arg("--message-format=json-render-diagnostics");
    append_feature_args(
        &mut command,
        project,
        backend_features,
        include_common_features,
    );
    append_target_args(&mut command, target, false, &package.rust_target);
    configure_build(&mut command, package);
    command.current_dir(project.root());

    let output = command.output().with_context(|| {
        format!(
            "failed to run Cargo for {}",
            project.cargo_manifest().display()
        )
    })?;
    let messages = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        let rendered = cargo_diagnostics(&messages);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Cargo failed while compiling {}\n{}{}",
            project.cargo_manifest().display(),
            rendered,
            stderr
        );
    }

    artifact_from_messages(&messages, &package.id, target).with_context(|| {
        format!(
            "Cargo did not report a cdylib artifact for package `{}`; ensure [lib] includes `crate-type = [\"cdylib\"]`",
            package.name
        )
    })
}

fn configure_build(command: &mut Command, package: &PackageIdentity) {
    let isolated_target = package.target_directory.join("rspyts");
    command.env("CARGO_TARGET_DIR", &isolated_target).env(
        "CARGO_ENCODED_RUSTFLAGS",
        path_remap_flags(package, &isolated_target),
    );
}

fn path_remap_flags(package: &PackageIdentity, isolated_target: &Path) -> String {
    let mut remaps = Vec::new();
    let cli_manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Some(source_workspace) = cli_manifest.parent().and_then(Path::parent)
        && source_workspace.join("crates/rspyts/Cargo.toml").is_file()
    {
        remaps.push((source_workspace.to_path_buf(), "/rspyts"));
    }
    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
    {
        remaps.push((home, "/home"));
    }
    if let Some(cargo_home) = std::env::var_os("CARGO_HOME").map(PathBuf::from) {
        remaps.push((cargo_home, "/cargo"));
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        remaps.push((home.join(".cargo"), "/cargo"));
    }
    remaps.extend([
        (package.workspace_root.clone(), "/workspace"),
        (package.target_directory.clone(), "/target"),
        (isolated_target.to_path_buf(), "/target"),
    ]);
    // rustc resolves overlapping remaps from the last flag backwards, so
    // parent paths must precede the more specific paths that should win.
    remaps.sort_by(|(left, _), (right, _)| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });
    remaps
        .into_iter()
        .map(|(path, replacement)| {
            format!(
                "--remap-path-prefix={}={replacement}",
                path.to_string_lossy()
            )
        })
        .collect::<Vec<_>>()
        .join("\x1f")
}

fn append_target_args(
    command: &mut Command,
    target: CompileTarget,
    metadata: bool,
    rust_target: &str,
) {
    if matches!(target, CompileTarget::Wasm) {
        command
            .arg("--config")
            .arg("profile.release.strip=\"debuginfo\"");
    }
    if metadata {
        command.arg("--filter-platform").arg(rust_target);
    } else {
        command.arg("--target").arg(rust_target);
    }
}

fn rust_target(project_root: &Path, target: CompileTarget) -> Result<String> {
    if matches!(target, CompileTarget::Wasm) {
        return Ok(WASM_TARGET.to_owned());
    }
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(&rustc)
        .arg("-vV")
        .current_dir(project_root)
        .output()
        .with_context(|| format!("failed to query host target from {rustc:?}"))?;
    if !output.status.success() {
        bail!(
            "failed to query host target from {rustc:?}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("rustc {rustc:?} did not report a host target"))
}

fn append_feature_args(
    command: &mut Command,
    project: &Project,
    backend_features: &[&str],
    include_common_features: bool,
) {
    if !project.default_features() {
        command.arg("--no-default-features");
    }
    let common_features = if include_common_features {
        project.features()
    } else {
        &[]
    };
    let features = common_features
        .iter()
        .map(String::as_str)
        .chain(backend_features.iter().copied())
        .collect::<BTreeSet<_>>();
    if !features.is_empty() {
        command
            .arg("--features")
            .arg(features.into_iter().collect::<Vec<_>>().join(","));
    }
}

#[derive(Debug, PartialEq, Eq)]
struct PackageIdentity {
    id: String,
    name: String,
    workspace_root: PathBuf,
    target_directory: PathBuf,
    rust_target: String,
}

fn package_identity(
    project: &Project,
    target: CompileTarget,
    backend_features: &[&str],
    include_common_features: bool,
) -> Result<PackageIdentity> {
    let manifest = project.cargo_manifest();
    let rust_target = rust_target(project.root(), target)?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .arg("metadata")
        .arg("--format-version=1")
        .arg("--locked")
        .arg("--no-deps")
        .arg("--manifest-path")
        .arg(manifest);
    append_feature_args(
        &mut command,
        project,
        backend_features,
        include_common_features,
    );
    append_target_args(&mut command, target, true, &rust_target);
    command.current_dir(project.root());
    let output = command
        .output()
        .with_context(|| format!("failed to read Cargo metadata for {}", manifest.display()))?;
    if !output.status.success() {
        bail!(
            "Cargo metadata failed for {}\n{}",
            manifest.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("Cargo returned invalid metadata for {}", manifest.display()))?;
    let expected_manifest = manifest
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", manifest.display()))?;
    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .context("Cargo metadata has no package list")?;
    let mut matching = packages.iter().filter_map(|package| {
        let package_manifest = Path::new(package.get("manifest_path")?.as_str()?)
            .canonicalize()
            .ok()?;
        (package_manifest == expected_manifest).then_some(package)
    });
    let package = matching.next().with_context(|| {
        format!(
            "Cargo metadata did not identify the package at {}",
            manifest.display()
        )
    })?;
    if matching.next().is_some() {
        bail!(
            "Cargo metadata identified more than one package at {}",
            manifest.display()
        );
    }
    Ok(PackageIdentity {
        id: package
            .get("id")
            .and_then(Value::as_str)
            .context("Cargo package metadata has no id")?
            .to_owned(),
        name: package
            .get("name")
            .and_then(Value::as_str)
            .context("Cargo package metadata has no name")?
            .to_owned(),
        workspace_root: metadata_path(&metadata, "workspace_root")?
            .canonicalize()
            .context("failed to resolve Cargo workspace root")?,
        target_directory: metadata_path(&metadata, "target_directory")?,
        rust_target,
    })
}

fn metadata_path(metadata: &Value, field: &str) -> Result<PathBuf> {
    metadata
        .get(field)
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .with_context(|| format!("Cargo metadata has no {field}"))
}

fn artifact_from_messages(
    messages: &str,
    package_id: &str,
    target: CompileTarget,
) -> Option<PathBuf> {
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|message| {
            if message.get("reason")?.as_str()? != "compiler-artifact"
                || message.get("package_id")?.as_str()? != package_id
            {
                return None;
            }
            let crate_types = message.get("target")?.get("crate_types")?.as_array()?;
            if !crate_types
                .iter()
                .any(|crate_type| crate_type.as_str() == Some("cdylib"))
            {
                return None;
            }
            message
                .get("filenames")?
                .as_array()?
                .iter()
                .find_map(|filename| {
                    let path = PathBuf::from(filename.as_str()?);
                    match target {
                        CompileTarget::Probe if is_dynamic_library(&path) => Some(path),
                        CompileTarget::Wasm
                            if path.extension().is_some_and(|value| value == "wasm") =>
                        {
                            Some(path)
                        }
                        _ => None,
                    }
                })
        })
}

fn is_dynamic_library(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "so" | "dylib" | "dll"))
}

fn cargo_diagnostics(messages: &str) -> String {
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|message| {
            message
                .get("message")?
                .get("rendered")?
                .as_str()
                .map(str::to_owned)
        })
        .collect::<Vec<_>>()
        .join("")
}

fn discovery_symbol(prefix: &str, package_name: &str) -> Vec<u8> {
    format!("{prefix}__{package_name}\0").into_bytes()
}

unsafe fn read_contract(library_path: &Path, package_name: &str) -> Result<DiscoveryPayload> {
    let library = unsafe { Library::new(library_path) }
        .with_context(|| format!("failed to load {}", library_path.display()))?;
    let contract_symbol = discovery_symbol(DISCOVERY_CONTRACT_SYMBOL, package_name);
    let contract: Symbol<'_, ContractFunction> = match unsafe {
        library.get(contract_symbol.as_slice())
    } {
        Ok(contract) => contract,
        Err(source) => {
            if let Some(legacy_symbol) =
                legacy_discovery_symbol(&library, LEGACY_DISCOVERY_CONTRACT_SYMBOL, package_name)
            {
                bail!(
                    "Cargo package `{package_name}` exports legacy unversioned rspyts discovery symbol `{}`, but this CLI requires discovery ABI v1 symbol `{}`; pin the `rspyts` crate and CLI to the exact same version, then rebuild the crate",
                    symbol_name(&legacy_symbol),
                    symbol_name(&contract_symbol),
                );
            }
            return Err(anyhow::Error::new(source).context(format!(
                    "Cargo package `{package_name}` is missing required rspyts discovery ABI v1 symbol `{}`; pin the `rspyts` crate and CLI to the exact same version, then rebuild the crate",
                    symbol_name(&contract_symbol),
                )));
        }
    };
    let free_symbol = discovery_symbol(DISCOVERY_FREE_SYMBOL, package_name);
    let free: Symbol<'_, ContractFreeFunction> = match unsafe {
        library.get(free_symbol.as_slice())
    } {
        Ok(free) => free,
        Err(source) => {
            if let Some(legacy_symbol) =
                legacy_discovery_symbol(&library, LEGACY_DISCOVERY_FREE_SYMBOL, package_name)
            {
                bail!(
                    "Cargo package `{package_name}` exports legacy unversioned rspyts discovery-free symbol `{}`, but this CLI requires discovery ABI v1 free symbol `{}`; pin the `rspyts` crate and CLI to the exact same version, then rebuild the crate",
                    symbol_name(&legacy_symbol),
                    symbol_name(&free_symbol),
                );
            }
            return Err(anyhow::Error::new(source).context(format!(
                    "Cargo package `{package_name}` is missing required rspyts discovery ABI v1 free symbol `{}`; pin the `rspyts` crate and CLI to the exact same version, then rebuild the crate",
                    symbol_name(&free_symbol),
                )));
        }
    };
    let result = unsafe { contract() };
    if result.payload.is_null() {
        if result.status == rspyts::__private::DISCOVERY_PANIC {
            bail!("Cargo package `{package_name}` panicked during rspyts contract discovery");
        }
        bail!(
            "Cargo package `{package_name}` returned a null rspyts discovery payload with status {}",
            result.status
        );
    }
    let bytes = unsafe { CStr::from_ptr(result.payload) }
        .to_bytes()
        .to_vec();
    unsafe { free(result.payload) };
    match result.status {
        rspyts::__private::DISCOVERY_SUCCESS => Ok(DiscoveryPayload {
            bytes,
            capabilities: result.capabilities,
        }),
        rspyts::__private::DISCOVERY_ERROR => bail!(
            "Cargo package `{package_name}` failed rspyts contract discovery: {}",
            String::from_utf8_lossy(&bytes)
        ),
        rspyts::__private::DISCOVERY_PANIC => bail!(
            "Cargo package `{package_name}` panicked during rspyts contract discovery: {}",
            String::from_utf8_lossy(&bytes)
        ),
        status => bail!(
            "Cargo package `{package_name}` returned unknown rspyts discovery status {status}"
        ),
    }
}

fn symbol_name(symbol: &[u8]) -> &str {
    std::str::from_utf8(&symbol[..symbol.len() - 1])
        .expect("rspyts discovery symbols are always valid UTF-8")
}

fn legacy_discovery_symbol(library: &Library, prefix: &str, package_name: &str) -> Option<Vec<u8>> {
    [
        format!("{prefix}\0").into_bytes(),
        discovery_symbol(prefix, package_name),
    ]
    .into_iter()
    .find(|symbol| unsafe { library.get::<*const ()>(symbol.as_slice()).is_ok() })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn workspace_package_path(name: &str) -> PathBuf {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = Command::new(cargo)
            .args([
                "metadata",
                "--format-version",
                "1",
                "--manifest-path",
                &Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("Cargo.toml")
                    .to_string_lossy(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
        let manifest = metadata["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|package| package["name"] == name)
            .and_then(|package| package["manifest_path"].as_str())
            .unwrap();
        Path::new(manifest).parent().unwrap().to_path_buf()
    }

    fn fixture_project(config: &str) -> (PathBuf, Project) {
        let root = std::env::temp_dir().join(format!(
            "rspyts-load-config-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(root.join("rust/src")).unwrap();
        fs::write(
            root.join("rust/Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(root.join("rust/src/lib.rs"), "").unwrap();
        fs::write(root.join("rspyts.toml"), config).unwrap();
        let project = Project::read(&root.join("rspyts.toml")).unwrap();
        (root.canonicalize().unwrap(), project)
    }

    fn generate_lockfile(root: &Path, manifest: &Path) {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = Command::new(cargo)
            .arg("generate-lockfile")
            .arg("--manifest-path")
            .arg(manifest)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn selects_only_the_requested_package_artifact() {
        let messages = r#"{"reason":"compiler-artifact","package_id":"path+file:///dep#dependency@1.0.0","target":{"name":"example_native","crate_types":["cdylib"]},"filenames":["/tmp/libdependency.so"]}
{"reason":"compiler-artifact","package_id":"path+file:///sample#sample@0.1.0","target":{"name":"helper","crate_types":["bin"]},"filenames":["/tmp/helper.so"]}
{"reason":"compiler-artifact","package_id":"path+file:///sample#sample@0.1.0","target":{"name":"example_native","crate_types":["cdylib","rlib"]},"filenames":["/tmp/libexample_native.rlib","/tmp/libexample_native.so"]}"#;
        assert_eq!(
            artifact_from_messages(
                messages,
                "path+file:///sample#sample@0.1.0",
                CompileTarget::Probe
            ),
            Some(PathBuf::from("/tmp/libexample_native.so"))
        );
    }

    #[test]
    fn missing_lockfile_is_an_actionable_locked_error() {
        let (root, project) = fixture_project(
            "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
        );
        fs::write(
            root.join("rust/Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nserde = \"1\"\n",
        )
        .unwrap();
        let error = match load_contract(&project) {
            Ok(_) => panic!("build unexpectedly accepted a missing Cargo.lock"),
            Err(error) => error,
        };
        let message = format!("{error:#}");
        assert!(
            message.contains("Cargo failed while compiling"),
            "{message}"
        );
        assert!(message.contains("--locked"), "{message}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_workspace_member_when_library_name_differs_from_package_name() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-workspace-member-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let rust = root.join("member");
        fs::create_dir_all(rust.join("src")).unwrap();
        let rspyts = workspace_package_path("rspyts");
        let macros = workspace_package_path("rspyts-macros");
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "[workspace]\nresolver = \"2\"\nmembers = [\"member\"]\n\n[patch.crates-io]\nrspyts-macros = {{ path = {:?} }}\n",
                macros.to_string_lossy()
            ),
        )
        .unwrap();
        fs::write(
            rust.join("Cargo.toml"),
            format!(
                "[package]\nname = \"example-package\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\nname = \"example_native\"\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nrspyts = {{ path = {:?} }}\n",
                rspyts.to_string_lossy()
            ),
        )
        .unwrap();
        fs::write(rust.join("src/lib.rs"), "rspyts::module!(native);\n").unwrap();
        fs::write(
            root.join("rspyts.toml"),
            "[crate]\npath = \"member\"\n\n[typescript]\npackage = \"example-package\"\nmode = \"static\"\n",
        )
        .unwrap();
        generate_lockfile(&root, &root.join("Cargo.toml"));

        let project = Project::read(&root.join("rspyts.toml")).unwrap();
        let loaded = load_contract(&project).unwrap();
        assert_eq!(loaded.manifest.crate_name, "example-package");
        assert_eq!(loaded.manifest.module_name, "native");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discovery_symbols_are_scoped_to_exact_cargo_package_names() {
        assert_eq!(
            discovery_symbol(DISCOVERY_CONTRACT_SYMBOL, "cross-package-consumer"),
            b"rspyts_discovery_v1_contract__cross-package-consumer\0"
        );
        assert_eq!(
            discovery_symbol(DISCOVERY_FREE_SYMBOL, "cross_package_owner"),
            b"rspyts_discovery_v1_contract_free__cross_package_owner\0"
        );
    }

    #[test]
    fn module_capabilities_match_configured_runtime_hosts() {
        let cases = [
            (
                "python-python-module",
                "[crate]\npath = \"rust\"\n\n[python]\npackage = \"fixture\"\n",
                rspyts::__private::DISCOVERY_PYTHON,
                true,
            ),
            (
                "python-typescript-module",
                "[crate]\npath = \"rust\"\n\n[python]\npackage = \"fixture\"\n",
                rspyts::__private::DISCOVERY_TYPESCRIPT,
                false,
            ),
            (
                "wasm-python-module",
                "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"wasm\"\n",
                rspyts::__private::DISCOVERY_PYTHON,
                false,
            ),
            (
                "wasm-typescript-module",
                "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"wasm\"\n",
                rspyts::__private::DISCOVERY_TYPESCRIPT,
                true,
            ),
            (
                "static-python-module",
                "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
                rspyts::__private::DISCOVERY_PYTHON,
                true,
            ),
        ];
        for (name, config, capabilities, accepted) in cases {
            let (root, project) = fixture_project(config);
            let result = validate_module_capabilities(&project, capabilities);
            assert_eq!(result.is_ok(), accepted, "case `{name}`: {result:?}");
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn unknown_module_capabilities_are_rejected() {
        let (root, project) = fixture_project(
            "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
        );
        let error = validate_module_capabilities(&project, 1 << 31).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown rspyts discovery capabilities")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn artifact_paths_are_remapped_from_the_fixed_workspace_and_target() {
        let package = PackageIdentity {
            id: "example@0.1.0".to_owned(),
            name: "example".to_owned(),
            workspace_root: PathBuf::from("/home/user/workspace"),
            target_directory: PathBuf::from("/home/user/workspace/target"),
            rust_target: "aarch64-apple-darwin".to_owned(),
        };
        let isolated = package.target_directory.join("rspyts");
        let flags = path_remap_flags(&package, &isolated);
        assert!(flags.contains("--remap-path-prefix=/home/user/workspace=/workspace"));
        assert!(flags.contains("--remap-path-prefix=/home/user/workspace/target=/target"));
        assert!(flags.contains("--remap-path-prefix=/home/user/workspace/target/rspyts=/target"));
        let source_workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap();
        if source_workspace.join("crates/rspyts/Cargo.toml").is_file() {
            assert!(flags.contains(&format!(
                "--remap-path-prefix={}=/rspyts",
                source_workspace.display()
            )));
        }
        assert!(
            flags
                .find("--remap-path-prefix=/home/user/workspace/target/rspyts=/target")
                .unwrap()
                > flags
                    .find("--remap-path-prefix=/home/user/workspace=/workspace")
                    .unwrap(),
            "specific paths must follow their parent paths so rustc resolves them first"
        );
    }

    #[test]
    fn semantic_contract_uses_only_explicit_probe_features() {
        let (root, project) = fixture_project(
            "[crate]\npath = \"rust\"\nfeatures = [\"common\"]\nprobe-features = [\"native\", \"formats\"]\n\n[python]\npackage = \"fixture\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"wasm\"\n",
        );
        let mut command = Command::new("cargo");
        append_feature_args(
            &mut command,
            &project,
            &project
                .probe_features()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            false,
        );
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            ["--no-default-features", "--features", "formats,native"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cargo_default_features_are_disabled_unless_opted_in() {
        let (root, project) = fixture_project(
            "[crate]\npath = \"rust\"\nfeatures = [\"domain\"]\n\n[python]\npackage = \"fixture\"\n",
        );
        let mut command = Command::new("cargo");
        append_feature_args(&mut command, &project, &["wasm"], true);
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, ["--no-default-features", "--features", "domain,wasm"]);
        fs::remove_dir_all(root).unwrap();

        let (root, project) = fixture_project(
            "[crate]\npath = \"rust\"\ndefault-features = true\n\n[python]\npackage = \"fixture\"\n",
        );
        let mut command = Command::new("cargo");
        append_feature_args(&mut command, &project, &["wasm"], true);
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, ["--features", "wasm"]);
        fs::remove_dir_all(root).unwrap();
    }
}
