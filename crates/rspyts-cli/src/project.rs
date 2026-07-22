use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, c_char};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::{Context, Result, bail};
use libloading::{Library, Symbol};
use rspyts::ir::{Manifest, Namespace};
use serde::Serialize;
use serde_json::Value;
use tempfile::TempDir;

use crate::contract::{
    named_identities, namespace_refs, namespaces, tagged_variant_name, type_namespace, type_refs,
};
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
    output: PathBuf,
}

impl Project {
    pub(super) fn read(path: &Path, requested_output: Option<&Path>) -> Result<Self> {
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

        let root = manifest
            .parent()
            .context("Cargo manifest has no parent")?
            .to_path_buf();
        let workspace_root = PathBuf::from(string(&metadata, "workspace_root")?);
        let output = requested_output.map_or_else(
            || Ok(root.join("dist")),
            |path| resolve_output(path, &root, &workspace_root),
        )?;

        Ok(Self {
            root,
            workspace_root,
            manifest,
            package_id,
            package_name,
            package_version,
            python_package,
            typescript_package,
            output,
        })
    }

    pub(super) fn output(&self) -> PathBuf {
        self.output.clone()
    }
}

fn resolve_output(path: &Path, project_root: &Path, workspace_root: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let output = if path.exists() {
        path.canonicalize()?
    } else {
        let parent = path
            .parent()
            .context("generated output path has no parent")?
            .canonicalize()
            .with_context(|| {
                format!(
                    "generated output parent does not exist: {}",
                    path.parent().expect("parent exists").display()
                )
            })?;
        parent.join(
            path.file_name()
                .context("generated output path has no name")?,
        )
    };
    if output == project_root || output == workspace_root {
        bail!("generated output cannot replace a project or workspace root");
    }
    Ok(output)
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
    validate_generated_files(&expected, &actual)
}

fn validate_generated_files(
    expected: &BTreeMap<PathBuf, Vec<u8>>,
    actual: &BTreeMap<PathBuf, Vec<u8>>,
) -> Result<()> {
    let expected_names = expected.keys().cloned().collect::<BTreeSet<_>>();
    let actual_names = actual.keys().cloned().collect::<BTreeSet<_>>();
    let missing = expected_names.difference(&actual_names).collect::<Vec<_>>();
    let extra = actual_names.difference(&expected_names).collect::<Vec<_>>();
    let changed = expected_names
        .intersection(&actual_names)
        .filter(|path| !is_binary(path) && expected.get(*path) != actual.get(*path))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() || !extra.is_empty() || !changed.is_empty() {
        bail!(
            "dist is not in sync (missing: {missing:?}; extra: {extra:?}; changed: {changed:?}); run `rspyts build`",
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
    if manifest.package_name != project.package_name
        || manifest.package_version != project.package_version
    {
        bail!("the discovered package does not match Cargo metadata");
    }
    if manifest.module_name.is_empty() {
        bail!("the Python module name cannot be empty");
    }
    validate_namespaces(manifest)?;
    validate_model_namespace_cycles(manifest)?;
    Ok(())
}

fn validate_namespaces(manifest: &Manifest) -> Result<()> {
    let namespace_map = namespaces(manifest);
    let python_namespace_paths = namespace_map
        .keys()
        .map(Namespace::python_segments)
        .collect::<Vec<_>>();
    for (owner, rust_module) in export_origins(manifest) {
        let namespace = manifest.namespace(owner, rust_module);
        if let Some(package) = &namespace.package {
            let segment = package.replace('-', "_");
            if !is_python_identifier(&segment) {
                bail!(
                    "Cargo package `{owner}` derives the invalid Python namespace segment `{segment}`; rename the Cargo package"
                );
            }
        }
        for segment in rust_module.split("::").skip(1) {
            if !is_python_identifier(segment) {
                bail!(
                    "Rust module `{rust_module}` in Cargo package `{owner}` contains the invalid Python namespace segment `{segment}`; rename the Rust module"
                );
            }
        }
    }
    let mut python_paths = BTreeMap::<Vec<String>, Namespace>::new();
    for namespace in namespace_map.keys() {
        let path = namespace.python_segments();
        if let Some(existing) = python_paths.insert(path.clone(), namespace.clone())
            && existing != *namespace
        {
            bail!(
                "Rust namespaces `{}` and `{}` both derive the Python path `{}`; rename one Cargo package",
                display_namespace(&existing),
                display_namespace(namespace),
                path.join(".")
            );
        }
    }
    for (namespace, items) in namespace_map {
        let namespace_path = namespace.python_segments();
        let child_package_names = python_namespace_paths
            .iter()
            .filter(|path| {
                path.len() == namespace_path.len() + 1
                    && path.starts_with(namespace_path.as_slice())
            })
            .filter_map(|path| path.last().map(String::as_str))
            .collect::<BTreeSet<_>>();
        let mut python_names = items
            .types
            .iter()
            .map(|item| item.name.clone())
            .chain(items.errors.iter().map(|item| item.name.clone()))
            .chain(items.resources.iter().map(|item| item.name.clone()))
            .chain(items.functions.iter().map(|item| item.rust_name.clone()))
            .chain(items.constants.iter().map(|item| item.host_name.clone()))
            .collect::<Vec<_>>();
        for item in &items.types {
            if let rspyts::ir::TypeShape::TaggedEnum { variants, .. } = &item.shape {
                python_names.extend(
                    variants
                        .iter()
                        .map(|variant| tagged_variant_name(&item.name, &variant.rust_name)),
                );
            }
        }
        let mut buffers = BTreeSet::new();
        for reference in namespace_refs(&items) {
            crate::contract::collect_buffers(reference, &mut buffers);
        }
        python_names.extend(
            buffers
                .into_iter()
                .map(|element| python::buffer_name(element).to_owned()),
        );
        if let Some(name) = python_names.iter().find(|name| {
            matches!(
                name.as_str(),
                "__all__" | "__dir__" | "__getattr__" | "api" | "models"
            ) || name.starts_with("_rspyts_models_")
                || (namespace == Namespace::root()
                    && (name.as_str() == "runtime"
                        || name.as_str() == manifest.module_name.as_str()))
                || child_package_names.contains(name.as_str())
        }) {
            bail!(
                "Python export name `{name}` is reserved for generated package loading in namespace `{}`",
                display_namespace(&namespace)
            );
        }
        unique_public_names("Python", python_names.into_iter())
            .with_context(|| format!("in namespace `{}`", display_namespace(&namespace)))?;

        let mut typescript_names = items
            .types
            .iter()
            .map(|item| item.name.clone())
            .chain(items.errors.iter().map(|item| item.name.clone()))
            .chain(items.resources.iter().map(|item| item.name.clone()))
            .chain(items.functions.iter().map(|item| item.host_name.clone()))
            .chain(items.constants.iter().map(|item| item.host_name.clone()))
            .collect::<Vec<_>>();
        for item in &items.types {
            if let rspyts::ir::TypeShape::TaggedEnum { variants, .. } = &item.shape {
                typescript_names.extend(
                    variants
                        .iter()
                        .map(|variant| tagged_variant_name(&item.name, &variant.rust_name)),
                );
            }
        }
        unique_public_names("TypeScript", typescript_names.into_iter())
            .with_context(|| format!("in namespace `{}`", display_namespace(&namespace)))?;
    }
    Ok(())
}

fn export_origins(manifest: &Manifest) -> Vec<(&rspyts::ir::CargoPackageId, &str)> {
    let mut origins = manifest
        .types
        .iter()
        .map(|item| (&item.owner, item.rust_module.as_str()))
        .chain(
            manifest
                .errors
                .iter()
                .map(|item| (&item.owner, item.rust_module.as_str())),
        )
        .chain(
            manifest
                .functions
                .iter()
                .map(|item| (&item.owner, item.rust_module.as_str())),
        )
        .chain(
            manifest
                .resources
                .iter()
                .map(|item| (&item.owner, item.rust_module.as_str())),
        )
        .chain(
            manifest
                .constants
                .iter()
                .map(|item| (&item.owner, item.rust_module.as_str())),
        )
        .collect::<Vec<_>>();
    origins.sort();
    origins.dedup();
    origins
}

fn validate_model_namespace_cycles(manifest: &Manifest) -> Result<()> {
    let mut graph = BTreeMap::<Namespace, BTreeSet<Namespace>>::new();
    for definition in &manifest.types {
        let source = manifest.namespace(&definition.owner, &definition.rust_module);
        graph.entry(source.clone()).or_default();
        for reference in type_refs(definition) {
            let mut identities = Vec::new();
            named_identities(reference, &mut identities);
            for identity in identities {
                let target = type_namespace(identity, manifest)?;
                if target != source {
                    graph.entry(source.clone()).or_default().insert(target);
                }
            }
        }
    }
    let mut complete = BTreeSet::new();
    let mut active = BTreeSet::new();
    let mut stack = Vec::new();
    for namespace in graph.keys() {
        if let Some(cycle) =
            namespace_cycle(namespace, &graph, &mut active, &mut complete, &mut stack)
        {
            let path = cycle
                .iter()
                .map(display_namespace)
                .collect::<Vec<_>>()
                .join(" -> ");
            bail!(
                "Python model namespaces form a dependency cycle: {path}; move the declarations into one Rust module or remove the cyclic type reference"
            );
        }
    }
    Ok(())
}

fn namespace_cycle(
    namespace: &Namespace,
    graph: &BTreeMap<Namespace, BTreeSet<Namespace>>,
    active: &mut BTreeSet<Namespace>,
    complete: &mut BTreeSet<Namespace>,
    stack: &mut Vec<Namespace>,
) -> Option<Vec<Namespace>> {
    if complete.contains(namespace) {
        return None;
    }
    if active.contains(namespace) {
        let start = stack.iter().position(|item| item == namespace).unwrap_or(0);
        let mut cycle = stack[start..].to_vec();
        cycle.push(namespace.clone());
        return Some(cycle);
    }
    active.insert(namespace.clone());
    stack.push(namespace.clone());
    if let Some(targets) = graph.get(namespace) {
        for target in targets {
            if let Some(cycle) = namespace_cycle(target, graph, active, complete, stack) {
                return Some(cycle);
            }
        }
    }
    stack.pop();
    active.remove(namespace);
    complete.insert(namespace.clone());
    None
}

fn display_namespace(namespace: &Namespace) -> String {
    let namespace = namespace.display();
    if namespace.is_empty() {
        "<root>".to_owned()
    } else {
        namespace
    }
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
    if value.is_empty() || value.split('.').any(|part| !is_python_identifier(part)) {
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

fn is_python_identifier(value: &str) -> bool {
    is_identifier(value)
        && !matches!(
            value,
            "False"
                | "None"
                | "True"
                | "and"
                | "as"
                | "assert"
                | "async"
                | "await"
                | "break"
                | "class"
                | "continue"
                | "def"
                | "del"
                | "elif"
                | "else"
                | "except"
                | "finally"
                | "for"
                | "from"
                | "global"
                | "if"
                | "import"
                | "in"
                | "is"
                | "lambda"
                | "nonlocal"
                | "not"
                | "or"
                | "pass"
                | "raise"
                | "return"
                | "try"
                | "while"
                | "with"
                | "yield"
        )
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use rspyts::ir::{
        CargoPackageId, ErrorDef, FieldConstraints, FieldDef, Manifest, TypeDef, TypeRef, TypeShape,
    };

    use super::{validate_generated_files, validate_model_namespace_cycles, validate_namespaces};

    fn model(owner: &str, module: &str, name: &str, references: Vec<TypeRef>) -> TypeDef {
        TypeDef {
            owner: CargoPackageId::new(owner),
            rust_module: module.to_owned(),
            id: format!("{module}::{name}"),
            name: name.to_owned(),
            docs: None,
            shape: TypeShape::Struct {
                fields: references
                    .into_iter()
                    .enumerate()
                    .map(|(index, ty)| FieldDef {
                        rust_name: format!("field_{index}"),
                        wire_name: format!("field{index}"),
                        docs: None,
                        ty,
                        required: true,
                        default: None,
                        constraints: FieldConstraints::default(),
                    })
                    .collect(),
            },
        }
    }

    fn manifest(types: Vec<TypeDef>) -> Manifest {
        Manifest {
            package_name: "app".to_owned(),
            package_version: "1.2.3".to_owned(),
            module_name: "native".to_owned(),
            types,
            errors: Vec::new(),
            functions: Vec::new(),
            resources: Vec::new(),
            constants: Vec::new(),
        }
    }

    #[test]
    fn accepts_an_acyclic_cross_namespace_model_reference() {
        let target = model("app-domain", "app_domain::target", "Target", Vec::new());
        let source = model(
            "app-domain",
            "app_domain::source",
            "Source",
            vec![TypeRef::Named {
                identity: target.identity(),
            }],
        );

        validate_model_namespace_cycles(&manifest(vec![source, target]))
            .expect("an acyclic reference is valid");
    }

    #[test]
    fn permits_a_model_cycle_inside_one_namespace() {
        let mut first = model("app-domain", "app_domain::shared", "First", Vec::new());
        let mut second = model("app-domain", "app_domain::shared", "Second", Vec::new());
        let first_identity = first.identity();
        let second_identity = second.identity();
        let TypeShape::Struct { fields } = &mut first.shape else {
            unreachable!();
        };
        fields.push(FieldDef {
            rust_name: "second".to_owned(),
            wire_name: "second".to_owned(),
            docs: None,
            ty: TypeRef::Named {
                identity: second_identity,
            },
            required: true,
            default: None,
            constraints: FieldConstraints::default(),
        });
        let TypeShape::Struct { fields } = &mut second.shape else {
            unreachable!();
        };
        fields.push(FieldDef {
            rust_name: "first".to_owned(),
            wire_name: "first".to_owned(),
            docs: None,
            ty: TypeRef::Named {
                identity: first_identity,
            },
            required: true,
            default: None,
            constraints: FieldConstraints::default(),
        });

        validate_model_namespace_cycles(&manifest(vec![first, second]))
            .expect("references inside one namespace do not form an import cycle");
    }

    #[test]
    fn reports_the_complete_cross_namespace_model_cycle() {
        let mut first = model("app-domain", "app_domain::first", "First", Vec::new());
        let mut second = model("app-domain", "app_domain::second", "Second", Vec::new());
        let first_identity = first.identity();
        let second_identity = second.identity();
        let TypeShape::Struct { fields } = &mut first.shape else {
            unreachable!();
        };
        fields.push(FieldDef {
            rust_name: "second".to_owned(),
            wire_name: "second".to_owned(),
            docs: None,
            ty: TypeRef::Named {
                identity: second_identity,
            },
            required: true,
            default: None,
            constraints: FieldConstraints::default(),
        });
        let TypeShape::Struct { fields } = &mut second.shape else {
            unreachable!();
        };
        fields.push(FieldDef {
            rust_name: "first".to_owned(),
            wire_name: "first".to_owned(),
            docs: None,
            ty: TypeRef::Named {
                identity: first_identity,
            },
            required: true,
            default: None,
            constraints: FieldConstraints::default(),
        });

        let error = validate_model_namespace_cycles(&manifest(vec![first, second])).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Python model namespaces form a dependency cycle: domain::first -> domain::second -> domain::first; move the declarations into one Rust module or remove the cyclic type reference"
        );
    }

    #[test]
    fn rejects_a_python_keyword_in_a_derived_path() {
        let keyword = model("app-domain", "app_domain::class", "Value", Vec::new());

        let error = validate_namespaces(&manifest(vec![keyword])).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Rust module `app_domain::class` in Cargo package `app-domain` contains the invalid Python namespace segment `class`; rename the Rust module"
        );
    }

    #[test]
    fn identifies_the_cargo_package_that_makes_an_invalid_python_path() {
        let invalid = model("app-123", "app_123", "Value", Vec::new());

        let error = validate_namespaces(&manifest(vec![invalid])).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Cargo package `app-123` derives the invalid Python namespace segment `123`; rename the Cargo package"
        );
    }

    #[test]
    fn rejects_a_cross_kind_name_collision_in_one_namespace() {
        let item = model("app-domain", "app_domain::shared", "Conflict", Vec::new());
        let mut contract = manifest(vec![item]);
        contract.errors.push(ErrorDef {
            owner: CargoPackageId::new("app-domain"),
            rust_module: "app_domain::shared".to_owned(),
            id: "app_domain::shared::ConflictError".to_owned(),
            name: "Conflict".to_owned(),
            docs: None,
        });

        let error = validate_namespaces(&contract).unwrap_err();
        let diagnostic = format!("{error:#}");

        assert!(diagnostic.contains("in namespace `domain::shared`"));
        assert!(diagnostic.contains("duplicate Python export name `Conflict`"));
    }

    #[test]
    fn rejects_names_reserved_for_generated_python_package_loading() {
        for name in ["__getattr__", "_rspyts_models_0", "api", "models"] {
            let item = model("app-domain", "app_domain::shared", name, Vec::new());
            let error = validate_namespaces(&manifest(vec![item])).unwrap_err();

            assert_eq!(
                error.to_string(),
                format!(
                    "Python export name `{name}` is reserved for generated package loading in namespace `domain::shared`"
                )
            );
        }
    }

    #[test]
    fn rejects_root_names_used_by_generated_python_runtime_modules() {
        for name in ["runtime", "native"] {
            let item = model("app", "app", name, Vec::new());
            let error = validate_namespaces(&manifest(vec![item])).unwrap_err();

            assert_eq!(
                error.to_string(),
                format!(
                    "Python export name `{name}` is reserved for generated package loading in namespace `<root>`"
                )
            );
        }
    }

    #[test]
    fn rejects_an_export_that_shadows_a_child_python_package() {
        let export = model("app", "app", "child", Vec::new());
        let child = model("app", "app::child", "Value", Vec::new());
        let error = validate_namespaces(&manifest(vec![export, child])).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Python export name `child` is reserved for generated package loading in namespace `<root>`"
        );
    }

    #[test]
    fn rejects_cargo_names_that_collapse_to_one_python_path() {
        let first = model("app-foo-bar", "app_foo_bar", "First", Vec::new());
        let second = model("app-foo_bar", "app_foo_bar", "Second", Vec::new());

        let error = validate_namespaces(&manifest(vec![first, second])).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Rust namespaces `foo-bar` and `foo_bar` both derive the Python path `foo_bar`; rename one Cargo package"
        );
    }

    #[test]
    fn sync_check_reports_stale_flat_files() {
        let expected = BTreeMap::from([(
            PathBuf::from("python/example/domain/api.py"),
            b"new\n".to_vec(),
        )]);
        let actual = BTreeMap::from([
            (
                PathBuf::from("python/example/domain/api.py"),
                b"new\n".to_vec(),
            ),
            (PathBuf::from("python/example/api.py"), b"stale\n".to_vec()),
        ]);

        let error = validate_generated_files(&expected, &actual).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("extra: [\"python/example/api.py\"]")
        );
        assert!(error.to_string().contains("run `rspyts build`"));
    }
}
