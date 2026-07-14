//! Building the bridged crate and locating its cdylib artifact.
//!
//! The CLI never parses Rust source: it runs `cargo build
//! --message-format=json-render-diagnostics` (diagnostics stay
//! human-readable on stderr, artifact records arrive as JSON on stdout)
//! and picks the `cdylib` artifact belonging to the configured package.

use anyhow::{Context, Result, bail, ensure};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `name` and `version` from the crate's `Cargo.toml`.
#[derive(Debug, PartialEq)]
pub struct CrateMeta {
    pub name: String,
    pub version: String,
}

/// Cargo inputs which may affect the compiled manifest or staged artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildOptions {
    pub features: Vec<String>,
    pub no_default_features: bool,
    /// Cargo profile name (`dev`, `release`, or a custom profile).
    pub profile: String,
    /// Additional target triples staged by `rspyts build`.
    pub targets: Vec<String>,
    pub locked: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            features: Vec::new(),
            no_default_features: false,
            profile: "dev".to_string(),
            targets: Vec::new(),
            locked: false,
        }
    }
}

/// Optional CLI values. `Some` always replaces the corresponding config
/// value, including `Some(false)` for paired boolean flags.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildOverrides {
    pub features: Option<Vec<String>>,
    pub no_default_features: Option<bool>,
    pub profile: Option<String>,
    pub targets: Option<Vec<String>>,
    pub locked: Option<bool>,
}

impl BuildOptions {
    pub fn new(
        features: Vec<String>,
        no_default_features: bool,
        profile: String,
        targets: Vec<String>,
        locked: bool,
    ) -> Result<Self> {
        validate_profile(&profile)?;
        for feature in &features {
            validate_feature(feature)?;
        }
        for target in &targets {
            validate_target(target)?;
        }
        let mut seen = BTreeSet::new();
        let targets = targets
            .into_iter()
            .filter(|target| seen.insert(target.clone()))
            .collect();
        Ok(Self {
            features,
            no_default_features,
            profile,
            targets,
            locked,
        })
    }

    pub fn with_overrides(&self, overrides: BuildOverrides) -> Result<Self> {
        Self::new(
            overrides.features.unwrap_or_else(|| self.features.clone()),
            overrides
                .no_default_features
                .unwrap_or(self.no_default_features),
            overrides.profile.unwrap_or_else(|| self.profile.clone()),
            overrides.targets.unwrap_or_else(|| self.targets.clone()),
            overrides.locked.unwrap_or(self.locked),
        )
    }

    /// Directory Cargo uses for the profile beneath a target directory.
    pub fn profile_dir(&self) -> &str {
        if self.profile == "dev" {
            "debug"
        } else {
            &self.profile
        }
    }
}

fn validate_feature(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && !value.chars().any(char::is_whitespace) && !value.contains(','),
        "invalid Cargo feature `{value}`: feature names must be non-empty and contain no whitespace or commas"
    );
    Ok(())
}

fn validate_profile(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')),
        "invalid Cargo profile `{value}`: expected a non-empty name containing letters, digits, `-`, or `_`"
    );
    Ok(())
}

fn validate_target(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')),
        "invalid target `{value}`: expected a Rust target triple containing letters, digits, `.`, `-`, or `_`"
    );
    ensure!(
        !value.starts_with("wasm64-"),
        "unsupported target `{value}`: the TypeScript runtime uses wasm32 pointers; choose `wasm32-unknown-unknown`"
    );
    Ok(())
}

/// Parse Cargo's comma/whitespace-separated CLI feature spelling.
pub fn parse_features(value: &str) -> Result<Vec<String>> {
    let features: Vec<String> = value
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    ensure!(
        !features.is_empty(),
        "--features requires at least one feature"
    );
    for feature in &features {
        validate_feature(feature)?;
    }
    Ok(features)
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

fn read_toml(path: &Path) -> Result<toml::Table> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read `{}`", path.display()))?;
    text.parse::<toml::Table>()
        .with_context(|| format!("cannot parse `{}`", path.display()))
}

/// Pure Cargo argument construction shared by generate, check, and build.
pub fn cargo_args(name: &str, options: &BuildOptions, target: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "build".to_string(),
        "--message-format=json-render-diagnostics".to_string(),
        "-p".to_string(),
        name.to_string(),
    ];
    if !options.features.is_empty() {
        args.push("--features".to_string());
        args.push(options.features.join(","));
    }
    if options.no_default_features {
        args.push("--no-default-features".to_string());
    }
    match options.profile.as_str() {
        "dev" => {}
        "release" => args.push("--release".to_string()),
        profile => {
            args.push("--profile".to_string());
            args.push(profile.to_string());
        }
    }
    if options.locked {
        args.push("--locked".to_string());
    }
    if let Some(target) = target {
        args.push("--target".to_string());
        args.push(target.to_string());
    }
    args
}

/// Build the host cdylib used for manifest loading by generate/check.
pub fn build_host_cdylib(crate_dir: &Path, name: &str, options: &BuildOptions) -> Result<PathBuf> {
    build_artifact(crate_dir, name, options, None)
}

fn build_artifact(
    crate_dir: &Path,
    name: &str,
    options: &BuildOptions,
    target: Option<&str>,
) -> Result<PathBuf> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(crate_dir)
        .args(cargo_args(name, options, target))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
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
        if let Some(path) = artifact_from_message(&line, name, target.is_some_and(is_wasm_target)) {
            artifact = Some(path);
        }
    }
    let status = child.wait().context("failed waiting for cargo")?;
    if !status.success() {
        let target = target.map_or(String::new(), |target| format!(" for target `{target}`"));
        bail!("`cargo build -p {name}`{target} failed");
    }
    artifact.with_context(|| {
        format!(
            "cargo produced no {} artifact for `{name}`{} — does its Cargo.toml declare \
             `crate-type = [\"cdylib\", \"rlib\"]` under [lib]?",
            if target.is_some_and(is_wasm_target) {
                "WASM"
            } else {
                "cdylib"
            },
            target.map_or(String::new(), |target| format!(" for target `{target}`")),
        )
    })
}

/// Extract a cdylib path from one `--message-format=json` line, if the
/// line is a compiler-artifact record for the package named `name`.
fn artifact_from_message(line: &str, name: &str, prefer_wasm: bool) -> Option<PathBuf> {
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
    let filenames = msg.get("filenames")?.as_array()?;
    let selected = if prefer_wasm {
        filenames
            .iter()
            .find(|value| artifact_extension(value) == Some("wasm"))
    } else {
        filenames
            .iter()
            .find(|value| matches!(artifact_extension(value), Some("dylib" | "so" | "dll")))
    }?;
    selected.as_str().map(PathBuf::from)
}

fn artifact_extension(value: &serde_json::Value) -> Option<&str> {
    Path::new(value.as_str()?).extension()?.to_str()
}

fn is_wasm_target(target: &str) -> bool {
    target.starts_with("wasm32-")
}

/// Cargo's configured target directory, obtained from metadata rather than
/// inferred from the workspace layout.
pub fn target_directory(crate_dir: &Path) -> Result<PathBuf> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(crate_dir)
        .args(["metadata", "--format-version=1", "--no-deps"])
        .output()
        .context("cannot run `cargo metadata` (is cargo on PATH?)")?;
    ensure!(output.status.success(), "`cargo metadata` failed");
    parse_target_directory(&output.stdout)
}

fn parse_target_directory(json: &[u8]) -> Result<PathBuf> {
    let value: serde_json::Value =
        serde_json::from_slice(json).context("cannot parse `cargo metadata` output")?;
    let target = value
        .get("target_directory")
        .and_then(serde_json::Value::as_str)
        .context("`cargo metadata` output has no string target_directory")?;
    Ok(PathBuf::from(target))
}

/// Active rustc host triple, obtained from `rustc -vV`.
pub fn rustc_host() -> Result<String> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(rustc)
        .arg("-vV")
        .output()
        .context("cannot run `rustc -vV` (is rustc on PATH?)")?;
    ensure!(output.status.success(), "`rustc -vV` failed");
    parse_rustc_host(&String::from_utf8_lossy(&output.stdout))
}

fn parse_rustc_host(output: &str) -> Result<String> {
    output
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .filter(|host| !host.is_empty())
        .map(str::to_string)
        .context("`rustc -vV` output has no host triple")
}

/// Build the host and every configured target, copying each final artifact
/// to `target/rspyts/<triple>/<profile>/`.
pub fn build_and_stage(
    crate_dir: &Path,
    name: &str,
    options: &BuildOptions,
) -> Result<Vec<PathBuf>> {
    let target_dir = target_directory(crate_dir)?;
    let host = rustc_host()?;
    let mut staged = Vec::new();

    let host_artifact = build_artifact(crate_dir, name, options, None)?;
    staged.push(stage_artifact(
        &host_artifact,
        &target_dir,
        &host,
        options.profile_dir(),
        name,
    )?);

    for target in &options.targets {
        if target == &host {
            continue;
        }
        let artifact = build_artifact(crate_dir, name, options, Some(target))?;
        staged.push(stage_artifact(
            &artifact,
            &target_dir,
            target,
            options.profile_dir(),
            name,
        )?);
    }
    Ok(staged)
}

fn stage_artifact(
    source: &Path,
    target_dir: &Path,
    target: &str,
    profile_dir: &str,
    name: &str,
) -> Result<PathBuf> {
    let filename = if source.extension().and_then(|ext| ext.to_str()) == Some("wasm") {
        format!("{}.wasm", normalize(name))
    } else {
        source
            .file_name()
            .context("compiled artifact path has no filename")?
            .to_string_lossy()
            .into_owned()
    };
    let destination = target_dir
        .join("rspyts")
        .join(target)
        .join(profile_dir)
        .join(filename);
    std::fs::create_dir_all(destination.parent().expect("destination has a parent")).with_context(
        || {
            format!(
                "cannot create staging directory for `{}`",
                destination.display()
            )
        },
    )?;
    std::fs::copy(source, &destination).with_context(|| {
        format!(
            "cannot stage `{}` as `{}`",
            source.display(),
            destination.display()
        )
    })?;
    Ok(destination)
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
            artifact_from_message(line, "demo-crate", false),
            Some(PathBuf::from("/t/libdemo_crate.dylib"))
        );

        let wasm = r#"{"reason":"compiler-artifact","target":{"kind":["cdylib"],"name":"demo_crate"},"filenames":["/t/demo_crate.rlib","/t/demo_crate.wasm"]}"#;
        assert_eq!(
            artifact_from_message(wasm, "demo-crate", true),
            Some(PathBuf::from("/t/demo_crate.wasm"))
        );
    }

    #[test]
    fn non_cdylib_and_other_packages_are_skipped() {
        let rlib_only = r#"{"reason":"compiler-artifact","target":{"kind":["lib"],"name":"demo-crate"},"filenames":["/t/libdemo_crate.rlib"]}"#;
        assert_eq!(artifact_from_message(rlib_only, "demo-crate", false), None);
        let other = r#"{"reason":"compiler-artifact","target":{"kind":["cdylib"],"name":"decoy"},"filenames":["/t/libdecoy.so"]}"#;
        assert_eq!(artifact_from_message(other, "demo-crate", false), None);
        assert_eq!(artifact_from_message("not json", "demo-crate", false), None);
    }

    #[test]
    fn cargo_arguments_are_deterministic_for_every_option() {
        let options = BuildOptions::new(
            vec!["serde".into(), "fast".into()],
            true,
            "release".into(),
            vec![],
            true,
        )
        .unwrap();
        assert_eq!(
            cargo_args("demo", &options, Some("wasm32-unknown-unknown")),
            [
                "build",
                "--message-format=json-render-diagnostics",
                "-p",
                "demo",
                "--features",
                "serde,fast",
                "--no-default-features",
                "--release",
                "--locked",
                "--target",
                "wasm32-unknown-unknown",
            ]
        );

        let custom = BuildOptions::new(vec![], false, "bench-local".into(), vec![], false).unwrap();
        assert!(
            cargo_args("demo", &custom, None)
                .ends_with(&["--profile".to_string(), "bench-local".to_string()])
        );
    }

    #[test]
    fn cli_overrides_replace_config_including_false_values() {
        let configured = BuildOptions::new(
            vec!["a".into()],
            true,
            "release".into(),
            vec!["wasm32-unknown-unknown".into()],
            true,
        )
        .unwrap();
        let resolved = configured
            .with_overrides(BuildOverrides {
                features: Some(vec!["b".into()]),
                no_default_features: Some(false),
                profile: Some("dev".into()),
                targets: Some(vec!["x86_64-unknown-linux-gnu".into()]),
                locked: Some(false),
            })
            .unwrap();
        assert_eq!(
            resolved,
            BuildOptions::new(
                vec!["b".into()],
                false,
                "dev".into(),
                vec!["x86_64-unknown-linux-gnu".into()],
                false,
            )
            .unwrap()
        );
    }

    #[test]
    fn feature_parser_and_validation_are_strict() {
        assert_eq!(parse_features("a,b c").unwrap(), ["a", "b", "c"]);
        assert!(parse_features(" , ").is_err());
        assert!(BuildOptions::new(vec![], false, "bad profile".into(), vec![], false).is_err());
        assert!(
            BuildOptions::new(
                vec![],
                false,
                "dev".into(),
                vec!["bad target".into()],
                false
            )
            .is_err()
        );
    }

    #[test]
    fn metadata_and_rustc_discovery_parsers_are_strict() {
        assert_eq!(
            parse_target_directory(br#"{"target_directory":"/workspace/target"}"#).unwrap(),
            PathBuf::from("/workspace/target")
        );
        assert!(parse_target_directory(br#"{}"#).is_err());
        assert_eq!(
            parse_rustc_host("rustc 1.85.0\nhost: aarch64-apple-darwin\n").unwrap(),
            "aarch64-apple-darwin"
        );
        assert!(parse_rustc_host("rustc 1.85.0\n").is_err());
    }

    #[test]
    fn staging_is_profile_and_target_separated() {
        let root = std::env::temp_dir().join(format!("rspyts-cli-stage-{}", std::process::id()));
        let source_dir = root.join("source");
        std::fs::create_dir_all(&source_dir).unwrap();
        let wasm = source_dir.join("hashed-name.wasm");
        std::fs::write(&wasm, b"wasm").unwrap();
        let first = stage_artifact(
            &wasm,
            &root,
            "wasm32-unknown-unknown",
            "debug",
            "demo-crate",
        )
        .unwrap();
        let second = stage_artifact(
            &wasm,
            &root,
            "wasm32-unknown-unknown",
            "release",
            "demo-crate",
        )
        .unwrap();
        assert_eq!(
            first,
            root.join("rspyts/wasm32-unknown-unknown/debug/demo_crate.wasm")
        );
        assert_eq!(
            second,
            root.join("rspyts/wasm32-unknown-unknown/release/demo_crate.wasm")
        );
        assert_eq!(std::fs::read(first).unwrap(), b"wasm");
        std::fs::remove_dir_all(root).unwrap();
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

    #[test]
    fn read_toml_parses_a_complete_document() {
        let path = std::env::temp_dir().join(format!(
            "rspyts-cli-toml-document-{}.toml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
        )
        .unwrap();

        let document = read_toml(&path).unwrap();
        assert_eq!(document["package"]["name"].as_str(), Some("demo"));
        assert_eq!(document["lib"]["crate-type"][0].as_str(), Some("cdylib"));

        std::fs::remove_file(path).unwrap();
    }
}
