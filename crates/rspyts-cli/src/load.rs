use std::ffi::{CStr, c_char};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use libloading::{Library, Symbol};
use serde_json::Value;

use crate::config::Project;

type ContractFunction = unsafe extern "C" fn() -> *mut c_char;
type ContractFreeFunction = unsafe extern "C" fn(*mut c_char);

pub struct LoadedContract {
    pub manifest: rspyts::ir::Manifest,
}

pub fn load_contract(project: &Project) -> Result<LoadedContract> {
    // Inventory probing is deliberately backend-neutral. Projects opt exported domain
    // declarations into this native build through [crate].probe-features; generated host
    // wrappers remain behind the fixed `python` and `wasm` artifact features below.
    let probe_features = project
        .probe_features()
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let artifact = compile(project, CompileTarget::Probe, &probe_features, false)?;
    let bytes = unsafe { read_contract(&artifact)? };
    let manifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("{} exported an invalid rspyts contract", artifact.display()))?;
    Ok(LoadedContract { manifest })
}

pub fn compile_python(project: &Project) -> Result<PathBuf> {
    compile(project, CompileTarget::Native, &["python"], true)
}

pub fn compile_wasm(project: &Project) -> Result<PathBuf> {
    compile(project, CompileTarget::Wasm, &["wasm"], true)
}

#[derive(Clone, Copy)]
enum CompileTarget {
    Probe,
    Native,
    Wasm,
}

fn compile(
    project: &Project,
    target: CompileTarget,
    backend_features: &[&str],
    include_common_features: bool,
) -> Result<PathBuf> {
    let package_name = package_name(project.cargo_manifest())?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(project.cargo_manifest())
        .arg("--package")
        .arg(&package_name)
        .arg("--lib")
        .arg("--release")
        .arg("--message-format=json-render-diagnostics");
    append_feature_args(
        &mut command,
        project,
        backend_features,
        include_common_features,
    );
    if matches!(target, CompileTarget::Wasm) {
        command.arg("--target").arg("wasm32-unknown-unknown");
    } else if matches!(target, CompileTarget::Native) {
        command.env("PYO3_BUILD_EXTENSION_MODULE", "1");
        if cfg!(target_os = "macos") {
            let mut rustflags = std::env::var("RUSTFLAGS").unwrap_or_default();
            if !rustflags.is_empty() {
                rustflags.push(' ');
            }
            rustflags.push_str("-C link-arg=-undefined -C link-arg=dynamic_lookup");
            command.env("RUSTFLAGS", rustflags);
        }
    }
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

    artifact_from_messages(&messages, &package_name, target).with_context(|| {
        format!(
            "Cargo did not report a cdylib artifact for package `{package_name}`; ensure it is a library crate"
        )
    })
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
        .collect::<std::collections::BTreeSet<_>>();
    if !features.is_empty() {
        command
            .arg("--features")
            .arg(features.into_iter().collect::<Vec<_>>().join(","));
    }
}

fn package_name(manifest: &Path) -> Result<String> {
    let source = fs::read_to_string(manifest)
        .with_context(|| format!("failed to read {}", manifest.display()))?;
    let document: toml::Value = toml::from_str(&source)
        .with_context(|| format!("invalid Cargo manifest {}", manifest.display()))?;
    document
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("{} has no [package].name", manifest.display()))
}

fn artifact_from_messages(
    messages: &str,
    package_name: &str,
    target: CompileTarget,
) -> Option<PathBuf> {
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|message| {
            if message.get("reason")?.as_str()? != "compiler-artifact" {
                return None;
            }
            let target_name = message.get("target")?.get("name")?.as_str()?;
            if target_name != package_name && target_name != package_name.replace('-', "_") {
                return None;
            }
            message
                .get("filenames")?
                .as_array()?
                .iter()
                .find_map(|filename| {
                    let path = PathBuf::from(filename.as_str()?);
                    match target {
                        CompileTarget::Probe | CompileTarget::Native
                            if is_dynamic_library(&path) =>
                        {
                            Some(path)
                        }
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

unsafe fn read_contract(library_path: &Path) -> Result<Vec<u8>> {
    let library = unsafe { Library::new(library_path) }
        .with_context(|| format!("failed to load {}", library_path.display()))?;
    let contract: Symbol<'_, ContractFunction> = unsafe { library.get(b"rspyts_contract\0") }
        .context("missing exported symbol rspyts_contract")?;
    let free: Symbol<'_, ContractFreeFunction> = unsafe { library.get(b"rspyts_contract_free\0") }
        .context("missing exported symbol rspyts_contract_free")?;
    let pointer = unsafe { contract() };
    if pointer.is_null() {
        bail!("rspyts_contract returned a null pointer");
    }
    let bytes = unsafe { CStr::from_ptr(pointer) }.to_bytes().to_vec();
    unsafe { free(pointer) };
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn fixture_project(config: &str) -> (PathBuf, Project) {
        let root = std::env::temp_dir().join(format!(
            "rspyts-load-config-{}-{}",
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
        fs::write(root.join("rspyts.toml"), config).unwrap();
        let project = Project::read(&root.join("rspyts.toml")).unwrap();
        (root, project)
    }

    #[test]
    fn selects_only_the_requested_package_artifact() {
        let messages = r#"{"reason":"compiler-artifact","package_id":"dependency 1.0.0 (path+file:///dep)","target":{"name":"dependency"},"filenames":["/tmp/libdependency.so"]}
{"reason":"compiler-artifact","package_id":"sample 0.1.0 (path+file:///sample)","target":{"name":"sample"},"filenames":["/tmp/libsample.rlib","/tmp/libsample.so"]}"#;
        assert_eq!(
            artifact_from_messages(messages, "sample", CompileTarget::Native),
            Some(PathBuf::from("/tmp/libsample.so"))
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
        append_feature_args(&mut command, &project, &["python"], true);
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            ["--no-default-features", "--features", "domain,python"]
        );
        fs::remove_dir_all(root).unwrap();

        let (root, project) = fixture_project(
            "[crate]\npath = \"rust\"\ndefault-features = true\n\n[python]\npackage = \"fixture\"\n",
        );
        let mut command = Command::new("cargo");
        append_feature_args(&mut command, &project, &["python"], true);
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, ["--features", "python"]);
        fs::remove_dir_all(root).unwrap();
    }
}
