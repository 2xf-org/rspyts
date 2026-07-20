use std::collections::BTreeSet;
use std::ffi::{CStr, c_char};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::{Context, Result, bail};
use libloading::{Library, Symbol};
use rspyts::ir::Manifest;
use serde::Serialize;
use serde_json::Value;
use tempfile::TempDir;

use crate::contract::buffer_elements;
use crate::output::{file_tree, project_lock, replace_directory, source_fingerprint, write_json};
use crate::{python, typescript};

const CONTRACT_SYMBOL: &str = "rspyts_discovery_v1_contract";
const FREE_SYMBOL: &str = "rspyts_discovery_v1_contract_free";

#[derive(Debug, Clone)]
pub(super) struct Project {
    pub(super) root: PathBuf,
    pub(super) workspace_root: PathBuf,
    manifest: PathBuf,
    package_id: String,
    package_name: String,
    package_version: String,
    pub(super) python_package: String,
    pub(super) typescript_package: String,
}

impl Project {
    pub(super) fn read(path: &Path) -> Result<Self> {
        let requested_manifest = if path.is_dir() {
            path.join("Cargo.toml")
        } else {
            path.to_path_buf()
        };
        let requested_manifest = requested_manifest
            .canonicalize()
            .with_context(|| format!("cannot find Cargo manifest {}", path.display()))?;
        let output = ProcessCommand::new(cargo())
            .args([
                "metadata",
                "--format-version",
                "1",
                "--no-deps",
                "--manifest-path",
            ])
            .arg(&requested_manifest)
            .output()
            .context("failed to run cargo metadata")?;
        if !output.status.success() {
            bail!(
                "cargo metadata failed\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let metadata: Value = serde_json::from_slice(&output.stdout)?;
        let packages = metadata["packages"]
            .as_array()
            .context("Cargo metadata has no package list")?;
        let workspace_members = metadata["workspace_members"]
            .as_array()
            .context("Cargo metadata has no workspace member list")?;
        let requested_package = packages.iter().find(|package| {
            package["manifest_path"]
                .as_str()
                .and_then(|value| Path::new(value).canonicalize().ok())
                .as_ref()
                == Some(&requested_manifest)
        });
        let mut candidates = packages
            .iter()
            .filter(|package| {
                package["id"]
                    .as_str()
                    .is_some_and(|id| workspace_members.iter().any(|member| member == id))
                    && is_binding_package(package)
            })
            .collect::<Vec<_>>();
        let package = match requested_package.filter(|package| is_binding_package(package)) {
            Some(package) => package,
            None => match candidates.len() {
                1 => candidates.pop().expect("one candidate exists"),
                0 => bail!(
                    "no rspyts binding crate found; add one workspace package with a direct `rspyts` dependency and crate-type = [\"cdylib\"]"
                ),
                _ => {
                    let names = candidates
                        .iter()
                        .filter_map(|package| package["name"].as_str())
                        .collect::<Vec<_>>();
                    bail!(
                        "multiple rspyts binding crates found: {names:?}; select one with `--manifest-path path/to/Cargo.toml`"
                    );
                }
            },
        };
        let manifest = PathBuf::from(string(package, "manifest_path")?)
            .canonicalize()
            .context("cannot resolve the binding Cargo manifest")?;
        let package_name = string(package, "name")?.to_owned();
        let package_version = string(package, "version")?.to_owned();
        let package_id = string(package, "id")?.to_owned();

        let settings = package["metadata"]["rspyts"].as_object();
        let python_package = settings
            .and_then(|value| value.get("python"))
            .and_then(Value::as_str)
            .map_or_else(|| package_name.replace('-', "_"), str::to_owned);
        let typescript_package = settings
            .and_then(|value| value.get("typescript"))
            .and_then(Value::as_str)
            .map_or_else(|| package_name.clone(), str::to_owned);
        validate_python_package(&python_package)?;
        validate_typescript_package(&typescript_package)?;

        Ok(Self {
            root: manifest
                .parent()
                .context("Cargo manifest has no parent")?
                .to_path_buf(),
            workspace_root: PathBuf::from(string(&metadata, "workspace_root")?),
            manifest,
            package_id,
            package_name,
            package_version,
            python_package,
            typescript_package,
        })
    }

    pub(super) fn output(&self) -> PathBuf {
        self.root.join("dist")
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BuildReport {
    status: &'static str,
    output: PathBuf,
    python_package: String,
    typescript_package: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeneratedContract<'a> {
    #[serde(flatten)]
    manifest: &'a Manifest,
    source_fingerprint: String,
}

pub(super) fn build(project: &Project) -> Result<BuildReport> {
    let _lock = project_lock(project)?;
    let generated = generate(project)?;
    replace_directory(&generated, &project.output())?;
    Ok(BuildReport {
        status: "ok",
        output: project.output(),
        python_package: project.python_package.clone(),
        typescript_package: project.typescript_package.clone(),
    })
}

pub(super) fn check(project: &Project) -> Result<()> {
    let _lock = project_lock(project)?;
    let generated = generate(project)?;
    let expected = file_tree(generated.path())?;
    let actual = file_tree(&project.output()).with_context(|| {
        format!(
            "{} does not exist; run `rspyts build`",
            project.output().display()
        )
    })?;
    if expected != actual {
        let expected_names = expected.keys().cloned().collect::<BTreeSet<_>>();
        let actual_names = actual.keys().cloned().collect::<BTreeSet<_>>();
        let changed = expected_names
            .intersection(&actual_names)
            .filter(|path| !is_binary(path) && expected.get(*path) != actual.get(*path))
            .cloned()
            .collect::<Vec<_>>();
        bail!(
            "dist is not in sync (missing: {:?}; extra: {:?}; changed: {:?}); run `rspyts build`",
            expected_names.difference(&actual_names).collect::<Vec<_>>(),
            actual_names.difference(&expected_names).collect::<Vec<_>>(),
            changed,
        );
    }
    Ok(())
}

fn is_binary(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| matches!(extension.to_str(), Some("pyd" | "so" | "wasm")))
}

fn generate(project: &Project) -> Result<TempDir> {
    let temporary = tempfile::Builder::new()
        .prefix(".rspyts-")
        .tempdir_in(&project.root)?;
    let native = compile(project, CompileKind::Native)?;
    let manifest = read_contract(&native, &project.package_name)?;
    validate_contract(project, &manifest)?;
    let wasm = compile(project, CompileKind::Wasm)?;

    let contract = GeneratedContract {
        manifest: &manifest,
        source_fingerprint: source_fingerprint(&project.workspace_root)?,
    };
    write_json(&temporary.path().join("contract.json"), &contract)?;
    python::emit(project, &manifest, &native, temporary.path())?;
    typescript::emit(project, &manifest, &wasm, temporary.path())?;
    Ok(temporary)
}

#[derive(Clone, Copy)]
enum CompileKind {
    Native,
    Wasm,
}

fn compile(project: &Project, kind: CompileKind) -> Result<PathBuf> {
    let (feature, label) = match kind {
        CompileKind::Native => (Some("rspyts/python-extension"), "Python"),
        CompileKind::Wasm => (None, "WebAssembly"),
    };
    let mut command = ProcessCommand::new(cargo());
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(&project.manifest)
        .arg("--package")
        .arg(&project.package_name)
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
    command.env("RUSTFLAGS", flags);
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
    artifact_from_messages(&output.stdout, &project.package_id, kind).with_context(|| {
        format!(
            "Cargo did not report the {label} cdylib for `{}`",
            project.package_name
        )
    })
}

fn append_rust_flag(flags: &mut String, value: &str) {
    if !flags.is_empty() {
        flags.push(' ');
    }
    flags.push_str(value);
}

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

fn cargo_diagnostics(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|message| message["message"]["rendered"].as_str().map(str::to_owned))
        .collect()
}

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
                "missing `{}`; add `rspyts::application!(your_api_crate);`",
                contract_name.trim_end_matches('\0')
            )
        })?;
    // SAFETY: the application macro emits these exact ABI signatures.
    let free: Symbol<'_, FreeFn> = unsafe { library.get(free_name.as_bytes()) }
        .with_context(|| format!("missing `{}`", free_name.trim_end_matches('\0')))?;
    // SAFETY: the loaded function follows the discovery ABI.
    let result = unsafe { contract() };
    if result.payload.is_null() {
        bail!("the aggregate binding panicked during contract discovery");
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

fn validate_contract(project: &Project, manifest: &Manifest) -> Result<()> {
    if manifest.ir_version != rspyts::ir::IR_VERSION {
        bail!("unsupported contract version {}", manifest.ir_version);
    }
    if manifest.package_name != project.package_name
        || manifest.package_version != project.package_version
    {
        bail!("the discovered package does not match Cargo metadata");
    }
    if manifest.module_name.is_empty() {
        bail!("the Python module name cannot be empty");
    }
    let mut python_names = manifest
        .types
        .iter()
        .map(|item| item.name.clone())
        .chain(manifest.errors.iter().map(|item| item.name.clone()))
        .chain(manifest.resources.iter().map(|item| item.name.clone()))
        .chain(manifest.functions.iter().map(|item| item.rust_name.clone()))
        .chain(manifest.constants.iter().map(|item| item.host_name.clone()))
        .collect::<Vec<_>>();
    python_names.extend(
        buffer_elements(manifest)
            .into_iter()
            .map(|element| python::buffer_name(element).to_owned()),
    );
    unique_public_names("Python", python_names.into_iter())?;
    unique_public_names(
        "TypeScript",
        manifest
            .types
            .iter()
            .map(|item| item.name.as_str())
            .chain(manifest.errors.iter().map(|item| item.name.as_str()))
            .chain(manifest.resources.iter().map(|item| item.name.as_str()))
            .chain(
                manifest
                    .functions
                    .iter()
                    .map(|item| item.host_name.as_str()),
            )
            .chain(
                manifest
                    .constants
                    .iter()
                    .map(|item| item.host_name.as_str()),
            ),
    )?;
    Ok(())
}

pub(super) fn unique_public_names<S: AsRef<str>>(
    host: &str,
    names: impl Iterator<Item = S>,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    for name in names {
        let name = name.as_ref();
        if !seen.insert(name.to_owned()) {
            bail!("duplicate {host} export name `{name}`");
        }
    }
    Ok(())
}

pub(super) fn validate_python_package(value: &str) -> Result<()> {
    if value.is_empty() || value.split('.').any(|part| !is_identifier(part)) {
        bail!("Python package `{value}` must contain dot-separated identifiers");
    }
    Ok(())
}

pub(super) fn validate_typescript_package(value: &str) -> Result<()> {
    let name = value.strip_prefix('@').unwrap_or(value);
    let parts = name.split('/').collect::<Vec<_>>();
    let expected = if value.starts_with('@') { 2 } else { 1 };
    if value.is_empty()
        || parts.len() != expected
        || parts.iter().any(|part| {
            part.is_empty()
                || !part.chars().all(|character| {
                    character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || matches!(character, '-' | '_' | '.')
                })
        })
    {
        bail!("invalid TypeScript package `{value}`");
    }
    Ok(())
}

pub(super) fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .with_context(|| format!("Cargo metadata has no `{key}` string"))
}

fn is_binding_package(package: &Value) -> bool {
    let has_cdylib = package["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|target| {
            target["crate_types"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|kind| kind == "cdylib")
        });
    let depends_on_rspyts = package["dependencies"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|dependency| dependency["name"] == "rspyts");
    has_cdylib && depends_on_rspyts
}

fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}
