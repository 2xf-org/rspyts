//! Building the bridged crate and locating its cdylib artifact.
//!
//! The CLI never parses Rust source: it runs `cargo build
//! --message-format=json-render-diagnostics` (diagnostics stay
//! human-readable on stderr, artifact records arrive as JSON on stdout)
//! and picks the `cdylib` artifact belonging to the configured package.

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// `name` and `version` from the crate's `Cargo.toml`.
#[derive(Debug, PartialEq)]
pub struct CrateMeta {
    pub name: String,
    pub version: String,
}

/// Cargo's exact identity for the configured package and its bridge target.
///
/// Package IDs, rather than display names, are the only reliable way to
/// distinguish the requested crate from a dependency in Cargo's artifact
/// stream. The cdylib target name may also differ from the package name via
/// `[lib] name = "..."`.
#[derive(Debug, PartialEq, Eq)]
struct CargoPackage {
    package_id: String,
    cdylib_target: String,
    target_directory: PathBuf,
}

/// Why an artifact was included in a completed `rspyts build`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    /// The native cdylib built for the active rustc host.
    Native,
    /// An artifact built for an explicit command-line target.
    Target,
}

/// One artifact copied into rspyts' stable staging tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StagedArtifact {
    pub kind: ArtifactKind,
    pub target: String,
    pub path: PathBuf,
}

/// Cargo inputs which may affect a compiled manifest or artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildOptions {
    pub features: Vec<String>,
    pub no_default_features: bool,
    /// Cargo profile name (`dev`, `release`, or a custom profile).
    pub profile: String,
    pub locked: bool,
}

/// Cargo inputs that define the compiled contract independently of one build.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContractOptions {
    pub features: Vec<String>,
    pub no_default_features: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            features: Vec::new(),
            no_default_features: false,
            profile: "dev".to_string(),
            locked: false,
        }
    }
}

/// Positive invocation overrides applied to contract defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildOverrides {
    pub features: Option<Vec<String>>,
    pub no_default_features: Option<bool>,
    pub profile: Option<String>,
    pub locked: Option<bool>,
}

/// Artifacts selected by one `rspyts build` invocation.
///
/// With no explicit values, `build` selects the host. Otherwise each value is
/// either the literal `host` or a Rust target triple. This keeps target
/// selection command-scoped rather than hiding it in project configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildSelection {
    pub include_host: bool,
    pub targets: Vec<String>,
}

impl BuildSelection {
    pub fn new(values: Vec<String>) -> Result<Self> {
        if values.is_empty() {
            return Ok(Self {
                include_host: true,
                targets: Vec::new(),
            });
        }

        let mut include_host = false;
        let mut seen = BTreeSet::new();
        let mut targets = Vec::new();
        for value in values {
            if value == "host" {
                include_host = true;
            } else {
                validate_target(&value)?;
                if seen.insert(value.clone()) {
                    targets.push(value);
                }
            }
        }
        ensure!(
            include_host || !targets.is_empty(),
            "build requires `host` or at least one Rust target triple"
        );
        Ok(Self {
            include_host,
            targets,
        })
    }

    pub fn artifact_count(&self) -> usize {
        usize::from(self.include_host) + self.targets.len()
    }

    pub fn deduplicate_included_host(&mut self, host: &str) {
        if self.include_host {
            self.targets.retain(|target| target != host);
        }
    }
}

impl BuildOptions {
    pub fn new(
        features: Vec<String>,
        no_default_features: bool,
        profile: String,
        locked: bool,
    ) -> Result<Self> {
        validate_profile(&profile)?;
        for feature in &features {
            validate_feature(feature)?;
        }
        Ok(Self {
            features,
            no_default_features,
            profile,
            locked,
        })
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

impl ContractOptions {
    pub fn new(features: Vec<String>, no_default_features: bool) -> Result<Self> {
        for feature in &features {
            validate_feature(feature)?;
        }
        Ok(Self {
            features,
            no_default_features,
        })
    }

    /// Resolve one invocation. Profile and lock policy intentionally default
    /// here rather than in `rspyts.toml`.
    pub fn build_options(&self, overrides: BuildOverrides) -> Result<BuildOptions> {
        BuildOptions::new(
            overrides.features.unwrap_or_else(|| self.features.clone()),
            overrides
                .no_default_features
                .unwrap_or(self.no_default_features),
            overrides.profile.unwrap_or_else(|| "dev".to_string()),
            overrides.locked.unwrap_or(false),
        )
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
        !value.starts_with("wasm32") || value == "wasm32-unknown-unknown",
        "unsupported WebAssembly target `{value}`: rspyts supports only `wasm32-unknown-unknown`"
    );
    ensure!(
        !value.starts_with("wasm64-"),
        "unsupported WebAssembly target `{value}`: the TypeScript runtime uses wasm32 pointers; choose `wasm32-unknown-unknown`"
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
///
/// The target is mandatory, including for native builds. An explicit
/// `--target <rustc-host>` prevents ambient `CARGO_BUILD_TARGET` and Cargo's
/// `build.target` setting from silently turning a host build into a cross
/// build whose artifact would then be mislabeled as native.
pub fn cargo_args(name: &str, options: &BuildOptions, target: &str) -> Vec<String> {
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
    args.push("--target".to_string());
    args.push(target.to_string());
    args
}

/// Build the host cdylib used for manifest inspection.
///
/// The returned artifact remains in Cargo's target tree. Manifest-consuming
/// commands never publish it into a generated package; artifact staging is the
/// sole responsibility of [`build_and_stage`].
pub fn build_host_cdylib(crate_dir: &Path, name: &str, options: &BuildOptions) -> Result<PathBuf> {
    let package = cargo_package(crate_dir, name, options.locked)?;
    let host = rustc_host()?;
    build_artifact(crate_dir, name, &package, options, &host)
}

fn build_artifact(
    crate_dir: &Path,
    name: &str,
    package: &CargoPackage,
    options: &BuildOptions,
    target: &str,
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
        if let Some(path) = artifact_from_message(
            &line,
            &package.package_id,
            &package.cdylib_target,
            is_wasm_target(target),
        ) {
            artifact = Some(path);
        }
    }
    let status = child.wait().context("failed waiting for cargo")?;
    if !status.success() {
        bail!("`cargo build -p {name}` for target `{target}` failed");
    }
    artifact.with_context(|| {
        format!(
            "cargo produced no {} artifact for `{name}` for target `{target}` — does its Cargo.toml declare \
             `crate-type = [\"cdylib\", \"rlib\"]` under [lib]?",
            if is_wasm_target(target) {
                "WASM"
            } else {
                "cdylib"
            },
        )
    })
}

/// Extract a cdylib path from one `--message-format=json` line when both the
/// Cargo package ID and actual cdylib target name match the configured crate.
fn artifact_from_message(
    line: &str,
    package_id: &str,
    cdylib_target: &str,
    prefer_wasm: bool,
) -> Option<PathBuf> {
    let msg: serde_json::Value = serde_json::from_str(line).ok()?;
    if msg.get("reason")?.as_str()? != "compiler-artifact" {
        return None;
    }
    if msg.get("package_id")?.as_str()? != package_id {
        return None;
    }
    let target = msg.get("target")?;
    let is_cdylib = target
        .get("crate_types")?
        .as_array()?
        .iter()
        .any(|k| k.as_str() == Some("cdylib"));
    let target_matches = target.get("name")?.as_str() == Some(cdylib_target);
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
    target == "wasm32-unknown-unknown"
}

/// Resolve the configured package's exact Cargo ID, actual cdylib target name,
/// and target directory from one metadata snapshot.
fn cargo_package(crate_dir: &Path, name: &str, locked: bool) -> Result<CargoPackage> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(crate_dir)
        .args(cargo_metadata_args(locked));
    let output = command
        .output()
        .context("cannot run `cargo metadata` (is cargo on PATH?)")?;
    ensure!(output.status.success(), "`cargo metadata` failed");
    let manifest_path = crate_dir.join("Cargo.toml");
    let manifest_path = std::fs::canonicalize(&manifest_path).with_context(|| {
        format!(
            "cannot resolve configured manifest `{}`",
            manifest_path.display()
        )
    })?;
    parse_cargo_package(&output.stdout, &manifest_path, name)
}

fn cargo_metadata_args(locked: bool) -> Vec<&'static str> {
    let mut args = vec!["metadata", "--format-version=1", "--no-deps"];
    if locked {
        args.push("--locked");
    }
    args
}

fn parse_cargo_package(json: &[u8], manifest_path: &Path, name: &str) -> Result<CargoPackage> {
    let value: serde_json::Value =
        serde_json::from_slice(json).context("cannot parse `cargo metadata` output")?;
    let target = value
        .get("target_directory")
        .and_then(serde_json::Value::as_str)
        .context("`cargo metadata` output has no string target_directory")?;
    let target = PathBuf::from(target);
    ensure!(
        target.is_absolute(),
        "`cargo metadata` target_directory is not absolute"
    );

    let packages = value
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .context("`cargo metadata` output has no packages array")?;
    let mut matches = packages.iter().filter(|package| {
        let Some(path) = package
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
        else {
            return false;
        };
        comparable_path(Path::new(path)) == comparable_path(manifest_path)
    });
    let package = matches.next().with_context(|| {
        format!(
            "`cargo metadata` did not contain configured manifest `{}`",
            manifest_path.display()
        )
    })?;
    ensure!(
        matches.next().is_none(),
        "`cargo metadata` contained configured manifest `{}` more than once",
        manifest_path.display()
    );
    let metadata_name = package
        .get("name")
        .and_then(serde_json::Value::as_str)
        .context("configured Cargo package has no string name")?;
    ensure!(
        metadata_name == name,
        "configured package name changed between Cargo.toml and metadata: expected `{name}`, found `{metadata_name}`"
    );
    let package_id = package
        .get("id")
        .and_then(serde_json::Value::as_str)
        .context("configured Cargo package has no string id")?
        .to_string();

    let targets = package
        .get("targets")
        .and_then(serde_json::Value::as_array)
        .context("configured Cargo package has no targets array")?;
    let mut cdylibs = targets.iter().filter(|candidate| {
        candidate
            .get("crate_types")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|types| types.iter().any(|kind| kind.as_str() == Some("cdylib")))
    });
    let cdylib = cdylibs.next().with_context(|| {
        format!(
            "Cargo package `{name}` has no cdylib target — declare `crate-type = [\"cdylib\", \"rlib\"]` under [lib]"
        )
    })?;
    ensure!(
        cdylibs.next().is_none(),
        "Cargo package `{name}` has more than one cdylib target"
    );
    let cdylib_target = cdylib
        .get("name")
        .and_then(serde_json::Value::as_str)
        .context("configured cdylib target has no string name")?
        .to_string();

    Ok(CargoPackage {
        package_id,
        cdylib_target,
        target_directory: target,
    })
}

fn comparable_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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

/// Build and stage exactly the selected artifacts.
pub fn build_and_stage(
    crate_dir: &Path,
    name: &str,
    options: &BuildOptions,
    selection: &BuildSelection,
    python_out: Option<&Path>,
    output_dir: Option<&Path>,
) -> Result<Vec<StagedArtifact>> {
    let package = cargo_package(crate_dir, name, options.locked)?;
    let host = rustc_host()?;
    let mut staged = Vec::new();
    let staging = StagingContext {
        target_dir: &package.target_directory,
        profile_dir: options.profile_dir(),
        package_name: name,
        python_out,
        output_dir,
    };

    if selection.include_host {
        let host_artifact = build_artifact(crate_dir, name, &package, options, &host)?;
        staged.push(stage_artifact(
            ArtifactKind::Native,
            &host_artifact,
            host.clone(),
            &staging,
        )?);
    }

    for target in &selection.targets {
        if selection.include_host && target == &host {
            continue;
        }
        let artifact = build_artifact(crate_dir, name, &package, options, target)?;
        staged.push(stage_artifact(
            ArtifactKind::Target,
            &artifact,
            target.clone(),
            &staging,
        )?);
    }
    Ok(staged)
}

#[derive(Clone, Copy)]
struct StagingContext<'a> {
    target_dir: &'a Path,
    profile_dir: &'a str,
    package_name: &'a str,
    python_out: Option<&'a Path>,
    output_dir: Option<&'a Path>,
}

fn stage_artifact(
    kind: ArtifactKind,
    source: &Path,
    target: String,
    context: &StagingContext<'_>,
) -> Result<StagedArtifact> {
    let filename = staged_filename(source, context.package_name)?;
    let destination = if let Some(output_dir) = context.output_dir {
        output_dir.join(filename)
    } else {
        match (kind, context.python_out) {
            (ArtifactKind::Native, Some(out)) => out.join("lib").join(filename),
            (ArtifactKind::Native, None) => context
                .target_dir
                .join("rspyts")
                .join("native")
                .join(context.profile_dir)
                .join(filename),
            (ArtifactKind::Target, _) => context
                .target_dir
                .join("rspyts")
                .join(&target)
                .join(context.profile_dir)
                .join(filename),
        }
    };
    let destination = std::path::absolute(&destination).with_context(|| {
        format!(
            "cannot make staged artifact path `{}` absolute",
            destination.display()
        )
    })?;
    std::fs::create_dir_all(destination.parent().expect("destination has a parent")).with_context(
        || {
            format!(
                "cannot create staging directory for `{}`",
                destination.display()
            )
        },
    )?;
    copy_atomically(source, &destination)?;
    Ok(StagedArtifact {
        kind,
        target,
        path: destination,
    })
}

static STAGE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn copy_atomically(source: &Path, destination: &Path) -> Result<()> {
    let parent = destination
        .parent()
        .context("staged artifact destination has no parent")?;
    let filename = destination
        .file_name()
        .and_then(|name| name.to_str())
        .context("staged artifact destination has no UTF-8 filename")?;
    let counter = STAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{filename}.rspyts-stage-{}-{counter}.tmp",
        std::process::id()
    ));

    if let Err(error) = std::fs::copy(source, &temporary) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| {
            format!(
                "cannot copy `{}` to temporary staging path `{}`",
                source.display(),
                temporary.display()
            )
        });
    }
    if let Err(error) = replace_file(&temporary, destination) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| {
            format!(
                "cannot atomically stage `{}` as `{}`",
                source.display(),
                destination.display()
            )
        });
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both paths are valid, NUL-terminated UTF-16 buffers that remain
    // alive for the duration of the Windows API call.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn staged_filename(source: &Path, name: &str) -> Result<String> {
    let extension = source
        .extension()
        .and_then(|ext| ext.to_str())
        .context("compiled artifact path has no UTF-8 extension")?;
    let stem = normalize(name);
    Ok(match extension {
        "wasm" => format!("{stem}.wasm"),
        "dll" => format!("{stem}.dll"),
        "dylib" | "so" => format!("lib{stem}.{extension}"),
        _ => bail!(
            "compiled artifact `{}` has unsupported extension `{extension}`",
            source.display()
        ),
    })
}

fn normalize(name: &str) -> String {
    name.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_message_matches_package_id_and_actual_lib_target() {
        let package_id = "path+file:///w#demo-crate@0.1.0";
        let line = r#"{"reason":"compiler-artifact","package_id":"path+file:///w#demo-crate@0.1.0","target":{"kind":["lib"],"crate_types":["rlib","cdylib"],"name":"actual_bridge"},"filenames":["/t/libactual_bridge.rlib","/t/libactual_bridge.dylib"]}"#;
        assert_eq!(
            artifact_from_message(line, package_id, "actual_bridge", false),
            Some(PathBuf::from("/t/libactual_bridge.dylib"))
        );

        let wasm = r#"{"reason":"compiler-artifact","package_id":"path+file:///w#demo-crate@0.1.0","target":{"kind":["lib"],"crate_types":["cdylib"],"name":"actual_bridge"},"filenames":["/t/actual_bridge.rlib","/t/actual_bridge.wasm"]}"#;
        assert_eq!(
            artifact_from_message(wasm, package_id, "actual_bridge", true),
            Some(PathBuf::from("/t/actual_bridge.wasm"))
        );
    }

    #[test]
    fn dependencies_and_non_cdylib_targets_are_skipped() {
        let package_id = "path+file:///w#demo-crate@0.1.0";
        let rlib_only = r#"{"reason":"compiler-artifact","package_id":"path+file:///w#demo-crate@0.1.0","target":{"kind":["lib"],"crate_types":["rlib"],"name":"actual_bridge"},"filenames":["/t/libactual_bridge.rlib"]}"#;
        assert_eq!(
            artifact_from_message(rlib_only, package_id, "actual_bridge", false),
            None
        );
        // A dependency can deliberately use the same lib target name and
        // produce a plausible cdylib filename. Its package ID still rejects it.
        let dependency = r#"{"reason":"compiler-artifact","package_id":"path+file:///w#deceptive-dependency@9.9.9","target":{"kind":["lib"],"crate_types":["cdylib"],"name":"actual_bridge"},"filenames":["/t/libactual_bridge.so"]}"#;
        assert_eq!(
            artifact_from_message(dependency, package_id, "actual_bridge", false),
            None
        );
        let wrong_target = r#"{"reason":"compiler-artifact","package_id":"path+file:///w#demo-crate@0.1.0","target":{"kind":["lib"],"crate_types":["cdylib"],"name":"other_bridge"},"filenames":["/t/libother_bridge.so"]}"#;
        assert_eq!(
            artifact_from_message(wrong_target, package_id, "actual_bridge", false),
            None
        );
        assert_eq!(
            artifact_from_message("not json", package_id, "actual_bridge", false),
            None
        );
    }

    #[test]
    fn cargo_arguments_are_deterministic_for_every_option() {
        let options = BuildOptions::new(
            vec!["serde".into(), "fast".into()],
            true,
            "release".into(),
            true,
        )
        .unwrap();
        assert_eq!(
            cargo_args("demo", &options, "wasm32-unknown-unknown"),
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

        let custom = BuildOptions::new(vec![], false, "bench-local".into(), false).unwrap();
        assert_eq!(
            cargo_args("demo", &custom, "aarch64-apple-darwin"),
            [
                "build",
                "--message-format=json-render-diagnostics",
                "-p",
                "demo",
                "--profile",
                "bench-local",
                "--target",
                "aarch64-apple-darwin",
            ]
        );
    }

    #[test]
    fn native_cargo_arguments_explicitly_override_ambient_cross_target() {
        // Cargo gives an explicit CLI `--target` precedence over both the
        // CARGO_BUILD_TARGET environment variable and build.target config.
        // Requiring the discovered host here therefore makes it impossible
        // for an ambient cross-target to be reported as a native artifact.
        let options = BuildOptions::default();
        let host = "aarch64-apple-darwin";
        let args = cargo_args("demo", &options, host);

        assert_eq!(args[args.len() - 2..], ["--target", host]);
        assert_eq!(args.iter().filter(|arg| *arg == "--target").count(), 1);
    }

    #[test]
    fn contract_and_invocation_options_have_separate_defaults() {
        let contract = ContractOptions::new(vec!["a".into()], true).unwrap();
        assert_eq!(
            contract.build_options(BuildOverrides::default()).unwrap(),
            BuildOptions::new(vec!["a".into()], true, "dev".into(), false).unwrap()
        );

        let resolved = contract
            .build_options(BuildOverrides {
                features: Some(vec!["b".into()]),
                no_default_features: None,
                profile: Some("release".into()),
                locked: Some(true),
            })
            .unwrap();
        assert_eq!(
            resolved,
            BuildOptions::new(vec!["b".into()], true, "release".into(), true).unwrap()
        );
    }

    #[test]
    fn feature_parser_and_validation_are_strict() {
        assert_eq!(parse_features("a,b c").unwrap(), ["a", "b", "c"]);
        assert!(parse_features(" , ").is_err());
        assert!(BuildOptions::new(vec![], false, "bad profile".into(), false).is_err());

        let default = BuildSelection::new(Vec::new()).unwrap();
        assert!(default.include_host);
        assert!(default.targets.is_empty());

        let selected = BuildSelection::new(vec![
            "host".into(),
            "wasm32-unknown-unknown".into(),
            "wasm32-unknown-unknown".into(),
        ])
        .unwrap();
        assert!(selected.include_host);
        assert_eq!(selected.targets, ["wasm32-unknown-unknown"]);

        for invalid in [
            "bad target",
            "wasm32v1-none",
            "wasm32-wasip1",
            "wasm32-unknown-emscripten",
        ] {
            assert!(BuildSelection::new(vec![invalid.into()]).is_err());
        }
    }

    #[test]
    fn metadata_and_rustc_discovery_parsers_are_strict() {
        assert_eq!(
            cargo_metadata_args(false),
            ["metadata", "--format-version=1", "--no-deps"]
        );
        assert_eq!(
            cargo_metadata_args(true),
            ["metadata", "--format-version=1", "--no-deps", "--locked"]
        );
        let metadata = br#"{
            "target_directory":"/workspace/target",
            "packages":[
                {
                    "id":"path+file:///workspace/dependency#0.1.0",
                    "name":"dependency",
                    "manifest_path":"/workspace/dependency/Cargo.toml",
                    "targets":[{"name":"actual_bridge","crate_types":["cdylib"]}]
                },
                {
                    "id":"path+file:///workspace/demo#demo-package@1.2.3",
                    "name":"demo-package",
                    "manifest_path":"/workspace/demo/Cargo.toml",
                    "targets":[{"name":"actual_bridge","crate_types":["rlib","cdylib"]}]
                }
            ]
        }"#;
        assert_eq!(
            parse_cargo_package(
                metadata,
                Path::new("/workspace/demo/Cargo.toml"),
                "demo-package"
            )
            .unwrap(),
            CargoPackage {
                package_id: "path+file:///workspace/demo#demo-package@1.2.3".into(),
                cdylib_target: "actual_bridge".into(),
                target_directory: PathBuf::from("/workspace/target"),
            }
        );
        assert!(
            parse_cargo_package(
                metadata,
                Path::new("/workspace/missing/Cargo.toml"),
                "demo-package"
            )
            .is_err()
        );
        assert!(
            parse_cargo_package(
                br#"{"target_directory":"target","packages":[]}"#,
                Path::new("/workspace/demo/Cargo.toml"),
                "demo-package"
            )
            .is_err()
        );
        assert_eq!(
            parse_rustc_host("rustc 1.85.0\nhost: aarch64-apple-darwin\n").unwrap(),
            "aarch64-apple-darwin"
        );
        assert!(parse_rustc_host("rustc 1.85.0\n").is_err());
    }

    #[test]
    fn staging_uses_stable_target_and_python_paths_atomically() {
        let root = std::env::temp_dir().join(format!("rspyts-cli-stage-{}", std::process::id()));
        let source_dir = root.join("source");
        std::fs::create_dir_all(&source_dir).unwrap();
        let wasm = source_dir.join("hashed-name.wasm");
        std::fs::write(&wasm, b"wasm").unwrap();
        let debug_staging = StagingContext {
            target_dir: &root,
            profile_dir: "debug",
            package_name: "demo-crate",
            python_out: None,
            output_dir: None,
        };
        let first = stage_artifact(
            ArtifactKind::Target,
            &wasm,
            "wasm32-unknown-unknown".into(),
            &debug_staging,
        )
        .unwrap();
        let release_staging = StagingContext {
            profile_dir: "release",
            ..debug_staging
        };
        let second = stage_artifact(
            ArtifactKind::Target,
            &wasm,
            "wasm32-unknown-unknown".into(),
            &release_staging,
        )
        .unwrap();
        assert_eq!(
            first.path,
            root.join("rspyts/wasm32-unknown-unknown/debug/demo_crate.wasm")
        );
        assert_eq!(
            second.path,
            root.join("rspyts/wasm32-unknown-unknown/release/demo_crate.wasm")
        );
        assert_eq!(first.kind, ArtifactKind::Target);
        assert_eq!(first.target, "wasm32-unknown-unknown");
        assert_eq!(std::fs::read(first.path).unwrap(), b"wasm");

        let native = source_dir.join("librenamed_target.dylib");
        std::fs::write(&native, b"native").unwrap();
        let python_out = root.join("python/src/demo/_generated");
        let python_staging = StagingContext {
            python_out: Some(&python_out),
            ..debug_staging
        };
        let staged_native = stage_artifact(
            ArtifactKind::Native,
            &native,
            "aarch64-apple-darwin".into(),
            &python_staging,
        )
        .unwrap();
        assert_eq!(
            staged_native.path,
            python_out.join("lib/libdemo_crate.dylib")
        );
        assert_eq!(std::fs::read(staged_native.path).unwrap(), b"native");
        std::fs::write(&native, b"replacement").unwrap();
        let replacement = stage_artifact(
            ArtifactKind::Native,
            &native,
            "aarch64-apple-darwin".into(),
            &python_staging,
        )
        .unwrap();
        assert_eq!(std::fs::read(replacement.path).unwrap(), b"replacement");
        assert!(
            std::fs::read_dir(python_out.join("lib"))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".tmp"))
        );

        let fallback_native = stage_artifact(
            ArtifactKind::Native,
            &native,
            "aarch64-apple-darwin".into(),
            &release_staging,
        )
        .unwrap();
        assert_eq!(
            fallback_native.path,
            root.join("rspyts/native/release/libdemo_crate.dylib")
        );

        let explicit_out = root.join("package-input");
        let explicit_staging = StagingContext {
            output_dir: Some(&explicit_out),
            ..python_staging
        };
        let explicit_native = stage_artifact(
            ArtifactKind::Native,
            &native,
            "aarch64-apple-darwin".into(),
            &explicit_staging,
        )
        .unwrap();
        assert_eq!(
            explicit_native.path,
            explicit_out.join("libdemo_crate.dylib")
        );
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
