//! Building the bridged crate and locating its cdylib artifact.
//!
//! The CLI never parses Rust source: it runs `cargo build
//! --message-format=json-render-diagnostics` (diagnostics stay
//! human-readable on stderr, artifact records arrive as JSON on stdout)
//! and picks the `cdylib` artifact belonging to the configured package.

use anyhow::{Context, Result, bail};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `name` and `version` from the crate's `Cargo.toml`.
#[derive(Debug, PartialEq)]
pub struct CrateMeta {
    pub name: String,
    pub version: String,
}

/// Read the package name and version from `{crate_dir}/Cargo.toml`.
///
/// `version.workspace = true` is resolved by walking up parent
/// directories until a `Cargo.toml` with `[workspace.package].version`
/// is found.
pub fn crate_meta(crate_dir: &Path) -> Result<CrateMeta> {
    let manifest_path = crate_dir.join("Cargo.toml");
    let doc = read_toml(&manifest_path)?;
    let package = doc
        .get("package")
        .with_context(|| format!("`{}` has no [package] section", manifest_path.display()))?;
    let name = package
        .get("name")
        .and_then(toml::Value::as_str)
        .with_context(|| format!("`{}` has no package.name", manifest_path.display()))?
        .to_string();
    let version = match package.get("version") {
        Some(toml::Value::String(v)) => v.clone(),
        Some(v) if v.get("workspace").and_then(toml::Value::as_bool) == Some(true) => {
            workspace_version(crate_dir)?
        }
        Some(other) => bail!(
            "`{}` has an unsupported package.version value: {other}",
            manifest_path.display()
        ),
        None => bail!("`{}` has no package.version", manifest_path.display()),
    };
    Ok(CrateMeta { name, version })
}

/// Walk from `start` upwards looking for `[workspace.package].version`.
fn workspace_version(start: &Path) -> Result<String> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let candidate = d.join("Cargo.toml");
        if candidate.is_file() {
            let doc = read_toml(&candidate)?;
            if let Some(version) = doc
                .get("workspace")
                .and_then(|w| w.get("package"))
                .and_then(|p| p.get("version"))
                .and_then(toml::Value::as_str)
            {
                return Ok(version.to_string());
            }
        }
        dir = d.parent();
    }
    bail!(
        "package.version is workspace-inherited but no ancestor of `{}` \
         declares [workspace.package].version",
        start.display()
    )
}

fn read_toml(path: &Path) -> Result<toml::Value> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read `{}`", path.display()))?;
    text.parse::<toml::Value>()
        .with_context(|| format!("cannot parse `{}`", path.display()))
}

/// Run `cargo build -p {name}` in `crate_dir` and return the path of the
/// produced cdylib (`.dylib` / `.so` / `.dll`).
pub fn build_cdylib(crate_dir: &Path, name: &str, release: bool) -> Result<PathBuf> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(crate_dir)
        .args([
            "build",
            "--message-format=json-render-diagnostics",
            "-p",
            name,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if release {
        cmd.arg("--release");
    }
    let mut child = cmd
        .spawn()
        .context("cannot run `cargo build` (is cargo on PATH?)")?;

    // Consume stdout while cargo runs, keeping the last matching artifact:
    // a crate can be rebuilt more than once in one invocation (e.g. for a
    // build script) and the final compiler-artifact record wins.
    let stdout = child.stdout.take().expect("stdout was piped");
    let mut artifact: Option<PathBuf> = None;
    for line in BufReader::new(stdout).lines() {
        let line = line.context("failed reading cargo output")?;
        if let Some(path) = cdylib_from_message(&line, name) {
            artifact = Some(path);
        }
    }
    let status = child.wait().context("failed waiting for cargo")?;
    if !status.success() {
        bail!("`cargo build -p {name}` failed");
    }
    artifact.with_context(|| {
        format!(
            "cargo produced no cdylib artifact for `{name}` — does its Cargo.toml declare \
             `crate-type = [\"cdylib\", \"rlib\"]` under [lib]?"
        )
    })
}

/// Extract a cdylib path from one `--message-format=json` line, if the
/// line is a compiler-artifact record for the package named `name`.
fn cdylib_from_message(line: &str, name: &str) -> Option<PathBuf> {
    let msg: serde_json::Value = serde_json::from_str(line).ok()?;
    if msg.get("reason")?.as_str()? != "compiler-artifact" {
        return None;
    }
    let target = msg.get("target")?;
    let is_cdylib = target
        .get("kind")?
        .as_array()?
        .iter()
        .any(|k| k.as_str() == Some("cdylib"));
    // Target names may use hyphens or underscores depending on cargo
    // version; compare normalized.
    let target_matches = target.get("name")?.as_str().map(normalize) == Some(normalize(name));
    if !is_cdylib || !target_matches {
        return None;
    }
    msg.get("filenames")?
        .as_array()?
        .iter()
        .filter_map(|f| f.as_str())
        .find(|f| {
            let ext = Path::new(f).extension().and_then(|e| e.to_str());
            matches!(ext, Some("dylib" | "so" | "dll"))
        })
        .map(PathBuf::from)
}

fn normalize(name: &str) -> String {
    name.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_message_is_matched() {
        let line = r#"{"reason":"compiler-artifact","package_id":"path+file:///w#demo-crate@0.1.0","target":{"kind":["cdylib","rlib"],"name":"demo-crate"},"filenames":["/t/libdemo_crate.rlib","/t/libdemo_crate.dylib"]}"#;
        assert_eq!(
            cdylib_from_message(line, "demo-crate"),
            Some(PathBuf::from("/t/libdemo_crate.dylib"))
        );
    }

    #[test]
    fn non_cdylib_and_other_packages_are_skipped() {
        let rlib_only = r#"{"reason":"compiler-artifact","target":{"kind":["lib"],"name":"demo-crate"},"filenames":["/t/libdemo_crate.rlib"]}"#;
        assert_eq!(cdylib_from_message(rlib_only, "demo-crate"), None);
        let other = r#"{"reason":"compiler-artifact","target":{"kind":["cdylib"],"name":"decoy"},"filenames":["/t/libdecoy.so"]}"#;
        assert_eq!(cdylib_from_message(other, "demo-crate"), None);
        assert_eq!(cdylib_from_message("not json", "demo-crate"), None);
    }

    #[test]
    fn crate_meta_reads_direct_and_workspace_versions() {
        let root = std::env::temp_dir().join(format!("rspyts-cli-meta-{}", std::process::id()));
        let member = root.join("crates").join("demo");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/demo\"]\n[workspace.package]\nversion = \"9.9.9\"\n",
        )
        .unwrap();
        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion.workspace = true\n",
        )
        .unwrap();
        assert_eq!(
            crate_meta(&member).unwrap(),
            CrateMeta {
                name: "demo".into(),
                version: "9.9.9".into()
            }
        );

        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n",
        )
        .unwrap();
        assert_eq!(crate_meta(&member).unwrap().version, "1.2.3");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
