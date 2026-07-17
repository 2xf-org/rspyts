mod python;
mod typescript;
mod util;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::LockedDependency;
use crate::config::{PythonConfig, TypeScriptConfig};
use crate::resolve::ResolvedContract;
use std::collections::BTreeMap;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContractFile<'a> {
    schema_version: u32,
    fingerprint: &'a str,
    manifest: &'a rspyts::ir::Manifest,
    dependencies: &'a BTreeMap<String, LockedDependency>,
    hosts: &'a crate::LockedHosts,
}

pub fn contract(
    root: &Path,
    manifest: &rspyts::ir::Manifest,
    dependencies: &BTreeMap<String, LockedDependency>,
    hosts: &crate::LockedHosts,
    fingerprint: &str,
) -> Result<()> {
    write_json(
        &root.join("contract.json"),
        &ContractFile {
            schema_version: crate::LOCK_VERSION,
            fingerprint,
            manifest,
            dependencies,
            hosts,
        },
    )
}

pub fn python(
    root: &Path,
    config: &PythonConfig,
    contract: &ResolvedContract,
    fingerprint: &str,
    native_library: Option<&Path>,
) -> Result<()> {
    python::emit(root, config, contract, fingerprint, native_library)
}

pub fn typescript(
    root: &Path,
    config: &TypeScriptConfig,
    contract: &ResolvedContract,
    fingerprint: &str,
) -> Result<()> {
    typescript::emit(root, config, contract, fingerprint)
}

pub(crate) fn write(path: &Path, source: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| {
            format!(
                "generated file {} collides with authored source",
                path.display()
            )
        })?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))
}

pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut source = serde_json::to_vec(value)?;
    source.push(b'\n');
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| {
            format!(
                "generated file {} collides with authored source",
                path.display()
            )
        })?;
    file.write_all(&source)
        .with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn generated_files_never_overwrite_authored_source() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-generated-collision-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("module.py");
        fs::write(&path, "# authored\n").unwrap();

        let error = write(&path, "# generated\n").unwrap_err();
        assert!(error.to_string().contains("collides with authored source"));
        assert_eq!(fs::read_to_string(path).unwrap(), "# authored\n");
        fs::remove_dir_all(root).unwrap();
    }
}
