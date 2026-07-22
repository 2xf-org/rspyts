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

pub(crate) use validation::*;

const CONTRACT_SYMBOL: &str = "rspyts_discovery_v1_contract";
const FREE_SYMBOL: &str = "rspyts_discovery_v1_contract_free";

#[derive(Debug, Clone)]
pub(super) struct Project {
    pub(super) root: PathBuf,
    pub(super) workspace_root: PathBuf,
    target_directory: PathBuf,
    linked_packages: Vec<LinkedPackage>,
    rspyts_dependency: ResolvedDependency,
    pub(super) package_name: String,
    pub(super) package_version: String,
    pub(super) python_package: String,
    pub(super) typescript_package: String,
    python_source: PathBuf,
    typescript_source: PathBuf,
    pub(super) config: Config,
}

#[derive(Debug, Clone)]
struct LinkedPackage {
    name: String,
    manifest: PathBuf,
}

#[derive(Debug, Clone)]
struct ResolvedDependency {
    root: PathBuf,
}

impl Project {
    pub(super) fn read(path: &Path) -> Result<Self> {
        let config = Config::read(path)?;
        let requested_manifest = config.root().join("Cargo.toml");
        let requested_manifest = requested_manifest
            .canonicalize()
            .with_context(|| format!("cannot find Cargo.toml beside {}", config.path.display()))?;
        let output = ProcessCommand::new(cargo())
            .args(["metadata", "--format-version", "1", "--manifest-path"])
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
        let primary = packages
            .iter()
            .find(|package| {
                package["manifest_path"]
                    .as_str()
                    .and_then(|value| Path::new(value).canonicalize().ok())
                    .as_ref()
                    == Some(&requested_manifest)
            })
            .context("rspyts.toml must be beside a Cargo package manifest")?;
        let primary_id = string(primary, "id")?;
        if !workspace_members.iter().any(|member| member == primary_id) {
            bail!("the Cargo package beside rspyts.toml must be a workspace member");
        }

        let mut selected = vec![primary];
        for name in &config.application.additional_packages {
            if selected
                .iter()
                .any(|package| package["name"].as_str() == Some(name))
            {
                bail!("rspyts application package `{name}` is listed more than once");
            }
            let mut matches = packages.iter().filter(|package| {
                package["name"].as_str() == Some(name)
                    && package["id"]
                        .as_str()
                        .is_some_and(|id| workspace_members.iter().any(|member| member == id))
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
                    string(package, "name")?
                );
            }
        }

        let rspyts_id = selected
            .iter()
            .map(|package| direct_rspyts_id(&metadata, string(package, "id")?))
            .collect::<Result<Vec<_>>>()?;
        if rspyts_id.iter().any(|id| id != &rspyts_id[0]) {
            bail!("all rspyts application packages must resolve the same `rspyts` package");
        }
        let rspyts_package = packages
            .iter()
            .find(|package| package["id"].as_str() == Some(&rspyts_id[0]))
            .context("Cargo metadata omitted the resolved rspyts package")?;
        let rspyts_dependency = resolved_dependency(rspyts_package)?;

        let primary_package_name = string(primary, "name")?.to_owned();
        let package_name = config
            .application
            .name
            .clone()
            .unwrap_or_else(|| primary_package_name.clone());
        validate_application_name(&package_name)?;
        let package_version = string(primary, "version")?.to_owned();
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
        let workspace_root = PathBuf::from(string(&metadata, "workspace_root")?);
        let target_directory = PathBuf::from(string(&metadata, "target_directory")?);
        let python_source = root.join("src-py");
        let typescript_source = root.join("src-ts");
        let linked_packages = selected
            .iter()
            .map(|package| {
                Ok(LinkedPackage {
                    name: string(package, "name")?.to_owned(),
                    manifest: PathBuf::from(string(package, "manifest_path")?),
                })
            })
            .collect::<Result<Vec<_>>>()?;

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

    pub(super) fn python_source(&self) -> &Path {
        &self.python_source
    }

    pub(super) fn typescript_source(&self) -> &Path {
        &self.typescript_source
    }

    pub(super) fn config_path(&self) -> &Path {
        &self.config.path
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BuildReport {
    status: &'static str,
    python_source: PathBuf,
    typescript_source: PathBuf,
    python_package: String,
    typescript_package: String,
}

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

fn current_config(project: &Project) -> Result<Config> {
    let config = Config::read(project.config_path())?;
    if config.application != project.config.application {
        bail!("[application] in rspyts.toml changed during the command; run it again");
    }
    Ok(config)
}

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

#[derive(Clone, Copy)]
enum CompileKind {
    Native,
    Wasm,
}

struct Bridge {
    manifest: PathBuf,
    package_id: String,
    package_name: String,
}

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
    let output = ProcessCommand::new(cargo())
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(&bridge_manifest)
        .output()
        .context("failed to inspect the generated rspyts bridge")?;
    if !output.status.success() {
        bail!(
            "generated rspyts bridge metadata failed\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let package = metadata["packages"]
        .as_array()
        .and_then(|packages| packages.first())
        .context("generated rspyts bridge metadata has no package")?;
    Ok(Bridge {
        manifest: bridge_manifest,
        package_id: string(package, "id")?.to_owned(),
        package_name,
    })
}

fn compile(project: &Project, bridge: &Bridge, kind: CompileKind) -> Result<PathBuf> {
    let (feature, label) = match kind {
        CompileKind::Native => (Some("rspyts/python-extension"), "Python"),
        CompileKind::Wasm => (None, "WebAssembly"),
    };
    let mut command = ProcessCommand::new(cargo());
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

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .with_context(|| format!("Cargo metadata has no `{key}` string"))
}

fn has_library_target(package: &Value) -> bool {
    package["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|target| {
            target["kind"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|kind| kind == "lib")
                || target["crate_types"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|kind| matches!(kind.as_str(), Some("lib" | "rlib" | "cdylib")))
        })
}

fn direct_rspyts_id(metadata: &Value, package_id: &str) -> Result<String> {
    let packages = metadata["packages"]
        .as_array()
        .context("Cargo metadata has no package list")?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .context("Cargo metadata has no dependency graph")?;
    let node = nodes
        .iter()
        .find(|node| node["id"].as_str() == Some(package_id))
        .context("Cargo metadata omitted an application package from its dependency graph")?;
    let mut matches = node["deps"]
        .as_array()
        .context("Cargo metadata dependency node has no dependency list")?
        .iter()
        .filter(|dependency| {
            dependency["dep_kinds"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|kind| kind["kind"].is_null())
        })
        .filter_map(|dependency| dependency["pkg"].as_str())
        .filter(|dependency_id| {
            packages.iter().any(|package| {
                package["id"].as_str() == Some(*dependency_id)
                    && package["name"].as_str() == Some("rspyts")
            })
        });
    let dependency = matches
        .next()
        .context("each rspyts application package must directly depend on `rspyts`")?;
    if matches.next().is_some() {
        bail!("an rspyts application package resolves multiple direct `rspyts` dependencies");
    }
    Ok(dependency.to_owned())
}

fn resolved_dependency(package: &Value) -> Result<ResolvedDependency> {
    let manifest = PathBuf::from(string(package, "manifest_path")?);
    let root = manifest
        .parent()
        .context("resolved rspyts manifest has no parent")?
        .canonicalize()
        .context("cannot resolve the selected rspyts package source")?;
    Ok(ResolvedDependency { root })
}

fn bridge_dependency(dependency: &ResolvedDependency) -> String {
    format!(
        "{{ path = {} }}",
        toml_string(&dependency.root.to_string_lossy())
    )
}

fn stable_key(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).expect("serializing a string cannot fail")
}

fn write_if_changed(path: &Path, source: &str) -> Result<()> {
    if fs::read(path).is_ok_and(|current| current == source.as_bytes()) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, source).with_context(|| format!("failed to write {}", path.display()))
}

fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}
