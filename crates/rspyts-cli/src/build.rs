use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use fs4::FileExt;
use serde::Serialize;
use tempfile::TempDir;

use crate::BuildTarget;
use crate::config::{Project, TypeScriptMode};
use crate::load::{compile_wasm, load_contract};

const BUILD_LOCK_NAME: &str = ".rspyts.build.lock";
pub(crate) struct ProjectLock {
    _file: fs::File,
}

pub struct PreparedBuild {
    report: BuildReport,
    temporary: TempDir,
    _lock: ProjectLock,
}

#[derive(Debug, Default)]
pub struct BuildOptions {
    pub target: BuildTarget,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildReport {
    pub schema_version: u32,
    pub status: &'static str,
    pub fingerprint: String,
    pub contract: PathBuf,
    pub output: PathBuf,
    pub python: Option<HostArtifact>,
    pub typescript: Option<HostArtifact>,
    #[serde(skip)]
    pub manifest: rspyts::ir::Manifest,
    #[serde(skip)]
    pub dependencies: std::collections::BTreeMap<String, crate::LockedDependency>,
    #[serde(skip)]
    pub hosts: crate::LockedHosts,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostArtifact {
    pub package: String,
    pub path: PathBuf,
    pub mode: &'static str,
}

pub fn build(project: &Project, options: BuildOptions) -> Result<BuildReport> {
    prepare(project, options)?.commit()
}

pub fn prepare(project: &Project, options: BuildOptions) -> Result<PreparedBuild> {
    validate_target(project, options.target)?;
    let project_lock = lock_project(project)?;
    let output = output_path(project)?;

    let loaded = load_contract(project)?;
    crate::validate::manifest(&loaded.manifest)?;
    let resolved = crate::resolve::contract(project, loaded.manifest)?;
    let fingerprint =
        crate::fingerprint(&resolved.manifest, &resolved.hosts, &resolved.dependencies)?;
    let temporary = tempfile::Builder::new()
        .prefix(".rspyts.tmp-")
        .tempdir_in(project.root())
        .context("failed to create temporary generated output")?;
    let temporary_path = temporary.path();

    if options.target.includes_python()
        && let Some(source) = project
            .python
            .as_ref()
            .and_then(|config| config.source.as_ref())
    {
        copy_source(source, &temporary_path.join("python"))?;
    }
    crate::emit::contract(
        temporary_path,
        &resolved.manifest,
        &resolved.dependencies,
        &resolved.hosts,
        &fingerprint,
    )?;
    if options.target.includes_python()
        && let Some(config) = project.python.as_ref()
    {
        crate::emit::python(temporary_path, config, &resolved, &fingerprint)?;
    }
    if options.target.includes_typescript()
        && let Some(config) = project.typescript.as_ref()
    {
        if config.mode == TypeScriptMode::Wasm {
            let wasm = compile_wasm(project)?;
            let wasm_output = temporary_path.join(".wasm-bindgen");
            run_wasm_bindgen(&wasm, &wasm_output)?;
            copy_source(&wasm_output, &temporary_path.join("typescript"))?;
            remove_any(&wasm_output)?;
        }
        crate::emit::typescript(temporary_path, config, &resolved, &fingerprint)?;
    }

    let contract = output.join("contract.json");
    let report = BuildReport {
        schema_version: 1,
        status: "ok",
        fingerprint,
        contract,
        output: output.clone(),
        python: options
            .target
            .includes_python()
            .then_some(project.python.as_ref())
            .flatten()
            .map(|config| HostArtifact {
                package: config.package.clone(),
                path: output.join("python").join(config.package.replace('.', "/")),
                mode: "source",
            }),
        typescript: options
            .target
            .includes_typescript()
            .then_some(project.typescript.as_ref())
            .flatten()
            .map(|config| HostArtifact {
                package: config.package.clone(),
                path: output.join("typescript"),
                mode: match config.mode {
                    TypeScriptMode::Static => "static",
                    TypeScriptMode::Wasm => "wasm",
                },
            }),
        manifest: resolved.manifest,
        dependencies: resolved.dependencies,
        hosts: resolved.hosts,
    };
    Ok(PreparedBuild {
        report,
        temporary,
        _lock: project_lock,
    })
}

impl PreparedBuild {
    pub fn report(&self) -> &BuildReport {
        &self.report
    }

    pub fn commit(self) -> Result<BuildReport> {
        replace_directory(&self.temporary, &self.report.output)?;
        Ok(self.report)
    }
}

pub fn clean(project: &Project) -> Result<PathBuf> {
    let _lock = lock_project(project)?;
    let output = output_path(project)?;
    if validate_output(&output)? {
        fs::remove_dir_all(&output)
            .with_context(|| format!("failed to remove generated output {}", output.display()))?;
    }
    Ok(output)
}

pub(crate) fn output_path(project: &Project) -> Result<PathBuf> {
    let output = project.output_dir();
    validate_source_separation(project, &output)?;
    Ok(output)
}

fn validate_source_separation(project: &Project, output: &Path) -> Result<()> {
    validate_output(output)?;
    let resolved_output = resolve_known_path(output)?;

    let configured_rust_root_path = project
        .cargo_manifest()
        .parent()
        .context("configured Cargo manifest has no parent directory")?;
    reject_directory_overlap(output, configured_rust_root_path, "Rust crate")?;
    let configured_rust_root = resolve_known_path(configured_rust_root_path)?;
    reject_directory_overlap(&resolved_output, &configured_rust_root, "Rust crate")?;
    let cargo_manifest = resolve_known_path(project.cargo_manifest())?;
    let resolved_rust_root = cargo_manifest
        .parent()
        .context("resolved Cargo manifest has no parent directory")?;
    if resolved_rust_root != configured_rust_root {
        reject_directory_overlap(&resolved_output, resolved_rust_root, "resolved Rust crate")?;
    }

    if let Some(source) = project
        .python
        .as_ref()
        .and_then(|config| config.source.as_deref())
    {
        let source = resolve_known_path(source)?;
        reject_directory_overlap(&resolved_output, &source, "Python source")?;
    }

    reject_file_replacement(
        output,
        &resolved_output,
        &project.lock_path(),
        "rspyts lock",
    )?;
    for (alias, dependency) in project.dependencies() {
        reject_file_replacement(
            output,
            &resolved_output,
            &dependency.lock,
            &format!("dependency `{alias}` lock"),
        )?;
    }
    Ok(())
}

fn resolve_known_path(path: &Path) -> Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to resolve authored path {}", path.display())),
    }
}

fn reject_directory_overlap(output: &Path, authored: &Path, label: &str) -> Result<()> {
    if authored.starts_with(output) || output.starts_with(authored) {
        bail!(
            "generated output {} must be separate from authored {label} {}",
            output.display(),
            authored.display()
        );
    }
    Ok(())
}

fn validate_output(output: &Path) -> Result<bool> {
    match fs::symlink_metadata(output) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!(
                "generated output may not be a symlink: {}",
                output.display()
            )
        }
        Ok(metadata) if !metadata.is_dir() => bail!(
            "generated output must be a directory when it already exists: {}",
            output.display()
        ),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error)
            .with_context(|| format!("failed to inspect generated output {}", output.display())),
    }
}

fn reject_file_replacement(
    output: &Path,
    resolved_output: &Path,
    authored: &Path,
    label: &str,
) -> Result<()> {
    let resolved_authored = resolve_known_path(authored)?;
    if authored.starts_with(output) || resolved_authored.starts_with(resolved_output) {
        bail!(
            "generated output {} may not replace authored {label} {}",
            output.display(),
            authored.display(),
        );
    }
    Ok(())
}

fn validate_target(project: &Project, target: BuildTarget) -> Result<()> {
    if target == BuildTarget::Python && project.python.is_none() {
        bail!("--target python requires a [python] configuration");
    }
    if target == BuildTarget::Typescript && project.typescript.is_none() {
        bail!("--target typescript requires a [typescript] configuration");
    }
    Ok(())
}

fn copy_source(source: &Path, destination: &Path) -> Result<()> {
    if destination.starts_with(source) {
        bail!(
            "authored source {} may not contain its generated destination {}",
            source.display(),
            destination.display()
        );
    }
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect source {}", source.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "authored source must be a directory without symlinks: {}",
            source.display()
        );
    }
    fs::create_dir_all(destination)?;
    let mut entries = fs::read_dir(source)
        .with_context(|| format!("failed to read authored source {}", source.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&from)
            .with_context(|| format!("failed to inspect authored source {}", from.display()))?;
        if metadata.file_type().is_symlink() {
            bail!("authored source may not contain symlink {}", from.display());
        }
        if metadata.is_dir() {
            if matches!(
                entry.file_name().to_str(),
                Some("__pycache__" | ".pytest_cache" | ".mypy_cache" | ".ruff_cache")
            ) {
                continue;
            }
            if to.exists() && !to.is_dir() {
                bail!(
                    "authored source directory collides with file {}",
                    to.display()
                );
            }
            copy_source(&from, &to)?;
        } else if metadata.is_file() {
            if entry.file_name() == ".DS_Store"
                || from
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| matches!(extension, "pyc" | "pyo"))
            {
                continue;
            }
            if to.exists() || to.is_symlink() {
                bail!("authored or generated source collision at {}", to.display());
            }
            fs::copy(&from, &to).with_context(|| {
                format!(
                    "failed to copy authored source {} to {}",
                    from.display(),
                    to.display()
                )
            })?;
        } else {
            bail!(
                "authored source contains unsupported file {}",
                from.display()
            );
        }
    }
    Ok(())
}

fn run_wasm_bindgen(wasm: &Path, output: &Path) -> Result<()> {
    fs::create_dir_all(output)?;
    let result = Command::new("wasm-bindgen")
        .arg(wasm)
        .arg("--target")
        .arg("web")
        .arg("--out-dir")
        .arg(output)
        .arg("--out-name")
        .arg("native")
        .output()
        .with_context(|| {
            "TypeScript mode is `wasm`, but wasm-bindgen is not installed on PATH; install the matching wasm-bindgen-cli"
        })?;
    if !result.status.success() {
        bail!(
            "wasm-bindgen failed for {}\n{}{}",
            wasm.display(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    sanitize_wasm_bindgen_dispose(output)?;
    Ok(())
}

fn sanitize_wasm_bindgen_dispose(output: &Path) -> Result<()> {
    for file in ["native.js", "native.d.ts"] {
        let path = output.join(file);
        let source = fs::read_to_string(&path)
            .with_context(|| format!("failed to read wasm-bindgen output {}", path.display()))?;
        let mut sanitized = source
            .lines()
            .filter(|line| !wasm_bindgen_dispose_line(file, line.trim()))
            .collect::<Vec<_>>()
            .join("\n");
        if source.ends_with('\n') {
            sanitized.push('\n');
        }
        if sanitized != source {
            fs::write(&path, sanitized).with_context(|| {
                format!("failed to sanitize wasm-bindgen output {}", path.display())
            })?;
        }
    }
    Ok(())
}

fn wasm_bindgen_dispose_line(file: &str, line: &str) -> bool {
    match file {
        "native.d.ts" => line == "[Symbol.dispose](): void;",
        "native.js" => {
            line.starts_with("if (Symbol.dispose) ")
                && line.contains(".prototype[Symbol.dispose] = ")
                && line.ends_with(".prototype.free;")
        }
        _ => false,
    }
}

pub(crate) fn lock_project(project: &Project) -> Result<ProjectLock> {
    let lock_path = project.root().join(BUILD_LOCK_NAME);
    let file = open_build_lock(&lock_path)?;
    FileExt::lock(&file).with_context(|| format!("failed to lock {}", lock_path.display()))?;
    Ok(ProjectLock { _file: file })
}

fn open_build_lock(path: &Path) -> Result<fs::File> {
    loop {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                bail!(
                    "build lock must be a regular file without symlinks: {}",
                    path.display()
                )
            }
            Ok(_) => {
                return OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .with_context(|| format!("failed to open build lock {}", path.display()));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                match OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(path)
                {
                    Ok(file) => return Ok(file),
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("failed to create build lock {}", path.display())
                        });
                    }
                }
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect build lock {}", path.display()));
            }
        }
    }
}

fn replace_directory(temporary: &TempDir, output: &Path) -> Result<()> {
    if !validate_output(output)? {
        return fs::rename(temporary.path(), output)
            .with_context(|| format!("failed to publish generated output {}", output.display()));
    }

    let parent = output
        .parent()
        .with_context(|| format!("generated output has no parent: {}", output.display()))?;
    let backup = tempfile::Builder::new()
        .prefix(".rspyts.old-")
        .tempdir_in(parent)
        .context("failed to create generated output backup")?;
    let previous = backup.path().join("previous");
    fs::rename(output, &previous)
        .with_context(|| format!("failed to back up generated output {}", output.display()))?;

    if let Err(error) = fs::rename(temporary.path(), output) {
        if let Err(restore_error) = fs::rename(&previous, output) {
            let preserved = backup.keep().join("previous");
            bail!(
                "failed to publish {}: {error}; restoring the previous output also failed: {restore_error}; previous output remains at {}",
                output.display(),
                preserved.display()
            );
        }
        return Err(error).with_context(|| format!("failed to publish {}", output.display()));
    }

    let backup_path = backup.path().to_path_buf();
    if let Err(error) = backup.close() {
        eprintln!(
            "warning: generated output was published, but its backup remains at {}: {error}",
            backup_path.display()
        );
    }
    Ok(())
}

fn remove_any(path: &Path) -> Result<()> {
    if path.is_dir() && !path.is_symlink() {
        fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))
    } else if path.exists() || path.is_symlink() {
        fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn fixture_project(name: &str) -> (PathBuf, Project) {
        let root = std::env::temp_dir().join(format!(
            "rspyts-{name}-{}-{}",
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
        (root, project)
    }

    #[test]
    fn project_lock_serializes_cooperating_transactions() {
        let (root, project) = fixture_project("transaction-lock");
        let first = lock_project(&project).unwrap();
        let config = root.join("rspyts.toml");
        let (sender, receiver) = mpsc::sync_channel(1);
        let waiter = thread::spawn(move || {
            let project = Project::read(&config).unwrap();
            let project_lock = lock_project(&project).unwrap();
            sender.send(()).unwrap();
            drop(project_lock);
        });

        assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
        drop(first);
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        waiter.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wasm_bindgen_dispose_sanitizer_removes_only_generated_hooks() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-wasm-dispose-sanitizer-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("native.js"),
            "export class Counter {}\nif (Symbol.dispose) Counter.prototype[Symbol.dispose] = Counter.prototype.free;\nconst retained = Symbol.dispose;\n",
        )
        .unwrap();
        fs::write(
            root.join("native.d.ts"),
            "export class Counter {\n  free(): void;\n  [Symbol.dispose](): void;\n}\n",
        )
        .unwrap();

        sanitize_wasm_bindgen_dispose(&root).unwrap();

        assert_eq!(
            fs::read_to_string(root.join("native.js")).unwrap(),
            "export class Counter {}\nconst retained = Symbol.dispose;\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("native.d.ts")).unwrap(),
            "export class Counter {\n  free(): void;\n}\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hidden_output_uses_single_dot_siblings_and_replaces_as_one_directory() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-atomic-{}-{}",
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
        let output = root.join(".rspyts");
        fs::create_dir_all(&output).unwrap();
        fs::write(output.join("old"), "old").unwrap();
        let temporary = tempfile::Builder::new()
            .prefix(".rspyts.tmp-")
            .tempdir_in(&root)
            .unwrap();
        assert!(
            temporary
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".rspyts.tmp-")
        );
        assert!(
            !temporary
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("..")
        );
        let backup =
            crate::atomic_sibling(&output, &format!("old-{}", std::process::id())).unwrap();
        assert!(
            backup
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".rspyts.old-")
        );
        assert!(
            !backup
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("..")
        );
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("authored"), "keep").unwrap();
        fs::write(temporary.path().join("new"), "new").unwrap();
        replace_directory(&temporary, &output).unwrap();
        assert!(!output.join("old").exists());
        assert_eq!(fs::read_to_string(output.join("new")).unwrap(), "new");
        assert!(!temporary.path().exists());
        assert_eq!(fs::read_to_string(backup.join("authored")).unwrap(), "keep");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn output_validation_allows_the_fixed_generated_directory_without_creating_it() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-output-validation-{}-{}",
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
        let output = output_path(&project).unwrap();
        assert!(!output.exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn output_validation_rejects_a_rust_crate_containing_generated_output() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-root-crate-output-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
        fs::write(
            root.join("rspyts.toml"),
            "[crate]\npath = \".\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
        )
        .unwrap();
        let project = Project::read(&root.join("rspyts.toml")).unwrap();

        let error = output_path(&project).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("separate from authored Rust crate")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn output_validation_rejects_python_source_containing_generated_output() {
        let (root, _) = fixture_project("source-containing-output");
        fs::write(
            root.join("rspyts.toml"),
            "[crate]\npath = \"rust\"\n\n[python]\npackage = \"fixture\"\nsource = \".\"\n",
        )
        .unwrap();
        let error = Project::read(&root.join("rspyts.toml")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("may not be the rspyts project root")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn authored_source_merges_directories_without_overwriting_files() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-source-merge-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source");
        let destination = root.join("generated");
        fs::create_dir_all(source.join("fixture")).unwrap();
        fs::create_dir_all(destination.join("fixture")).unwrap();
        fs::write(source.join("fixture/authored.py"), "AUTHORED = True\n").unwrap();
        fs::write(destination.join("fixture/generated.py"), "generated\n").unwrap();

        copy_source(&source, &destination).unwrap();
        assert_eq!(
            fs::read_to_string(destination.join("fixture/authored.py")).unwrap(),
            "AUTHORED = True\n"
        );
        assert!(destination.join("fixture/generated.py").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn authored_source_collision_is_a_hard_error() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-source-collision-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source");
        let destination = root.join("generated");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("same.py"), "authored\n").unwrap();
        fs::write(destination.join("same.py"), "generated\n").unwrap();

        let error = copy_source(&source, &destination).unwrap_err();
        assert!(error.to_string().contains("collision"));
        assert_eq!(
            fs::read_to_string(destination.join("same.py")).unwrap(),
            "generated\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn authored_source_omits_python_and_operating_system_cache_files() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-source-cache-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source");
        let destination = root.join("generated");
        fs::create_dir_all(source.join("package/__pycache__")).unwrap();
        fs::create_dir_all(source.join(".pytest_cache")).unwrap();
        fs::write(source.join("package/module.py"), "VALUE = True\n").unwrap();
        fs::write(source.join("package/module.pyc"), "cache").unwrap();
        fs::write(source.join("package/__pycache__/module.pyc"), "cache").unwrap();
        fs::write(source.join(".DS_Store"), "metadata").unwrap();

        copy_source(&source, &destination).unwrap();
        assert!(destination.join("package/module.py").is_file());
        assert!(!destination.join("package/module.pyc").exists());
        assert!(!destination.join("package/__pycache__").exists());
        assert!(!destination.join(".pytest_cache").exists());
        assert!(!destination.join(".DS_Store").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn authored_source_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "rspyts-source-symlink-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(root.join("outside.py"), "outside\n").unwrap();
        symlink(root.join("outside.py"), source.join("linked.py")).unwrap();

        let error = copy_source(&source, &root.join("generated")).unwrap_err();
        assert!(error.to_string().contains("symlink"));
        fs::remove_dir_all(root).unwrap();
    }
}
