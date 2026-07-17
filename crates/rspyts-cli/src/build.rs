use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::BuildTarget;
use crate::config::{Project, PythonMode, TypeScriptMode};
use crate::load::{compile_python, compile_wasm, load_contract};

static BUILD_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default)]
pub struct BuildOptions {
    pub staging: Option<PathBuf>,
    pub target: BuildTarget,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildReport {
    pub schema_version: u32,
    pub status: &'static str,
    pub fingerprint: String,
    pub contract: PathBuf,
    pub staging: PathBuf,
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
    validate_target(project, options.target)?;
    let loaded = load_contract(project)?;
    crate::validate::manifest(&loaded.manifest)?;
    let resolved = crate::resolve::contract(project, loaded.manifest)?;
    let fingerprint =
        crate::fingerprint(&resolved.manifest, &resolved.hosts, &resolved.dependencies)?;
    let output = options
        .staging
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                project.root().join(path)
            }
        })
        .unwrap_or_else(|| project.output_dir());
    let output = normalized_output(&output)?;
    validate_source_separation(project, options.target, &output)?;
    let temporary = temporary_sibling(&output)?;
    remove_any(&temporary)?;
    fs::create_dir_all(&temporary)
        .with_context(|| format!("failed to create {}", temporary.display()))?;

    let result = (|| {
        if options.target.includes_python()
            && let Some(source) = project
                .python
                .as_ref()
                .and_then(|config| config.source.as_ref())
        {
            copy_source(source, &temporary.join("python"))?;
        }
        if options.target.includes_typescript()
            && let Some(source) = project
                .typescript
                .as_ref()
                .and_then(|config| config.source.as_ref())
        {
            copy_source(source, &temporary.join("typescript"))?;
        }
        crate::emit::contract(
            &temporary,
            &resolved.manifest,
            &resolved.dependencies,
            &resolved.hosts,
            &fingerprint,
        )?;
        if options.target.includes_python()
            && let Some(config) = project.python.as_ref()
        {
            let python_library = match config.mode {
                PythonMode::Standalone => Some(compile_python(project)?),
                PythonMode::Source => None,
            };
            crate::emit::python(
                &temporary,
                config,
                &resolved,
                &fingerprint,
                python_library.as_deref(),
            )?;
        }
        if options.target.includes_typescript()
            && let Some(config) = project.typescript.as_ref()
        {
            if config.mode == TypeScriptMode::Wasm {
                let wasm = compile_wasm(project)?;
                let wasm_output = temporary.join(".wasm-bindgen");
                run_wasm_bindgen(&wasm, &wasm_output)?;
                copy_source(&wasm_output, &temporary.join("typescript"))?;
                remove_any(&wasm_output)?;
            }
            crate::emit::typescript(&temporary, config, &resolved, &fingerprint)?;
        }
        replace_directory(&temporary, &output)
    })();
    if result.is_err() {
        let _ = remove_any(&temporary);
    }
    result?;

    let contract = output.join("contract.json");
    Ok(BuildReport {
        schema_version: 1,
        status: "ok",
        fingerprint,
        contract,
        staging: output.clone(),
        python: options
            .target
            .includes_python()
            .then_some(project.python.as_ref())
            .flatten()
            .map(|config| HostArtifact {
                package: config.package.clone(),
                path: output.join("python").join(config.package.replace('.', "/")),
                mode: match config.mode {
                    PythonMode::Standalone => "pyo3-abi3",
                    PythonMode::Source => "pyo3-source",
                },
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
    })
}

fn normalized_output(output: &Path) -> Result<PathBuf> {
    let name = output
        .file_name()
        .context("staging output must name a directory, not a filesystem root")?;
    let parent = output
        .parent()
        .with_context(|| format!("output {} has no parent", output.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create output parent {}", parent.display()))?;
    Ok(parent
        .canonicalize()
        .with_context(|| format!("failed to resolve output parent {}", parent.display()))?
        .join(name))
}

fn validate_source_separation(project: &Project, target: BuildTarget, output: &Path) -> Result<()> {
    let sources = [
        target
            .includes_python()
            .then_some(project.python.as_ref())
            .flatten()
            .and_then(|config| config.source.as_deref()),
        target
            .includes_typescript()
            .then_some(project.typescript.as_ref())
            .flatten()
            .and_then(|config| config.source.as_deref()),
    ];
    for source in sources.into_iter().flatten() {
        if output.starts_with(source) || source.starts_with(output) {
            bail!(
                "staging output {} must be separate from authored source {}",
                output.display(),
                source.display()
            );
        }
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
            "authored source {} may not contain its staging destination {}",
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
    let command = std::env::var_os("WASM_BINDGEN").unwrap_or_else(|| "wasm-bindgen".into());
    let result = Command::new(&command)
        .arg(wasm)
        .arg("--target")
        .arg("web")
        .arg("--out-dir")
        .arg(output)
        .arg("--out-name")
        .arg("native")
        .output()
        .with_context(|| {
            "TypeScript mode is `wasm`, but wasm-bindgen is not installed; install wasm-bindgen-cli or set WASM_BINDGEN"
        })?;
    if !result.status.success() {
        bail!(
            "wasm-bindgen failed for {}\n{}{}",
            wasm.display(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    Ok(())
}

fn temporary_sibling(output: &Path) -> Result<PathBuf> {
    let parent = output
        .parent()
        .with_context(|| format!("output {} has no parent", output.display()))?;
    fs::create_dir_all(parent)?;
    let id = BUILD_ID.fetch_add(1, Ordering::Relaxed);
    crate::atomic_sibling(output, &format!("tmp-{}-{id}", std::process::id()))
}

fn replace_directory(temporary: &Path, output: &Path) -> Result<()> {
    let backup = crate::atomic_sibling(output, &format!("old-{}", std::process::id()))?;
    remove_any(&backup)?;
    let had_output = output.exists() || output.is_symlink();
    if had_output {
        fs::rename(output, &backup)
            .with_context(|| format!("failed to stage replacement of {}", output.display()))?;
    }
    if let Err(error) = fs::rename(temporary, output) {
        if had_output {
            let _ = fs::rename(&backup, output);
        }
        return Err(error).with_context(|| format!("failed to replace {}", output.display()));
    }
    if had_output {
        remove_any(&backup)?;
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn hidden_staging_uses_single_dot_siblings_and_replaces_as_one_directory() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-atomic-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let output = root.join(".rspyts");
        fs::create_dir_all(&output).unwrap();
        fs::write(output.join("old"), "old").unwrap();
        let temporary = temporary_sibling(&output).unwrap();
        assert!(
            temporary
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".rspyts.tmp-")
        );
        assert!(
            !temporary
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
        fs::create_dir_all(&temporary).unwrap();
        fs::write(temporary.join("new"), "new").unwrap();
        replace_directory(&temporary, &output).unwrap();
        assert!(!output.join("old").exists());
        assert_eq!(fs::read_to_string(output.join("new")).unwrap(), "new");
        assert!(!temporary.exists());
        assert!(!backup.exists());
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
        let destination = root.join("staging");
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
        let destination = root.join("staging");
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
    fn authored_source_omits_python_and_platform_cache_files() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-source-cache-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source");
        let destination = root.join("staging");
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

        let error = copy_source(&source, &root.join("staging")).unwrap_err();
        assert!(error.to_string().contains("symlink"));
        fs::remove_dir_all(root).unwrap();
    }
}
