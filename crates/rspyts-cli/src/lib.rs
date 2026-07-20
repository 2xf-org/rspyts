use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, c_char};
use std::fmt::Write as FmtWrite;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use fs4::FileExt;
use libloading::{Library, Symbol};
use rspyts::ir::{
    BufferElement, DefinitionId, FieldDef, FunctionDef, Manifest, ParamDef, ResourceDef,
    ScalarValue, TypeDef, TypeRef, TypeShape,
};
use serde::Serialize;
use serde_json::{Value, json};
use tempfile::TempDir;

const CONTRACT_SYMBOL: &str = "rspyts_discovery_v1_contract";
const FREE_SYMBOL: &str = "rspyts_discovery_v1_contract_free";

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
    /// Build the Python and TypeScript packages.
    Build(ProjectArgs),
    /// Rebuild when Rust or Cargo files change.
    Watch(ProjectArgs),
    /// Check that dist matches the Rust source.
    Check(ProjectArgs),
}

#[derive(Debug, Args)]
struct ProjectArgs {
    /// Path to the aggregate binding Cargo.toml.
    #[arg(long, default_value = "Cargo.toml")]
    manifest_path: PathBuf,
}

#[derive(Debug, Clone)]
struct Project {
    root: PathBuf,
    workspace_root: PathBuf,
    manifest: PathBuf,
    package_id: String,
    package_name: String,
    package_version: String,
    python_package: String,
    typescript_package: String,
}

impl Project {
    fn read(path: &Path) -> Result<Self> {
        let manifest = path
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
            .arg(&manifest)
            .output()
            .context("failed to run cargo metadata")?;
        if !output.status.success() {
            bail!(
                "cargo metadata failed\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let metadata: Value = serde_json::from_slice(&output.stdout)?;
        let package = metadata["packages"]
            .as_array()
            .and_then(|packages| {
                packages.iter().find(|package| {
                    package["manifest_path"]
                        .as_str()
                        .and_then(|value| Path::new(value).canonicalize().ok())
                        .as_ref()
                        == Some(&manifest)
                })
            })
            .context("the manifest does not describe a Cargo package")?;
        let package_name = string(package, "name")?.to_owned();
        let package_version = string(package, "version")?.to_owned();
        let package_id = string(package, "id")?.to_owned();
        let features = package["features"]
            .as_object()
            .context("Cargo metadata has no feature table")?;
        for feature in ["python", "wasm"] {
            if !features.contains_key(feature) {
                bail!("aggregate binding `{package_name}` must define a `{feature}` Cargo feature");
            }
        }
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
        if !has_cdylib {
            bail!(
                "aggregate binding `{package_name}` must set crate-type = [\"cdylib\", \"rlib\"]"
            );
        }

        let settings = package["metadata"]["rspyts"].as_object();
        let python_package = settings
            .and_then(|value| value.get("python"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| package_name.replace('-', "_"));
        let typescript_package = settings
            .and_then(|value| value.get("typescript"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| package_name.clone());
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

    fn output(&self) -> PathBuf {
        self.root.join("dist")
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildReport {
    status: &'static str,
    output: PathBuf,
    python_package: String,
    typescript_package: String,
}

pub fn run() -> Result<()> {
    run_from(Cli::parse())
}

fn run_from(cli: Cli) -> Result<()> {
    match cli.command {
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

fn build(project: &Project) -> Result<BuildReport> {
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

fn check(project: &Project) -> Result<()> {
    let _lock = project_lock(project)?;
    let generated = generate(project)?;
    let expected = file_tree(generated.path())?;
    let actual = file_tree(&project.output()).with_context(|| {
        format!(
            "{} does not exist; run `rspyts build --manifest-path {}`",
            project.output().display(),
            project.manifest.display()
        )
    })?;
    if expected != actual {
        let expected_names = expected.keys().cloned().collect::<BTreeSet<_>>();
        let actual_names = actual.keys().cloned().collect::<BTreeSet<_>>();
        let changed = expected_names
            .intersection(&actual_names)
            .filter(|path| expected.get(*path) != actual.get(*path))
            .cloned()
            .collect::<Vec<_>>();
        bail!(
            "dist is not in sync (missing: {:?}; extra: {:?}; changed: {:?}); run `rspyts build --manifest-path {}`",
            expected_names.difference(&actual_names).collect::<Vec<_>>(),
            actual_names.difference(&expected_names).collect::<Vec<_>>(),
            changed,
            project.manifest.display(),
        );
    }
    Ok(())
}

fn generate(project: &Project) -> Result<TempDir> {
    let temporary = tempfile::Builder::new()
        .prefix(".rspyts-")
        .tempdir_in(&project.root)?;
    let probe = compile(project, CompileKind::Probe)?;
    let manifest = read_contract(&probe, &project.package_name)?;
    validate_contract(project, &manifest)?;
    let native = compile(project, CompileKind::Native)?;
    let wasm = compile(project, CompileKind::Wasm)?;

    write_json(&temporary.path().join("contract.json"), &manifest)?;
    emit_python(project, &manifest, &native, temporary.path())?;
    emit_typescript(project, &manifest, &wasm, temporary.path())?;
    Ok(temporary)
}

#[derive(Clone, Copy)]
enum CompileKind {
    Probe,
    Native,
    Wasm,
}

fn compile(project: &Project, kind: CompileKind) -> Result<PathBuf> {
    let feature = match kind {
        CompileKind::Probe => None,
        CompileKind::Native => Some("python"),
        CompileKind::Wasm => Some("wasm"),
    };
    let mut command = ProcessCommand::new(cargo());
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(&project.manifest)
        .arg("--package")
        .arg(&project.package_name)
        .arg("--release")
        .arg("--locked")
        .arg("--no-default-features")
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
        format!(
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
            format!("--remap-path-prefix={}=/cargo", cargo_home.display()),
        );
    }
    if matches!(kind, CompileKind::Native) && cfg!(target_os = "macos") {
        append_rust_flag(&mut flags, "-C".into());
        append_rust_flag(&mut flags, "link-arg=-undefined".into());
        append_rust_flag(&mut flags, "-C".into());
        append_rust_flag(&mut flags, "link-arg=dynamic_lookup".into());
    }
    command.env("RUSTFLAGS", flags);
    let label = feature.unwrap_or("contract probe");
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

fn append_rust_flag(flags: &mut String, value: String) {
    if !flags.is_empty() {
        flags.push(' ');
    }
    flags.push_str(&value);
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
                    CompileKind::Probe | CompileKind::Native => path
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
                "missing `{}`; add `rspyts::application!(native);`",
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
            .map(|element| python_buffer_name(element).to_owned()),
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

fn unique_public_names<S: AsRef<str>>(host: &str, names: impl Iterator<Item = S>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for name in names {
        let name = name.as_ref();
        if !seen.insert(name.to_owned()) {
            bail!("duplicate {host} export name `{name}`");
        }
    }
    Ok(())
}

fn emit_python(project: &Project, manifest: &Manifest, native: &Path, root: &Path) -> Result<()> {
    let python_root = root.join("python");
    let package = python_root.join(project.python_package.replace('.', "/"));
    fs::create_dir_all(&package)?;
    let mut parent = python_root.clone();
    for segment in project
        .python_package
        .split('.')
        .collect::<Vec<_>>()
        .iter()
        .take(project.python_package.split('.').count().saturating_sub(1))
    {
        parent.push(segment);
        write(&parent.join("__init__.py"), "")?;
    }

    let extension = if cfg!(windows) { "pyd" } else { "so" };
    fs::copy(
        native,
        package.join(format!("{}.{}", manifest.module_name, extension)),
    )
    .with_context(|| format!("failed to copy Python extension {}", native.display()))?;
    write(&package.join("models.py"), &python_models(manifest)?)?;
    write(&package.join("api.py"), &python_api(manifest)?)?;
    write(&package.join("__init__.py"), &python_init(manifest))?;
    write(&package.join("py.typed"), "")?;
    write(
        &package.join(format!("{}.pyi", manifest.module_name)),
        &python_native_stub(manifest),
    )?;

    let distribution = project.python_package.replace('.', "-");
    let dependencies = if uses_buffer(manifest) {
        "dependencies = [\"pydantic>=2,<3\", \"numpy>=2,<3\"]"
    } else {
        "dependencies = [\"pydantic>=2,<3\"]"
    };
    let pyproject = format!(
        "[build-system]\nrequires = [\"setuptools>=77\", \"wheel>=0.45\"]\nbuild-backend = \"setuptools.build_meta\"\n\n[project]\nname = {}\nversion = {}\nrequires-python = \">=3.11\"\n{}\n\n[tool.setuptools.packages.find]\nwhere = [\".\"]\n\n[tool.setuptools.package-data]\n\"*\" = [\"*.so\", \"*.pyd\", \"*.pyi\", \"py.typed\"]\n",
        py_string(&distribution),
        py_string(&manifest.package_version),
        dependencies,
    );
    write(&python_root.join("pyproject.toml"), &pyproject)?;
    write(
        &python_root.join("setup.py"),
        "from setuptools import Distribution, setup\n\n\nclass BinaryDistribution(Distribution):\n    def has_ext_modules(self) -> bool:\n        return True\n\n\nsetup(distclass=BinaryDistribution)\n",
    )?;
    write(
        &python_root.join("setup.cfg"),
        "[bdist_wheel]\npy_limited_api = cp311\n",
    )
}

fn python_models(manifest: &Manifest) -> Result<String> {
    let mut source = String::from(
        "from __future__ import annotations\n\nfrom datetime import datetime\nfrom enum import StrEnum\nfrom typing import Annotated, Any, Literal, TypeAlias\n\nfrom pydantic import BaseModel, ConfigDict, Field, RootModel\n",
    );
    let buffers = buffer_elements(manifest);
    if !buffers.is_empty() {
        source.push_str("from pydantic.functional_serializers import PlainSerializer\nfrom pydantic.functional_validators import BeforeValidator\nimport numpy as np\nfrom numpy.typing import NDArray\n");
    }
    source.push('\n');
    for element in buffers {
        let name = python_buffer_name(element);
        let scalar = python_numpy_scalar(element);
        writeln!(
            source,
            "{name}: TypeAlias = Annotated[NDArray[np.{scalar}], BeforeValidator(lambda value: np.asarray(value, dtype=np.{scalar})), PlainSerializer(lambda value: value.tolist(), return_type=list)]"
        )?;
    }
    if uses_buffer(manifest) {
        source.push('\n');
    }

    for definition in &manifest.types {
        emit_python_type(&mut source, definition, manifest)?;
    }
    for definition in &manifest.types {
        match definition.shape {
            TypeShape::Struct { .. } | TypeShape::Alias { .. } => {
                writeln!(source, "{}.model_rebuild()", definition.name)?;
            }
            TypeShape::TaggedEnum { ref variants, .. } => {
                for variant in variants {
                    writeln!(
                        source,
                        "{}.model_rebuild()",
                        tagged_variant_name(&definition.name, &variant.rust_name)
                    )?;
                }
            }
            TypeShape::StringEnum { .. } => {}
        }
    }
    Ok(source)
}

fn emit_python_type(source: &mut String, definition: &TypeDef, manifest: &Manifest) -> Result<()> {
    match &definition.shape {
        TypeShape::Struct { fields } => {
            writeln!(source, "\nclass {}(BaseModel):", definition.name)?;
            emit_python_doc(source, definition.docs.as_deref(), "    ")?;
            source.push_str("    model_config = ConfigDict(frozen=True, strict=True, populate_by_name=True, extra=\"forbid\", arbitrary_types_allowed=True)\n");
            if fields.is_empty() {
                source.push_str("    pass\n");
            }
            for field in fields {
                emit_python_field(source, field, manifest, "    ")?;
            }
        }
        TypeShape::StringEnum { variants } => {
            writeln!(source, "\nclass {}(StrEnum):", definition.name)?;
            emit_python_doc(source, definition.docs.as_deref(), "    ")?;
            if variants.is_empty() {
                source.push_str("    pass\n");
            }
            for variant in variants {
                writeln!(
                    source,
                    "    {} = {}",
                    variant.rust_name,
                    py_string(&variant.wire_name)
                )?;
            }
        }
        TypeShape::TaggedEnum { tag, variants } => {
            for variant in variants {
                let name = tagged_variant_name(&definition.name, &variant.rust_name);
                writeln!(source, "\nclass {name}(BaseModel):")?;
                emit_python_doc(source, variant.docs.as_deref(), "    ")?;
                source.push_str("    model_config = ConfigDict(frozen=True, strict=True, populate_by_name=True, extra=\"forbid\", arbitrary_types_allowed=True)\n");
                writeln!(
                    source,
                    "    {}: Literal[{}] = Field(default={}, alias={})",
                    safe_python_name(tag),
                    py_string(&variant.wire_name),
                    py_string(&variant.wire_name),
                    py_string(tag),
                )?;
                for field in &variant.fields {
                    emit_python_field(source, field, manifest, "    ")?;
                }
            }
            let names = variants
                .iter()
                .map(|variant| tagged_variant_name(&definition.name, &variant.rust_name))
                .collect::<Vec<_>>()
                .join(" | ");
            writeln!(source, "\n{}: TypeAlias = {}", definition.name, names)?;
        }
        TypeShape::Alias { target } => {
            writeln!(
                source,
                "\nclass {}(RootModel[{}]):\n    pass",
                definition.name,
                python_ref(target, manifest)?
            )?;
        }
    }
    Ok(())
}

fn emit_python_field(
    source: &mut String,
    field: &FieldDef,
    manifest: &Manifest,
    indent: &str,
) -> Result<()> {
    if let Some(docs) = field.docs.as_deref() {
        writeln!(source, "{indent}# {}", docs.replace('\n', " "))?;
    }
    let annotation = if let Some(literal) = &field.constraints.literal {
        format!("Literal[{}]", python_scalar(literal))
    } else {
        python_ref(&field.ty, manifest)?
    };
    let default = if field.required {
        "...".to_owned()
    } else if let Some(value) = &field.default {
        python_scalar(value)
    } else {
        "None".to_owned()
    };
    let mut options = vec![format!("default={default}")];
    if field.wire_name != field.rust_name {
        options.push(format!("alias={}", py_string(&field.wire_name)));
    }
    if let Some(value) = field.constraints.min_length {
        options.push(format!("min_length={value}"));
    }
    if let Some(value) = field.constraints.max_length {
        options.push(format!("max_length={value}"));
    }
    if let Some(value) = field.constraints.ge {
        options.push(format!("ge={value}"));
    }
    if let Some(value) = field.constraints.le {
        options.push(format!("le={value}"));
    }
    writeln!(
        source,
        "{indent}{}: {annotation} = Field({})",
        safe_python_name(&field.rust_name),
        options.join(", ")
    )?;
    Ok(())
}

fn python_api(manifest: &Manifest) -> Result<String> {
    let mut source = String::from(
        "from __future__ import annotations\n\nfrom datetime import date, datetime\nfrom typing import Any, Final\n\nfrom pydantic import BaseModel, ConfigDict, TypeAdapter\n",
    );
    if uses_buffer(manifest) {
        source.push_str("import numpy as np\n");
    }
    let model_names = python_model_names(manifest);
    if !model_names.is_empty() {
        writeln!(source, "\nfrom .models import {}", model_names.join(", "))?;
    }
    writeln!(source, "from . import {} as native\n", manifest.module_name)?;
    source.push_str(PYTHON_ADAPTERS);
    writeln!(source, "\nnative_schemas: dict[str, Any] = {{")?;
    for definition in &manifest.types {
        writeln!(
            source,
            "    {}: {},",
            py_string(&definition.name),
            python_named_spec(definition, manifest)?
        )?;
    }
    source.push_str("}\n");

    for error in &manifest.errors {
        writeln!(source, "\nclass {}(RuntimeError):", error.name)?;
        emit_python_doc(&mut source, error.docs.as_deref(), "    ")?;
        source.push_str("    def __init__(self, code: str, message: str) -> None:\n        super().__init__(message)\n        self.code = code\n\n");
    }
    for function in &manifest.functions {
        emit_python_function(&mut source, function, manifest, None)?;
    }
    for resource in &manifest.resources {
        emit_python_resource(&mut source, resource, manifest)?;
    }
    for constant in &manifest.constants {
        let value = python_json(&constant.value);
        let ty = python_ref(&constant.ty, manifest)?;
        if is_plain_python_constant(&constant.ty) {
            writeln!(source, "\n{}: Final[{ty}] = {value}", constant.host_name)?;
        } else {
            writeln!(
                source,
                "\n{}: Final[{ty}] = {}.validate_python(restore_host({value}, {}))",
                constant.host_name,
                python_type_adapter(&constant.ty, manifest)?,
                python_spec(&constant.ty, manifest)?
            )?;
        }
    }
    Ok(source)
}

const PYTHON_ADAPTERS: &str = r#"
def prepare_host(value: Any) -> Any:
    if isinstance(value, BaseModel):
        return prepare_host(value.model_dump(mode="python", by_alias=True))
    if isinstance(value, (datetime, date)):
        return value.isoformat()
    if isinstance(value, bytes):
        return list(value)
    if "np" in globals() and isinstance(value, np.ndarray):
        return value.tolist()
    if isinstance(value, dict):
        return {key: prepare_host(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [prepare_host(item) for item in value]
    return value

def restore_host(value: Any, spec: Any) -> Any:
    if value is None or spec is None:
        return value
    kind = spec[0]
    if kind == "bytes":
        return bytes(value)
    if kind == "buffer":
        return np.asarray(value, dtype=spec[1])
    if kind == "list":
        return [restore_host(item, spec[1]) for item in value]
    if kind == "map":
        return {key: restore_host(item, spec[1]) for key, item in value.items()}
    if kind == "tuple":
        return tuple(restore_host(item, item_spec) for item, item_spec in zip(value, spec[1]))
    if kind == "named":
        return restore_host(value, native_schemas.get(spec[1]))
    if kind == "alias":
        return restore_host(value, spec[1])
    if kind == "struct":
        return {key: restore_host(item, spec[1].get(key)) for key, item in value.items()}
    if kind == "tagged":
        fields = spec[2].get(value.get(spec[1]), {})
        return {key: restore_host(item, fields.get(key)) for key, item in value.items()}
    return value

def native_error(error: RuntimeError, error_type: type[RuntimeError]) -> RuntimeError:
    if len(error.args) == 2:
        return error_type(str(error.args[0]), str(error.args[1]))
    return error
"#;

fn emit_python_function(
    source: &mut String,
    function: &FunctionDef,
    manifest: &Manifest,
    receiver: Option<&str>,
) -> Result<()> {
    let params = function
        .params
        .iter()
        .map(|param| python_param(param, manifest))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let call_params = function
        .params
        .iter()
        .map(|param| format!("prepare_host({})", safe_python_name(&param.rust_name)))
        .collect::<Vec<_>>()
        .join(", ");
    let return_type = python_ref(&function.returns, manifest)?;
    let (indent, signature, call) = if receiver.is_some() {
        (
            "    ",
            format!(
                "    def {}(self{}{}) -> {return_type}:",
                function.rust_name,
                if params.is_empty() { "" } else { ", " },
                params
            ),
            format!("self.native_resource.{}({call_params})", function.host_name),
        )
    } else {
        (
            "",
            format!("def {}({params}) -> {return_type}:", function.rust_name),
            format!("native.{}({call_params})", function.host_name),
        )
    };
    writeln!(source, "\n{signature}")?;
    emit_python_doc(source, function.docs.as_deref(), &format!("{indent}    "))?;
    if function.error.is_some() {
        writeln!(source, "{indent}    try:")?;
        writeln!(source, "{indent}        result = {call}")?;
        writeln!(source, "{indent}    except RuntimeError as error:")?;
        let error_name = error_name(function.error.as_ref(), manifest)?;
        writeln!(
            source,
            "{indent}        raise native_error(error, {error_name}) from None"
        )?;
    } else {
        writeln!(source, "{indent}    result = {call}")?;
    }
    if matches!(function.returns, TypeRef::Unit) {
        writeln!(source, "{indent}    return None")?;
    } else {
        writeln!(
            source,
            "{indent}    return {}.validate_python(restore_host(result, {}))",
            python_type_adapter(&function.returns, manifest)?,
            python_spec(&function.returns, manifest)?
        )?;
    }
    Ok(())
}

fn emit_python_resource(
    source: &mut String,
    resource: &ResourceDef,
    manifest: &Manifest,
) -> Result<()> {
    let constructor = resource
        .constructors
        .iter()
        .find(|item| item.rust_name == "new")
        .or_else(|| resource.constructors.first())
        .context("resource has no constructor")?;
    writeln!(source, "\nclass {}:", resource.name)?;
    emit_python_doc(source, resource.docs.as_deref(), "    ")?;
    let params = constructor
        .params
        .iter()
        .map(|param| python_param(param, manifest))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let calls = constructor
        .params
        .iter()
        .map(|param| format!("prepare_host({})", safe_python_name(&param.rust_name)))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        source,
        "    def __init__(self{}{}) -> None:",
        if params.is_empty() { "" } else { ", " },
        params
    )?;
    let native_call = format!("native.{}({calls})", resource.name);
    if constructor.error.is_some() {
        writeln!(source, "        try:")?;
        writeln!(source, "            self.native_resource = {native_call}")?;
        writeln!(source, "        except RuntimeError as error:")?;
        writeln!(
            source,
            "            raise native_error(error, {}) from None",
            error_name(constructor.error.as_ref(), manifest)?
        )?;
    } else {
        writeln!(source, "        self.native_resource = {native_call}")?;
    }
    for factory in resource
        .constructors
        .iter()
        .filter(|item| !std::ptr::eq(*item, constructor))
    {
        let params = factory
            .params
            .iter()
            .map(|param| python_param(param, manifest))
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        let calls = factory
            .params
            .iter()
            .map(|param| format!("prepare_host({})", safe_python_name(&param.rust_name)))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            source,
            "\n    @classmethod\n    def {}(cls, {params}) -> {}:",
            factory.rust_name, resource.name
        )?;
        writeln!(source, "        value = cls.__new__(cls)")?;
        let native_call = format!("native.{}.{}({calls})", resource.name, factory.host_name);
        if factory.error.is_some() {
            writeln!(source, "        try:")?;
            writeln!(source, "            value.native_resource = {native_call}")?;
            writeln!(source, "        except RuntimeError as error:")?;
            writeln!(
                source,
                "            raise native_error(error, {}) from None",
                error_name(factory.error.as_ref(), manifest)?
            )?;
        } else {
            writeln!(source, "        value.native_resource = {native_call}")?;
        }
        writeln!(source, "        return value")?;
    }
    for method in &resource.methods {
        let function = FunctionDef {
            owner: resource.owner.clone(),
            rust_name: method.rust_name.clone(),
            host_name: method.host_name.clone(),
            docs: method.docs.clone(),
            params: method.params.clone(),
            returns: method.returns.clone(),
            error: method.error.clone(),
        };
        emit_python_function(source, &function, manifest, Some(&resource.name))?;
    }
    source.push_str("\n    def close(self) -> None:\n        self.native_resource.close()\n");
    Ok(())
}

fn python_init(manifest: &Manifest) -> String {
    let mut model_names = python_model_names(manifest);
    let mut api_names = manifest
        .errors
        .iter()
        .map(|item| item.name.clone())
        .chain(manifest.functions.iter().map(|item| item.rust_name.clone()))
        .chain(manifest.resources.iter().map(|item| item.name.clone()))
        .chain(manifest.constants.iter().map(|item| item.host_name.clone()))
        .collect::<Vec<_>>();
    model_names.sort();
    api_names.sort();
    let mut source = String::from("\"\"\"Generated from the Rust application API.\"\"\"\n\n");
    if !model_names.is_empty() {
        writeln!(source, "from .models import {}", model_names.join(", ")).unwrap();
    }
    if !api_names.is_empty() {
        writeln!(source, "from .api import {}", api_names.join(", ")).unwrap();
    }
    let mut all = model_names;
    all.extend(api_names);
    writeln!(
        source,
        "\n__all__ = [{}]",
        all.iter()
            .map(|item| py_string(item))
            .collect::<Vec<_>>()
            .join(", ")
    )
    .unwrap();
    source
}

fn python_native_stub(manifest: &Manifest) -> String {
    let mut source = String::from("from typing import Any\n\n");
    for function in &manifest.functions {
        let params = function
            .params
            .iter()
            .map(|param| format!("{}: Any", safe_python_name(&param.rust_name)))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(source, "def {}({params}) -> Any: ...", function.host_name).unwrap();
    }
    for resource in &manifest.resources {
        writeln!(source, "\nclass {}:", resource.name).unwrap();
        source.push_str("    def __init__(self, *args: Any) -> None: ...\n");
        for method in &resource.methods {
            writeln!(
                source,
                "    def {}(self, *args: Any) -> Any: ...",
                method.host_name
            )
            .unwrap();
        }
        source.push_str("    def close(self) -> None: ...\n");
    }
    source
}

fn emit_typescript(project: &Project, manifest: &Manifest, wasm: &Path, root: &Path) -> Result<()> {
    let package = root.join("typescript");
    fs::create_dir_all(&package)?;
    let output = ProcessCommand::new("wasm-bindgen")
        .arg(wasm)
        .args(["--target", "web", "--out-name", "native", "--out-dir"])
        .arg(&package)
        .output()
        .context("wasm-bindgen is not installed on PATH")?;
    if !output.status.success() {
        bail!(
            "wasm-bindgen failed\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    write(
        &package.join("index.d.ts"),
        &typescript_declarations(manifest)?,
    )?;
    write(&package.join("index.js"), &typescript_runtime(manifest)?)?;
    let package_json = json!({
        "name": project.typescript_package,
        "version": manifest.package_version,
        "type": "module",
        "sideEffects": true,
        "exports": {
            ".": {
                "types": "./index.d.ts",
                "default": "./index.js"
            }
        },
        "files": ["index.js", "index.d.ts", "native.js", "native.d.ts", "native_bg.wasm"]
    });
    write_json(&package.join("package.json"), &package_json)
}

fn typescript_declarations(manifest: &Manifest) -> Result<String> {
    let mut source = String::from(
        "export type JsonValue = null | boolean | number | string | JsonValue[] | { readonly [key: string]: JsonValue };\n",
    );
    for definition in &manifest.types {
        emit_typescript_type(&mut source, definition, manifest)?;
    }
    for error in &manifest.errors {
        writeln!(
            source,
            "\nexport class {} extends Error {{\n  readonly code: string;\n  constructor(code: string, message: string);\n}}",
            error.name
        )?;
    }
    for function in &manifest.functions {
        writeln!(
            source,
            "\nexport function {}({}): {};",
            function.host_name,
            typescript_params(&function.params, manifest)?,
            typescript_ref(&function.returns, manifest)?
        )?;
    }
    for resource in &manifest.resources {
        emit_typescript_resource_declaration(&mut source, resource, manifest)?;
    }
    for constant in &manifest.constants {
        writeln!(
            source,
            "\nexport const {}: {};",
            constant.host_name,
            typescript_ref(&constant.ty, manifest)?
        )?;
    }
    Ok(source)
}

fn emit_typescript_type(
    source: &mut String,
    definition: &TypeDef,
    manifest: &Manifest,
) -> Result<()> {
    match &definition.shape {
        TypeShape::Struct { fields } => {
            emit_ts_doc(source, definition.docs.as_deref(), "")?;
            writeln!(source, "export interface {} {{", definition.name)?;
            for field in fields {
                emit_ts_doc(source, field.docs.as_deref(), "  ")?;
                writeln!(
                    source,
                    "  readonly {}{}: {};",
                    ts_property(&field.wire_name),
                    if field.required { "" } else { "?" },
                    typescript_ref(&field.ty, manifest)?
                )?;
            }
            source.push_str("}\n");
        }
        TypeShape::StringEnum { variants } => {
            emit_ts_doc(source, definition.docs.as_deref(), "")?;
            writeln!(
                source,
                "export type {} = {};",
                definition.name,
                variants
                    .iter()
                    .map(|variant| ts_string(&variant.wire_name))
                    .collect::<Vec<_>>()
                    .join(" | ")
            )?;
        }
        TypeShape::TaggedEnum { tag, variants } => {
            for variant in variants {
                let name = tagged_variant_name(&definition.name, &variant.rust_name);
                emit_ts_doc(source, variant.docs.as_deref(), "")?;
                writeln!(source, "export interface {name} {{")?;
                writeln!(
                    source,
                    "  readonly {}: {};",
                    ts_property(tag),
                    ts_string(&variant.wire_name)
                )?;
                for field in &variant.fields {
                    emit_ts_doc(source, field.docs.as_deref(), "  ")?;
                    writeln!(
                        source,
                        "  readonly {}{}: {};",
                        ts_property(&field.wire_name),
                        if field.required { "" } else { "?" },
                        typescript_ref(&field.ty, manifest)?
                    )?;
                }
                source.push_str("}\n");
            }
            writeln!(
                source,
                "export type {} = {};",
                definition.name,
                variants
                    .iter()
                    .map(|variant| tagged_variant_name(&definition.name, &variant.rust_name))
                    .collect::<Vec<_>>()
                    .join(" | ")
            )?;
        }
        TypeShape::Alias { target } => {
            emit_ts_doc(source, definition.docs.as_deref(), "")?;
            writeln!(
                source,
                "export type {} = {};",
                definition.name,
                typescript_ref(target, manifest)?
            )?;
        }
    }
    Ok(())
}

fn emit_typescript_resource_declaration(
    source: &mut String,
    resource: &ResourceDef,
    manifest: &Manifest,
) -> Result<()> {
    let constructor = resource
        .constructors
        .iter()
        .find(|item| item.rust_name == "new")
        .or_else(|| resource.constructors.first())
        .context("resource has no constructor")?;
    emit_ts_doc(source, resource.docs.as_deref(), "")?;
    writeln!(source, "export class {} {{", resource.name)?;
    writeln!(
        source,
        "  constructor({});",
        typescript_params(&constructor.params, manifest)?
    )?;
    for factory in resource
        .constructors
        .iter()
        .filter(|item| !std::ptr::eq(*item, constructor))
    {
        writeln!(
            source,
            "  static {}({}): {};",
            factory.host_name,
            typescript_params(&factory.params, manifest)?,
            resource.name
        )?;
    }
    for method in &resource.methods {
        writeln!(
            source,
            "  {}({}): {};",
            method.host_name,
            typescript_params(&method.params, manifest)?,
            typescript_ref(&method.returns, manifest)?
        )?;
    }
    source.push_str("  close(): void;\n}\n");
    Ok(())
}

fn typescript_runtime(manifest: &Manifest) -> Result<String> {
    let mut source = String::from(
        "import initializeNative, * as native from \"./native.js\";\n\nconst wasmUrl = new URL(\"./native_bg.wasm\", import.meta.url);\nlet wasmInput = wasmUrl;\nif (wasmUrl.protocol === \"file:\" && globalThis.process?.versions?.node) {\n  const nodeModule = \"node:fs/promises\";\n  const { readFile } = await import(nodeModule);\n  wasmInput = await readFile(wasmUrl);\n}\nawait initializeNative({ module_or_path: wasmInput });\n",
    );
    source.push_str(TYPESCRIPT_ADAPTERS);
    source.push_str("\nconst nativeSchemas = {\n");
    for definition in &manifest.types {
        writeln!(
            source,
            "  {}: {},",
            ts_property(&definition.name),
            typescript_named_spec(definition, manifest)?
        )?;
    }
    source.push_str("};\n");

    for error in &manifest.errors {
        writeln!(
            source,
            "\nexport class {} extends Error {{\n  constructor(code, message) {{\n    super(message);\n    this.name = {};\n    this.code = code;\n  }}\n}}",
            error.name,
            ts_string(&error.name)
        )?;
    }
    for function in &manifest.functions {
        emit_typescript_function(&mut source, function, manifest, None)?;
    }
    for resource in &manifest.resources {
        emit_typescript_resource(&mut source, resource, manifest)?;
    }
    for constant in &manifest.constants {
        writeln!(
            source,
            "\nexport const {} = restoreHost({}, {});",
            constant.host_name,
            typescript_value(&constant.value, &constant.ty, manifest)?,
            typescript_spec(&constant.ty, manifest)?
        )?;
    }
    Ok(source)
}

const TYPESCRIPT_ADAPTERS: &str = r#"
function prepareHost(value) {
  if (value instanceof Date) return value.toISOString();
  if (ArrayBuffer.isView(value)) return value;
  if (Array.isArray(value)) return value.map(prepareHost);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, prepareHost(item)]));
  }
  return value;
}

const bufferConstructors = {
  u8: Uint8Array, i8: Int8Array, u16: Uint16Array, i16: Int16Array,
  u32: Uint32Array, i32: Int32Array, u64: BigUint64Array, i64: BigInt64Array,
  f32: Float32Array, f64: Float64Array,
};

function restoreHost(value, spec) {
  if (value == null || spec == null) return value;
  const [kind, detail, variants] = spec;
  if (kind === "bytes") return new Uint8Array(value);
  if (kind === "buffer") return new bufferConstructors[detail](value);
  if (kind === "list") return Array.from(value, item => restoreHost(item, detail));
  if (kind === "map") return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, detail)]));
  if (kind === "tuple") return value.map((item, index) => restoreHost(item, detail[index]));
  if (kind === "named") return restoreHost(value, nativeSchemas[detail]);
  if (kind === "alias") return restoreHost(value, detail);
  if (kind === "struct") return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, detail[key])]));
  if (kind === "tagged") {
    const fields = variants[value[detail]] ?? {};
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, fields[key])]));
  }
  return value;
}

function nativeError(error, ErrorType) {
  const text = String(error);
  const line = text.indexOf("\n");
  return line < 0 ? error : new ErrorType(text.slice(0, line), text.slice(line + 1));
}
"#;

fn emit_typescript_function(
    source: &mut String,
    function: &FunctionDef,
    manifest: &Manifest,
    receiver: Option<&str>,
) -> Result<()> {
    let params = function
        .params
        .iter()
        .map(|param| param.host_name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let calls = function
        .params
        .iter()
        .map(|param| format!("prepareHost({})", param.host_name))
        .collect::<Vec<_>>()
        .join(", ");
    let native_name = format!("__rspyts_export_{}", function.host_name);
    let (indent, signature, call) = if receiver.is_some() {
        (
            "  ",
            format!("  {}({params}) {{", function.host_name),
            format!("this.nativeResource.{}({calls})", function.host_name),
        )
    } else {
        (
            "",
            format!("export function {}({params}) {{", function.host_name),
            format!(
                "native[{quoted}]({calls})",
                quoted = ts_string(&native_name)
            ),
        )
    };
    writeln!(source, "\n{signature}")?;
    if function.error.is_some() {
        writeln!(source, "{indent}  try {{")?;
        writeln!(source, "{indent}    const result = {call};")?;
        writeln!(
            source,
            "{indent}    return restoreHost(result, {});",
            typescript_spec(&function.returns, manifest)?
        )?;
        writeln!(source, "{indent}  }} catch (error) {{")?;
        writeln!(
            source,
            "{indent}    throw nativeError(error, {});",
            error_name(function.error.as_ref(), manifest)?
        )?;
        writeln!(source, "{indent}  }}")?;
    } else {
        writeln!(source, "{indent}  const result = {call};")?;
        writeln!(
            source,
            "{indent}  return restoreHost(result, {});",
            typescript_spec(&function.returns, manifest)?
        )?;
    }
    writeln!(source, "{indent}}}")?;
    Ok(())
}

fn emit_typescript_resource(
    source: &mut String,
    resource: &ResourceDef,
    manifest: &Manifest,
) -> Result<()> {
    let constructor = resource
        .constructors
        .iter()
        .find(|item| item.rust_name == "new")
        .or_else(|| resource.constructors.first())
        .context("resource has no constructor")?;
    let params = constructor
        .params
        .iter()
        .map(|item| item.host_name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let calls = constructor
        .params
        .iter()
        .map(|item| format!("prepareHost({})", item.host_name))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(source, "\nexport class {} {{", resource.name)?;
    writeln!(source, "  constructor({params}) {{")?;
    let native_call = format!("new native.RspytsWasm{}({calls})", resource.name);
    if constructor.error.is_some() {
        source.push_str("    try {\n");
        writeln!(source, "      this.nativeResource = {native_call};")?;
        source.push_str("    } catch (error) {\n");
        writeln!(
            source,
            "      throw nativeError(error, {});",
            error_name(constructor.error.as_ref(), manifest)?
        )?;
        source.push_str("    }\n");
    } else {
        writeln!(source, "    this.nativeResource = {native_call};")?;
    }
    source.push_str("  }\n");
    for factory in resource
        .constructors
        .iter()
        .filter(|item| !std::ptr::eq(*item, constructor))
    {
        let params = factory
            .params
            .iter()
            .map(|item| item.host_name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let calls = factory
            .params
            .iter()
            .map(|item| format!("prepareHost({})", item.host_name))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(source, "\n  static {}({params}) {{", factory.host_name)?;
        writeln!(
            source,
            "    const value = Object.create({}.prototype);",
            resource.name
        )?;
        let native_call = format!(
            "native.RspytsWasm{}.{}({calls})",
            resource.name, factory.host_name
        );
        if factory.error.is_some() {
            source.push_str("    try {\n");
            writeln!(source, "      value.nativeResource = {native_call};")?;
            source.push_str("    } catch (error) {\n");
            writeln!(
                source,
                "      throw nativeError(error, {});",
                error_name(factory.error.as_ref(), manifest)?
            )?;
            source.push_str("    }\n");
        } else {
            writeln!(source, "    value.nativeResource = {native_call};")?;
        }
        source.push_str("    return value;\n  }\n");
    }
    for method in &resource.methods {
        let function = FunctionDef {
            owner: resource.owner.clone(),
            rust_name: method.rust_name.clone(),
            host_name: method.host_name.clone(),
            docs: method.docs.clone(),
            params: method.params.clone(),
            returns: method.returns.clone(),
            error: method.error.clone(),
        };
        emit_typescript_function(source, &function, manifest, Some(&resource.name))?;
    }
    source.push_str("\n  close() {\n    this.nativeResource.close();\n  }\n}\n");
    Ok(())
}

fn python_ref(reference: &TypeRef, manifest: &Manifest) -> Result<String> {
    Ok(match reference {
        TypeRef::Unit => "None".into(),
        TypeRef::Bool => "bool".into(),
        TypeRef::Int { .. } => "int".into(),
        TypeRef::Float { .. } => "float".into(),
        TypeRef::String => "str".into(),
        TypeRef::DateTime => "datetime".into(),
        TypeRef::Json => "Any".into(),
        TypeRef::Option { item } => format!("{} | None", python_ref(item, manifest)?),
        TypeRef::List { item } => format!("list[{}]", python_ref(item, manifest)?),
        TypeRef::Map { value } => format!("dict[str, {}]", python_ref(value, manifest)?),
        TypeRef::Tuple { items } => format!(
            "tuple[{}]",
            items
                .iter()
                .map(|item| python_ref(item, manifest))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Named { identity } => type_name(identity, manifest)?.to_owned(),
        TypeRef::Bytes | TypeRef::FixedBytes { .. } => "bytes".into(),
        TypeRef::Buffer { element } => python_buffer_name(*element).into(),
    })
}

fn python_adapter_type(reference: &TypeRef, manifest: &Manifest) -> Result<String> {
    if matches!(reference, TypeRef::Unit) {
        Ok("type(None)".into())
    } else {
        python_ref(reference, manifest)
    }
}

fn python_param(param: &ParamDef, manifest: &Manifest) -> Result<String> {
    Ok(format!(
        "{}: {}",
        safe_python_name(&param.rust_name),
        python_ref(&param.ty, manifest)?
    ))
}

fn python_spec(reference: &TypeRef, manifest: &Manifest) -> Result<String> {
    Ok(match reference {
        TypeRef::Option { item } => python_spec(item, manifest)?,
        TypeRef::List { item } => format!("(\"list\", {})", python_spec(item, manifest)?),
        TypeRef::Map { value } => format!("(\"map\", {})", python_spec(value, manifest)?),
        TypeRef::Tuple { items } => format!(
            "(\"tuple\", ({}))",
            items
                .iter()
                .map(|item| python_spec(item, manifest))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Named { identity } => {
            format!("(\"named\", {})", py_string(type_name(identity, manifest)?))
        }
        TypeRef::Bytes | TypeRef::FixedBytes { .. } => "(\"bytes\",)".into(),
        TypeRef::Buffer { element } => {
            format!("(\"buffer\", {})", py_string(python_numpy_scalar(*element)))
        }
        _ => "None".into(),
    })
}

fn python_named_spec(definition: &TypeDef, manifest: &Manifest) -> Result<String> {
    Ok(match &definition.shape {
        TypeShape::Struct { fields } => format!(
            "(\"struct\", {{{}}})",
            fields
                .iter()
                .map(|field| Ok(format!(
                    "{}: {}",
                    py_string(&field.wire_name),
                    python_spec(&field.ty, manifest)?
                )))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeShape::TaggedEnum { tag, variants } => format!(
            "(\"tagged\", {}, {{{}}})",
            py_string(tag),
            variants
                .iter()
                .map(|variant| Ok(format!(
                    "{}: {{{}}}",
                    py_string(&variant.wire_name),
                    variant
                        .fields
                        .iter()
                        .map(|field| Ok(format!(
                            "{}: {}",
                            py_string(&field.wire_name),
                            python_spec(&field.ty, manifest)?
                        )))
                        .collect::<Result<Vec<_>>>()?
                        .join(", ")
                )))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeShape::Alias { target } => {
            format!("(\"alias\", {})", python_spec(target, manifest)?)
        }
        TypeShape::StringEnum { .. } => "None".into(),
    })
}

fn python_model_names(manifest: &Manifest) -> Vec<String> {
    let mut names = Vec::new();
    for definition in &manifest.types {
        names.push(definition.name.clone());
        if let TypeShape::TaggedEnum { variants, .. } = &definition.shape {
            names.extend(
                variants
                    .iter()
                    .map(|variant| tagged_variant_name(&definition.name, &variant.rust_name)),
            );
        }
    }
    names.extend(
        buffer_elements(manifest)
            .into_iter()
            .map(|element| python_buffer_name(element).to_owned()),
    );
    names.sort();
    names.dedup();
    names
}

fn python_scalar(value: &ScalarValue) -> String {
    match value {
        ScalarValue::Bool(value) => if *value { "True" } else { "False" }.into(),
        ScalarValue::I64(value) => value.to_string(),
        ScalarValue::String(value) => py_string(value),
    }
}

fn python_json(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(value) => if *value { "True" } else { "False" }.into(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => py_string(value),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(python_json)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{}: {}", py_string(key), python_json(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn is_plain_python_constant(reference: &TypeRef) -> bool {
    matches!(
        reference,
        TypeRef::Unit
            | TypeRef::Bool
            | TypeRef::Int { .. }
            | TypeRef::Float { .. }
            | TypeRef::String
            | TypeRef::Json
    )
}

fn emit_python_doc(source: &mut String, docs: Option<&str>, indent: &str) -> Result<()> {
    if let Some(docs) = docs {
        writeln!(source, "{indent}{}", py_string(docs))?;
    }
    Ok(())
}

fn safe_python_name(value: &str) -> String {
    if matches!(
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
    ) {
        format!("{value}_value")
    } else {
        value.to_owned()
    }
}

fn py_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings serialize")
}

fn tagged_variant_name(type_name: &str, variant_name: &str) -> String {
    format!("{type_name}{variant_name}")
}

fn python_buffer_name(element: BufferElement) -> &'static str {
    match element {
        BufferElement::U8 => "UInt8Buffer",
        BufferElement::I8 => "Int8Buffer",
        BufferElement::U16 => "UInt16Buffer",
        BufferElement::I16 => "Int16Buffer",
        BufferElement::U32 => "UInt32Buffer",
        BufferElement::I32 => "Int32Buffer",
        BufferElement::U64 => "UInt64Buffer",
        BufferElement::I64 => "Int64Buffer",
        BufferElement::F32 => "Float32Buffer",
        BufferElement::F64 => "Float64Buffer",
    }
}

fn python_numpy_scalar(element: BufferElement) -> &'static str {
    match element {
        BufferElement::U8 => "uint8",
        BufferElement::I8 => "int8",
        BufferElement::U16 => "uint16",
        BufferElement::I16 => "int16",
        BufferElement::U32 => "uint32",
        BufferElement::I32 => "int32",
        BufferElement::U64 => "uint64",
        BufferElement::I64 => "int64",
        BufferElement::F32 => "float32",
        BufferElement::F64 => "float64",
    }
}

fn typescript_ref(reference: &TypeRef, manifest: &Manifest) -> Result<String> {
    Ok(match reference {
        TypeRef::Unit => "void".into(),
        TypeRef::Bool => "boolean".into(),
        TypeRef::Int { bits: 64, .. } => "bigint".into(),
        TypeRef::Int { .. } | TypeRef::Float { .. } => "number".into(),
        TypeRef::String | TypeRef::DateTime => "string".into(),
        TypeRef::Json => "JsonValue".into(),
        TypeRef::Option { item } => format!("{} | null", typescript_ref(item, manifest)?),
        TypeRef::List { item } => format!("readonly {}[]", typescript_ref(item, manifest)?),
        TypeRef::Map { value } => {
            format!(
                "Readonly<Record<string, {}>>",
                typescript_ref(value, manifest)?
            )
        }
        TypeRef::Tuple { items } => format!(
            "readonly [{}]",
            items
                .iter()
                .map(|item| typescript_ref(item, manifest))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Named { identity } => type_name(identity, manifest)?.into(),
        TypeRef::Bytes | TypeRef::FixedBytes { .. } => "Uint8Array".into(),
        TypeRef::Buffer { element } => typescript_buffer_name(*element).into(),
    })
}

fn typescript_params(params: &[ParamDef], manifest: &Manifest) -> Result<String> {
    params
        .iter()
        .map(|param| {
            Ok(format!(
                "{}: {}",
                param.host_name,
                typescript_ref(&param.ty, manifest)?
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(|items| items.join(", "))
}

fn typescript_spec(reference: &TypeRef, manifest: &Manifest) -> Result<String> {
    Ok(match reference {
        TypeRef::Option { item } => typescript_spec(item, manifest)?,
        TypeRef::List { item } => format!("[\"list\", {}]", typescript_spec(item, manifest)?),
        TypeRef::Map { value } => format!("[\"map\", {}]", typescript_spec(value, manifest)?),
        TypeRef::Tuple { items } => format!(
            "[\"tuple\", [{}]]",
            items
                .iter()
                .map(|item| typescript_spec(item, manifest))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Named { identity } => {
            format!("[\"named\", {}]", ts_string(type_name(identity, manifest)?))
        }
        TypeRef::Bytes | TypeRef::FixedBytes { .. } => "[\"bytes\"]".into(),
        TypeRef::Buffer { element } => {
            format!("[\"buffer\", {}]", ts_string(buffer_key(*element)))
        }
        _ => "null".into(),
    })
}

fn typescript_named_spec(definition: &TypeDef, manifest: &Manifest) -> Result<String> {
    Ok(match &definition.shape {
        TypeShape::Struct { fields } => format!(
            "[\"struct\", {{{}}}]",
            fields
                .iter()
                .map(|field| Ok(format!(
                    "{}: {}",
                    ts_property(&field.wire_name),
                    typescript_spec(&field.ty, manifest)?
                )))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeShape::TaggedEnum { tag, variants } => format!(
            "[\"tagged\", {}, {{{}}}]",
            ts_string(tag),
            variants
                .iter()
                .map(|variant| Ok(format!(
                    "{}: {{{}}}",
                    ts_property(&variant.wire_name),
                    variant
                        .fields
                        .iter()
                        .map(|field| Ok(format!(
                            "{}: {}",
                            ts_property(&field.wire_name),
                            typescript_spec(&field.ty, manifest)?
                        )))
                        .collect::<Result<Vec<_>>>()?
                        .join(", ")
                )))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeShape::Alias { target } => {
            format!("[\"alias\", {}]", typescript_spec(target, manifest)?)
        }
        TypeShape::StringEnum { .. } => "null".into(),
    })
}

fn typescript_value(value: &Value, reference: &TypeRef, manifest: &Manifest) -> Result<String> {
    if value.is_null() {
        return Ok("null".into());
    }
    Ok(match reference {
        TypeRef::Int { bits: 64, .. } => format!(
            "{}n",
            value
                .as_u64()
                .map(|item| item.to_string())
                .or_else(|| value.as_i64().map(|item| item.to_string()))
                .context("invalid 64-bit constant")?
        ),
        TypeRef::Option { item } => typescript_value(value, item, manifest)?,
        TypeRef::List { item } => format!(
            "[{}]",
            value
                .as_array()
                .context("invalid list constant")?
                .iter()
                .map(|value| typescript_value(value, item, manifest))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Map { value: item } => format!(
            "{{{}}}",
            value
                .as_object()
                .context("invalid map constant")?
                .iter()
                .map(|(key, value)| Ok(format!(
                    "{}: {}",
                    ts_property(key),
                    typescript_value(value, item, manifest)?
                )))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Tuple { items } => format!(
            "[{}]",
            value
                .as_array()
                .context("invalid tuple constant")?
                .iter()
                .zip(items)
                .map(|(value, item)| typescript_value(value, item, manifest))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Named { identity } => {
            let definition = type_definition(identity, manifest)?;
            typescript_named_value(value, definition, manifest)?
        }
        TypeRef::Bytes | TypeRef::FixedBytes { .. } | TypeRef::Buffer { .. } => {
            serde_json::to_string(value)?
        }
        _ => serde_json::to_string(value)?,
    })
}

fn typescript_named_value(
    value: &Value,
    definition: &TypeDef,
    manifest: &Manifest,
) -> Result<String> {
    match &definition.shape {
        TypeShape::Alias { target } => typescript_value(value, target, manifest),
        TypeShape::StringEnum { .. } => Ok(serde_json::to_string(value)?),
        TypeShape::Struct { fields } => typescript_object_value(value, fields, manifest),
        TypeShape::TaggedEnum { tag, variants } => {
            let object = value.as_object().context("invalid tagged enum constant")?;
            let tag_value = object
                .get(tag)
                .and_then(Value::as_str)
                .context("tagged enum constant has no tag")?;
            let variant = variants
                .iter()
                .find(|variant| variant.wire_name == tag_value)
                .context("unknown tagged enum constant variant")?;
            let mut fields = variant.fields.clone();
            fields.push(FieldDef {
                rust_name: tag.clone(),
                wire_name: tag.clone(),
                docs: None,
                ty: TypeRef::String,
                required: true,
                default: None,
                constraints: Default::default(),
            });
            typescript_object_value(value, &fields, manifest)
        }
    }
}

fn typescript_object_value(
    value: &Value,
    fields: &[FieldDef],
    manifest: &Manifest,
) -> Result<String> {
    let object = value.as_object().context("invalid object constant")?;
    Ok(format!(
        "{{{}}}",
        object
            .iter()
            .map(|(key, value)| {
                let field = fields
                    .iter()
                    .find(|field| field.wire_name == *key)
                    .context("constant has an unknown field")?;
                Ok(format!(
                    "{}: {}",
                    ts_property(key),
                    typescript_value(value, &field.ty, manifest)?
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ")
    ))
}

fn emit_ts_doc(source: &mut String, docs: Option<&str>, indent: &str) -> Result<()> {
    if let Some(docs) = docs {
        writeln!(
            source,
            "{indent}/** {} */",
            docs.replace("*/", "* /").replace('\n', " ")
        )?;
    }
    Ok(())
}

fn ts_property(value: &str) -> String {
    if is_identifier(value) {
        value.to_owned()
    } else {
        ts_string(value)
    }
}

fn ts_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings serialize")
}

fn typescript_buffer_name(element: BufferElement) -> &'static str {
    match element {
        BufferElement::U8 => "Uint8Array",
        BufferElement::I8 => "Int8Array",
        BufferElement::U16 => "Uint16Array",
        BufferElement::I16 => "Int16Array",
        BufferElement::U32 => "Uint32Array",
        BufferElement::I32 => "Int32Array",
        BufferElement::U64 => "BigUint64Array",
        BufferElement::I64 => "BigInt64Array",
        BufferElement::F32 => "Float32Array",
        BufferElement::F64 => "Float64Array",
    }
}

fn buffer_key(element: BufferElement) -> &'static str {
    match element {
        BufferElement::U8 => "u8",
        BufferElement::I8 => "i8",
        BufferElement::U16 => "u16",
        BufferElement::I16 => "i16",
        BufferElement::U32 => "u32",
        BufferElement::I32 => "i32",
        BufferElement::U64 => "u64",
        BufferElement::I64 => "i64",
        BufferElement::F32 => "f32",
        BufferElement::F64 => "f64",
    }
}

fn type_name<'a>(identity: &DefinitionId, manifest: &'a Manifest) -> Result<&'a str> {
    Ok(type_definition(identity, manifest)?.name.as_str())
}

fn type_definition<'a>(identity: &DefinitionId, manifest: &'a Manifest) -> Result<&'a TypeDef> {
    manifest
        .types
        .iter()
        .find(|item| item.owner == identity.owner && item.id == identity.id)
        .with_context(|| format!("missing type `{identity}`"))
}

fn error_name<'a>(identity: Option<&DefinitionId>, manifest: &'a Manifest) -> Result<&'a str> {
    let identity = identity.context("missing error identity")?;
    manifest
        .errors
        .iter()
        .find(|item| item.owner == identity.owner && item.id == identity.id)
        .map(|item| item.name.as_str())
        .with_context(|| format!("missing error `{identity}`"))
}

fn buffer_elements(manifest: &Manifest) -> BTreeSet<BufferElement> {
    let mut result = BTreeSet::new();
    for reference in contract_refs(manifest) {
        collect_buffers(reference, &mut result);
    }
    result
}

fn uses_buffer(manifest: &Manifest) -> bool {
    !buffer_elements(manifest).is_empty()
}

fn python_type_adapter(reference: &TypeRef, manifest: &Manifest) -> Result<String> {
    let annotation = python_adapter_type(reference, manifest)?;
    let mut buffers = BTreeSet::new();
    collect_buffers(reference, &mut buffers);
    if buffers.is_empty() {
        Ok(format!("TypeAdapter({annotation})"))
    } else {
        Ok(format!(
            "TypeAdapter({annotation}, config=ConfigDict(arbitrary_types_allowed=True))"
        ))
    }
}

fn collect_buffers(reference: &TypeRef, result: &mut BTreeSet<BufferElement>) {
    match reference {
        TypeRef::Buffer { element } => {
            result.insert(*element);
        }
        TypeRef::Option { item } | TypeRef::List { item } => collect_buffers(item, result),
        TypeRef::Map { value } => collect_buffers(value, result),
        TypeRef::Tuple { items } => {
            for item in items {
                collect_buffers(item, result);
            }
        }
        _ => {}
    }
}

fn contract_refs(manifest: &Manifest) -> Vec<&TypeRef> {
    let mut result = Vec::new();
    for definition in &manifest.types {
        match &definition.shape {
            TypeShape::Struct { fields } => result.extend(fields.iter().map(|field| &field.ty)),
            TypeShape::TaggedEnum { variants, .. } => result.extend(
                variants
                    .iter()
                    .flat_map(|variant| variant.fields.iter().map(|field| &field.ty)),
            ),
            TypeShape::Alias { target } => result.push(target),
            TypeShape::StringEnum { .. } => {}
        }
    }
    for function in &manifest.functions {
        result.extend(function.params.iter().map(|param| &param.ty));
        result.push(&function.returns);
    }
    for resource in &manifest.resources {
        for constructor in &resource.constructors {
            result.extend(constructor.params.iter().map(|param| &param.ty));
        }
        for method in &resource.methods {
            result.extend(method.params.iter().map(|param| &param.ty));
            result.push(&method.returns);
        }
    }
    result.extend(manifest.constants.iter().map(|constant| &constant.ty));
    result
}

fn validate_python_package(value: &str) -> Result<()> {
    if value.is_empty() || value.split('.').any(|part| !is_identifier(part)) {
        bail!("Python package `{value}` must contain dot-separated identifiers");
    }
    Ok(())
}

fn validate_typescript_package(value: &str) -> Result<()> {
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

fn is_identifier(value: &str) -> bool {
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

fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

struct ProjectLock(fs::File);

fn project_lock(project: &Project) -> Result<ProjectLock> {
    let path = project.root.join(".rspyts-build.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    FileExt::lock(&file).with_context(|| format!("failed to lock {}", path.display()))?;
    Ok(ProjectLock(file))
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn replace_directory(temporary: &TempDir, output: &Path) -> Result<()> {
    if output.exists() {
        let metadata = fs::symlink_metadata(output)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!(
                "generated output must be a normal directory: {}",
                output.display()
            );
        }
        let backup = output.with_extension("rspyts-old");
        if backup.exists() {
            fs::remove_dir_all(&backup)?;
        }
        fs::rename(output, &backup)?;
        if let Err(error) = fs::rename(temporary.path(), output) {
            fs::rename(&backup, output)?;
            return Err(error).context("failed to publish generated packages");
        }
        fs::remove_dir_all(backup)?;
    } else {
        fs::rename(temporary.path(), output)?;
    }
    Ok(())
}

fn file_tree(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut result = BTreeMap::new();
    collect_files(root, root, &mut result)?;
    Ok(result)
}

fn collect_files(
    root: &Path,
    current: &Path,
    result: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<()> {
    let mut entries = fs::read_dir(current)
        .with_context(|| format!("failed to read {}", current.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(name.as_ref(), "__pycache__" | "build")
            || name.ends_with(".egg-info")
            || path.extension().is_some_and(|value| value == "pyc")
        {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            bail!("generated output contains a symlink: {}", path.display());
        }
        if metadata.is_dir() {
            collect_files(root, &path, result)?;
        } else if metadata.is_file() {
            result.insert(path.strip_prefix(root)?.to_path_buf(), fs::read(path)?);
        }
    }
    Ok(())
}

fn source_state(root: &Path) -> Result<BTreeMap<PathBuf, (u64, Option<SystemTime>)>> {
    let mut result = BTreeMap::new();
    collect_source_state(root, &mut result)?;
    Ok(result)
}

fn collect_source_state(
    current: &Path,
    result: &mut BTreeMap<PathBuf, (u64, Option<SystemTime>)>,
) -> Result<()> {
    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            if matches!(
                entry.file_name().to_str(),
                Some(".git" | "dist" | "node_modules" | "target")
            ) {
                continue;
            }
            collect_source_state(&path, result)?;
        } else if metadata.is_file()
            && (path.extension().is_some_and(|value| value == "rs")
                || path.file_name().is_some_and(|value| {
                    matches!(value.to_str(), Some("Cargo.toml" | "Cargo.lock"))
                }))
        {
            result.insert(path, (metadata.len(), metadata.modified().ok()));
        }
    }
    Ok(())
}

fn write(path: &Path, source: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("generated file collision at {}", path.display()))?;
    file.write_all(source.as_bytes())?;
    Ok(())
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut source = serde_json::to_string_pretty(value)?;
    source.push('\n');
    write(path, &source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_package_names() {
        assert!(validate_python_package("example.client").is_ok());
        assert!(validate_python_package("example-client").is_err());
        assert!(validate_typescript_package("@example/client").is_ok());
        assert!(validate_typescript_package("Example").is_err());
    }

    #[test]
    fn rejects_duplicate_public_names() {
        assert!(unique_public_names("Python", ["Thing", "Other"].into_iter()).is_ok());
        assert!(unique_public_names("Python", ["Thing", "Thing"].into_iter()).is_err());
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
            typescript_ref(
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
        write(&directory.path().join("package.py"), "value = 1\n").unwrap();
        write(&directory.path().join("__pycache__/package.pyc"), "cache").unwrap();
        write(&directory.path().join("build/package.py"), "build").unwrap();
        write(
            &directory.path().join("package.egg-info/PKG-INFO"),
            "metadata",
        )
        .unwrap();

        let files = file_tree(directory.path()).unwrap();
        assert_eq!(files.keys().collect::<Vec<_>>(), [Path::new("package.py")]);
    }

    #[test]
    fn configures_pydantic_for_numpy_results() {
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
        let adapter = python_type_adapter(
            &TypeRef::Buffer {
                element: BufferElement::F64,
            },
            &manifest,
        )
        .unwrap();
        assert!(adapter.contains("arbitrary_types_allowed=True"));
    }
}
