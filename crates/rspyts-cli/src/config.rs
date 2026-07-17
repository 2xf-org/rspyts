use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(rename = "crate")]
    rust_crate: CrateConfig,
    python: Option<PythonConfig>,
    typescript: Option<TypeScriptConfig>,
    #[serde(default)]
    dependencies: BTreeMap<String, RawDependencyConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct CrateConfig {
    path: PathBuf,
    #[serde(default)]
    features: Vec<String>,
    #[serde(rename = "probe-features")]
    probe_features: Option<Vec<String>>,
    #[serde(rename = "default-features", default)]
    default_features: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonConfig {
    pub package: String,
    pub source: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeScriptConfig {
    pub package: String,
    pub mode: TypeScriptMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TypeScriptMode {
    Static,
    Wasm,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDependencyConfig {
    #[serde(rename = "crate")]
    owner: String,
    lock: PathBuf,
    python: Option<String>,
    typescript: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DependencyConfig {
    pub owner: rspyts::ir::CargoPackageId,
    pub lock: PathBuf,
    pub python: Option<String>,
    pub typescript: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Project {
    root: PathBuf,
    cargo_manifest: PathBuf,
    features: Vec<String>,
    probe_features: Vec<String>,
    default_features: bool,
    pub python: Option<PythonConfig>,
    pub typescript: Option<TypeScriptConfig>,
    dependencies: BTreeMap<String, DependencyConfig>,
}

impl Project {
    pub fn read(path: &Path) -> Result<Self> {
        let config_path = path
            .canonicalize()
            .with_context(|| format!("failed to resolve config {}", path.display()))?;
        let source = fs::read_to_string(&config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        let config: Config = toml::from_str(&source)
            .with_context(|| format!("invalid config {}", config_path.display()))?;
        if config.python.is_none() && config.typescript.is_none() {
            bail!("rspyts.toml must configure at least one of [python] or [typescript]");
        }

        let root = config_path
            .parent()
            .context("rspyts.toml has no parent directory")?
            .to_path_buf();
        if config.rust_crate.path.as_os_str().is_empty()
            || config.rust_crate.path.is_absolute()
            || config.rust_crate.path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            bail!("[crate].path must stay inside the rspyts project");
        }
        let crate_path = root.join(&config.rust_crate.path);
        let cargo_manifest = if crate_path
            .file_name()
            .is_some_and(|name| name == "Cargo.toml")
        {
            crate_path
        } else {
            crate_path.join("Cargo.toml")
        };
        if !cargo_manifest.is_file() {
            bail!(
                "configured Rust crate has no Cargo manifest at {}",
                cargo_manifest.display()
            );
        }
        let cargo_manifest = cargo_manifest
            .canonicalize()
            .with_context(|| format!("failed to resolve {}", cargo_manifest.display()))?;
        if !cargo_manifest.starts_with(&root) {
            bail!("[crate].path must stay inside the rspyts project");
        }

        validate_python_package(config.python.as_ref())?;
        validate_typescript_package(config.typescript.as_ref())?;
        validate_features("features", &config.rust_crate.features)?;
        if let Some(features) = config.rust_crate.probe_features.as_deref() {
            validate_features("probe-features", features)?;
        }

        let mut python = config.python;
        if let Some(config) = python.as_mut() {
            config.source = resolve_source(&root, config.source.as_deref(), "python")?;
        }
        let typescript = config.typescript;
        let dependencies = resolve_dependencies(&root, config.dependencies)?;

        let probe_features = config
            .rust_crate
            .probe_features
            .unwrap_or_else(|| config.rust_crate.features.clone());
        Ok(Self {
            root,
            cargo_manifest,
            features: config.rust_crate.features,
            probe_features,
            default_features: config.rust_crate.default_features,
            python,
            typescript,
            dependencies,
        })
    }

    pub fn cargo_manifest(&self) -> &Path {
        &self.cargo_manifest
    }

    pub fn features(&self) -> &[String] {
        &self.features
    }

    pub fn default_features(&self) -> bool {
        self.default_features
    }

    pub fn probe_features(&self) -> &[String] {
        &self.probe_features
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dependencies(&self) -> &BTreeMap<String, DependencyConfig> {
        &self.dependencies
    }

    pub fn output_dir(&self) -> PathBuf {
        self.root.join(".rspyts")
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join("rspyts.lock")
    }
}

fn resolve_dependencies(
    root: &Path,
    dependencies: BTreeMap<String, RawDependencyConfig>,
) -> Result<BTreeMap<String, DependencyConfig>> {
    if dependencies.len() > 1 {
        bail!("rspyts supports at most one direct dependency");
    }
    dependencies
        .into_iter()
        .map(|(alias, dependency)| {
            if !is_identifier(&alias) {
                bail!("dependency alias `{alias}` must be an identifier");
            }
            if !is_cargo_package_name(&dependency.owner) {
                bail!("dependency `{alias}` has an invalid Cargo package owner");
            }
            if dependency.lock.as_os_str().is_empty() || dependency.lock.is_absolute() {
                bail!("dependency `{alias}` lock must be a non-empty relative path");
            }
            if let Some(package) = dependency.python.as_deref() {
                validate_python_name(package)?;
            }
            if let Some(package) = dependency.typescript.as_deref() {
                validate_typescript_name(package)?;
            }
            Ok((
                alias,
                DependencyConfig {
                    owner: rspyts::ir::CargoPackageId::new(dependency.owner),
                    lock: root.join(dependency.lock),
                    python: dependency.python,
                    typescript: dependency.typescript,
                },
            ))
        })
        .collect()
}

fn resolve_source(root: &Path, source: Option<&Path>, host: &str) -> Result<Option<PathBuf>> {
    let Some(source) = source else {
        return Ok(None);
    };
    if source.as_os_str().is_empty() || source.is_absolute() {
        bail!("[{host}].source must be a non-empty path relative to rspyts.toml");
    }

    let mut candidate = root.to_path_buf();
    for component in source.components() {
        use std::path::Component;
        match component {
            Component::Normal(part) => {
                candidate.push(part);
                let metadata = fs::symlink_metadata(&candidate).with_context(|| {
                    format!(
                        "[{host}].source component does not exist: {}",
                        candidate.display()
                    )
                })?;
                if metadata.file_type().is_symlink() {
                    bail!(
                        "[{host}].source may not traverse symlink {}",
                        candidate.display()
                    );
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("[{host}].source may not contain parent or rooted components");
            }
        }
    }

    let resolved = candidate
        .canonicalize()
        .with_context(|| format!("failed to resolve [{host}].source {}", candidate.display()))?;
    if !resolved.starts_with(root) {
        bail!("[{host}].source resolves outside the rspyts project");
    }
    if resolved == root {
        bail!("[{host}].source may not be the rspyts project root");
    }
    if !resolved.is_dir() {
        bail!("[{host}].source must resolve to a directory");
    }
    Ok(Some(resolved))
}

fn validate_python_package(config: Option<&PythonConfig>) -> Result<()> {
    let Some(config) = config else {
        return Ok(());
    };
    validate_python_name(&config.package)
}

fn validate_python_name(package: &str) -> Result<()> {
    if package.is_empty()
        || package
            .split('.')
            .any(|part| !is_identifier(part) || is_python_keyword(part))
    {
        bail!("Python package `{package}` must contain only dot-separated identifiers");
    }
    let root = package
        .split('.')
        .next()
        .expect("a non-empty package has a root segment");
    if matches!(root, "math" | "enum" | "typing" | "pydantic" | "numpy") {
        bail!(
            "Python package root `{root}` shadows a runtime module required by generated bindings"
        );
    }
    Ok(())
}

fn validate_typescript_package(config: Option<&TypeScriptConfig>) -> Result<()> {
    let Some(config) = config else {
        return Ok(());
    };
    validate_typescript_name(&config.package)
}

fn validate_typescript_name(package: &str) -> Result<()> {
    let name = package.strip_prefix('@').unwrap_or(package);
    let parts = name.split('/').collect::<Vec<_>>();
    let expected_parts = usize::from(package.starts_with('@')) + 1;
    if package.is_empty()
        || package.len() > 214
        || parts.len() != expected_parts
        || (!package.starts_with('@') && matches!(package, "node_modules" | "favicon.ico"))
        || parts.iter().any(|part| {
            part.is_empty()
                || part.starts_with(['.', '_'])
                || !part.chars().all(|character| {
                    character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || "-_.".contains(character)
                })
        })
    {
        bail!("invalid TypeScript package `{package}`");
    }
    Ok(())
}

fn validate_features(label: &str, features: &[String]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for feature in features {
        if feature.trim().is_empty()
            || feature.contains(char::is_whitespace)
            || feature.contains(',')
        {
            bail!("invalid Cargo feature `{feature}` in [crate].{label}");
        }
        if matches!(feature.as_str(), "python" | "wasm") {
            bail!(
                "[crate].{label} must be backend-neutral; rspyts adds `{feature}` for its target build"
            );
        }
        if !seen.insert(feature) {
            bail!("duplicate Cargo feature `{feature}` in [crate].{label}");
        }
    }
    Ok(())
}

fn is_cargo_package_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn is_python_keyword(value: &str) -> bool {
    matches!(
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
    )
}

fn is_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn accepts_host_package_names() {
        assert!(
            validate_python_package(Some(&PythonConfig {
                package: "example.shared".into(),
                source: None,
            }))
            .is_ok()
        );
        assert!(
            validate_typescript_package(Some(&TypeScriptConfig {
                package: "@example/shared".into(),
                mode: TypeScriptMode::Static,
            }))
            .is_ok()
        );
    }

    #[test]
    fn rejects_ambiguous_package_names() {
        assert!(
            validate_python_package(Some(&PythonConfig {
                package: "example/shared".into(),
                source: None,
            }))
            .is_err()
        );
        assert!(
            validate_typescript_package(Some(&TypeScriptConfig {
                package: "@example".into(),
                mode: TypeScriptMode::Wasm,
            }))
            .is_err()
        );
        assert!(validate_python_name("example.class").is_err());
        assert!(validate_typescript_name("@Example/Shared").is_err());
        assert!(validate_typescript_name("node_modules").is_err());
        assert!(validate_typescript_name("favicon.ico").is_err());
        assert!(validate_typescript_name("@example/node_modules").is_ok());
        assert!(validate_typescript_name("@example/favicon.ico").is_ok());
        for root in ["math", "enum", "typing", "pydantic", "numpy"] {
            let error = validate_python_name(&format!("{root}.contract")).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("shadows a runtime module required by generated bindings")
            );
        }
        assert!(validate_python_name("example.math").is_ok());
    }

    #[test]
    fn project_config_rejects_python_packages_that_shadow_runtime_imports() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-config-python-shadow-{}-{}",
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

        for package in ["math", "enum.contract", "typing", "pydantic.v2", "numpy"] {
            fs::write(
                root.join("rspyts.toml"),
                format!("[crate]\npath = \"rust\"\n\n[python]\npackage = \"{package}\"\n"),
            )
            .unwrap();
            let error = Project::read(&root.join("rspyts.toml")).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("shadows a runtime module required by generated bindings"),
                "unexpected error for {package}: {error:#}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_removed_python_mode_configuration() {
        let error = toml::from_str::<Config>(
            "[crate]\npath = \"rust\"\n\n[python]\npackage = \"fixture\"\nmode = \"source\"\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field `mode`"));
    }

    #[test]
    fn rejects_crates_outside_the_project() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-config-crate-path-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();

        for path in ["/outside", "../outside"] {
            fs::write(
                root.join("rspyts.toml"),
                format!(
                    "[crate]\npath = {path:?}\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n"
                ),
            )
            .unwrap();
            let error = Project::read(&root.join("rspyts.toml")).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("[crate].path must stay inside the rspyts project"),
                "unexpected error for {path}: {error:#}"
            );
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_ambiguous_cargo_feature_configuration() {
        assert!(validate_features("features", &["domain".into(), "domain".into()]).is_err());
        assert!(validate_features("features", &["python".into()]).is_err());
        assert!(validate_features("probe-features", &["wasm".into()]).is_err());
        assert!(validate_features("features", &["one,two".into()]).is_err());
        assert!(validate_features("features", &["formats/native".into()]).is_ok());
    }

    #[test]
    fn rejects_more_than_one_direct_dependency() {
        let dependency = |owner: &str| RawDependencyConfig {
            owner: owner.into(),
            lock: PathBuf::from(format!("{owner}.lock")),
            python: None,
            typescript: None,
        };
        let dependencies = BTreeMap::from([
            ("first".into(), dependency("first-owner")),
            ("second".into(), dependency("second-owner")),
        ]);

        let error = resolve_dependencies(Path::new("."), dependencies).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("supports at most one direct dependency")
        );
    }

    #[test]
    fn resolves_authored_sources_relative_to_the_configuration() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-config-source-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("rust/src")).unwrap();
        fs::create_dir_all(root.join("python-src")).unwrap();
        fs::write(
            root.join("rust/Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(root.join("rust/src/lib.rs"), "").unwrap();
        fs::write(
            root.join("rspyts.toml"),
            "[crate]\npath = \"rust\"\n\n[python]\npackage = \"fixture\"\nsource = \"python-src\"\n",
        )
        .unwrap();

        let project = Project::read(&root.join("rspyts.toml")).unwrap();
        assert_eq!(
            project.python.unwrap().source.unwrap(),
            root.join("python-src").canonicalize().unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn crate_default_features_are_opt_in() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-config-default-features-{}-{}",
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
        assert!(
            !Project::read(&root.join("rspyts.toml"))
                .unwrap()
                .default_features()
        );

        fs::write(
            root.join("rspyts.toml"),
            "[crate]\npath = \"rust\"\ndefault-features = true\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
        )
        .unwrap();
        assert!(
            Project::read(&root.join("rspyts.toml"))
                .unwrap()
                .default_features()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_typescript_source_instead_of_staging_unpublished_files() {
        let error = toml::from_str::<Config>(
            "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\nsource = \"typescript/src\"\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field `source`"));
    }
}
