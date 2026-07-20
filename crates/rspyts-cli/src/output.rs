use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use fs4::FileExt;
use serde::Serialize;
use tempfile::TempDir;

use crate::project::Project;

pub(super) struct ProjectLock(fs::File);

pub(super) fn project_lock(project: &Project) -> Result<ProjectLock> {
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

pub(super) fn replace_directory(temporary: &TempDir, output: &Path) -> Result<()> {
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

pub(super) fn file_tree(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
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

pub(super) fn source_state(root: &Path) -> Result<BTreeMap<PathBuf, (u64, Option<SystemTime>)>> {
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

pub(super) fn write(path: &Path, source: &str) -> Result<()> {
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

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut source = serde_json::to_string_pretty(value)?;
    source.push('\n');
    write(path, &source)
}
