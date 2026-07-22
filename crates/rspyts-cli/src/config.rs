use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use toml_edit::DocumentMut;

pub(super) const CONFIG_FILE: &str = "rspyts.toml";

pub(super) const CONFIG_TEMPLATE: &str = r#"# RSPYTS application configuration and generated-file ownership.
# Edit [application]. RSPYTS updates only the [generated] tables.

[application]
# Manifests and root entrypoints are user-owned; update them with any override.

# Override the public application name.
# Defaults to the adjacent Cargo package name.
# name = "my-application"

# Link additional Cargo workspace packages into the application.
# The adjacent Cargo package is always linked automatically.
# additional_packages = ["my-shared-api"]

# Override the Python import package.
# Defaults to the application name with hyphens replaced by underscores.
# python_package = "my_application"

# Override the npm package name.
# Defaults to the application name and must match src-ts/package.json.
# typescript_package = "my-application"

# Generate src-py/.gitignore and src-ts/.gitignore for RSPYTS-owned files.
# Defaults to true. Set this to false to allow generated files to be committed.
# gitignore = false

[generated]
# Fingerprint of the Rust/Cargo sources and active [application] settings.
source_fingerprint = ""

[generated.python]
# Python files owned by RSPYTS and safe to overwrite or remove.
files = ["src-py/.gitignore"]

# Extension-module basenames; the platform supplies .so or .pyd.
native_modules = []

[generated.typescript]
# TypeScript, wasm-bindgen declarations/JavaScript, and Wasm files owned by RSPYTS.
files = ["src-ts/.gitignore"]
"#;

const GENERATED_TEMPLATE: &str = r#"[generated]
# Fingerprint of the Rust/Cargo sources and active [application] settings.
source_fingerprint = __SOURCE_FINGERPRINT__

[generated.python]
# Python files owned by RSPYTS and safe to overwrite or remove.
files = __PYTHON_FILES__

# Extension-module basenames; the platform supplies .so or .pyd.
native_modules = __PYTHON_NATIVE_MODULES__

[generated.typescript]
# TypeScript, wasm-bindgen declarations/JavaScript, and Wasm files owned by RSPYTS.
files = __TYPESCRIPT_FILES__
"#;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ApplicationSettings {
    pub(super) name: Option<String>,
    pub(super) additional_packages: Vec<String>,
    pub(super) python_package: Option<String>,
    pub(super) typescript_package: Option<String>,
    pub(super) gitignore: bool,
}

impl Default for ApplicationSettings {
    fn default() -> Self {
        Self {
            name: None,
            additional_packages: Vec::new(),
            python_package: None,
            typescript_package: None,
            gitignore: true,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(super) struct GeneratedState {
    pub(super) source_fingerprint: String,
    pub(super) python: PythonGeneratedState,
    pub(super) typescript: TypeScriptGeneratedState,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(super) struct PythonGeneratedState {
    pub(super) files: Vec<String>,
    pub(super) native_modules: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(super) struct TypeScriptGeneratedState {
    pub(super) files: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigData {
    application: ApplicationSettings,
    generated: GeneratedState,
}

#[derive(Clone, Debug)]
pub(super) struct Config {
    pub(super) path: PathBuf,
    pub(super) application: ApplicationSettings,
    pub(super) generated: GeneratedState,
    document: DocumentMut,
}

impl Config {
    pub(super) fn read(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .with_context(|| format!("cannot find RSPYTS configuration {}", path.display()))?;
        let source = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let document = source
            .parse::<DocumentMut>()
            .with_context(|| format!("failed to parse {}", path.display()))?;
        let data = toml::from_str::<ConfigData>(&source)
            .with_context(|| format!("invalid RSPYTS configuration in {}", path.display()))?;
        Ok(Self {
            path,
            application: data.application,
            generated: data.generated,
            document,
        })
    }

    pub(super) fn root(&self) -> &Path {
        self.path
            .parent()
            .expect("a canonical configuration path has a parent")
    }

    pub(super) fn render_generated(&self, generated: &GeneratedState) -> Result<String> {
        let fragment = GENERATED_TEMPLATE
            .replace(
                "__SOURCE_FINGERPRINT__",
                &quoted(&generated.source_fingerprint),
            )
            .replace("__PYTHON_FILES__", &string_array(&generated.python.files))
            .replace(
                "__PYTHON_NATIVE_MODULES__",
                &string_array(&generated.python.native_modules),
            )
            .replace(
                "__TYPESCRIPT_FILES__",
                &string_array(&generated.typescript.files),
            );
        let mut fragment = fragment
            .parse::<DocumentMut>()
            .context("failed to render generated RSPYTS configuration")?;
        let mut document = self.document.clone();
        let generated_decor = document["generated"]
            .as_table()
            .map(|table| table.decor().clone());
        let mut generated = fragment
            .remove("generated")
            .context("rendered RSPYTS configuration omitted [generated]")?;
        if let (Some(decor), Some(table)) = (generated_decor, generated.as_table_mut()) {
            *table.decor_mut() = decor;
        }
        document["generated"] = generated;
        Ok(document.to_string())
    }

    pub(super) fn application_fingerprint_source(&self) -> Result<String> {
        let mut document = DocumentMut::new();
        if let Some(name) = &self.application.name {
            document["name"] = toml_edit::value(name);
        }
        if !self.application.additional_packages.is_empty() {
            document["additional_packages"] = string_array_value(
                self.application
                    .additional_packages
                    .iter()
                    .map(String::as_str),
            );
        }
        if let Some(package) = &self.application.python_package {
            document["python_package"] = toml_edit::value(package);
        }
        if let Some(package) = &self.application.typescript_package {
            document["typescript_package"] = toml_edit::value(package);
        }
        if !self.application.gitignore {
            document["gitignore"] = toml_edit::value(false);
        }
        Ok(document.to_string())
    }
}

pub(super) fn discover(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        let path = if path.is_dir() {
            path.join(CONFIG_FILE)
        } else {
            path.to_path_buf()
        };
        return path
            .canonicalize()
            .with_context(|| format!("cannot find RSPYTS configuration {}", path.display()));
    }

    let current = std::env::current_dir().context("failed to read the current directory")?;
    if let Some(path) = ancestors(&current)
        .map(|directory| directory.join(CONFIG_FILE))
        .find(|path| path.is_file())
    {
        return path.canonicalize().map_err(Into::into);
    }

    let cargo_manifest = ancestors(&current)
        .map(|directory| directory.join("Cargo.toml"))
        .find(|path| path.is_file())
        .context("cannot find rspyts.toml or an enclosing Cargo workspace")?;
    let output = ProcessCommand::new(cargo())
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(&cargo_manifest)
        .output()
        .context("failed to run cargo metadata while locating rspyts.toml")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed while locating rspyts.toml\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let workspace_members = metadata["workspace_members"]
        .as_array()
        .context("Cargo metadata has no workspace member list")?;
    let mut candidates = metadata["packages"]
        .as_array()
        .context("Cargo metadata has no package list")?
        .iter()
        .filter(|package| {
            package["id"]
                .as_str()
                .is_some_and(|id| workspace_members.iter().any(|member| member == id))
        })
        .filter_map(|package| package["manifest_path"].as_str())
        .filter_map(|manifest| Path::new(manifest).parent())
        .map(|root| root.join(CONFIG_FILE))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    match candidates.as_slice() {
        [path] => path.canonicalize().map_err(Into::into),
        [] => bail!(
            "no rspyts.toml found in the Cargo workspace; run from an application directory or pass `--config path/to/rspyts.toml`"
        ),
        paths => bail!(
            "multiple RSPYTS applications found: {paths:?}; select one with `--config path/to/rspyts.toml`"
        ),
    }
}

fn ancestors(path: &Path) -> impl Iterator<Item = &Path> {
    path.ancestors()
}

fn quoted(value: &str) -> String {
    serde_json::to_string(value).expect("serializing a string cannot fail")
}

fn string_array(values: &[String]) -> String {
    if values.is_empty() {
        return "[]".to_owned();
    }
    let mut source = String::from("[\n");
    for value in values {
        source.push_str("    ");
        source.push_str(&quoted(value));
        source.push_str(",\n");
    }
    source.push(']');
    source
}

fn string_array_value<'a>(values: impl Iterator<Item = &'a str>) -> toml_edit::Item {
    let mut array = toml_edit::Array::new();
    for value in values {
        array.push(value);
    }
    toml_edit::value(array)
}

fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_updates_preserve_application_text_and_comments() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(CONFIG_FILE);
        let customized = CONFIG_TEMPLATE.replace(
            "# name = \"my-application\"",
            "# keep this comment\nname = \"demo\"",
        );
        let application_source = customized
            .split_once("[generated]")
            .expect("template has generated settings")
            .0;
        fs::write(&path, &customized).unwrap();
        let config = Config::read(&path).unwrap();
        assert!(config.application.gitignore);
        let rendered = config
            .render_generated(&GeneratedState {
                source_fingerprint: "abc".to_owned(),
                python: PythonGeneratedState {
                    files: vec!["src-py/demo/api.py".to_owned()],
                    native_modules: vec!["src-py/demo/native".to_owned()],
                },
                typescript: TypeScriptGeneratedState {
                    files: vec!["src-ts/demo/api.ts".to_owned()],
                },
            })
            .unwrap();

        assert!(rendered.contains("# keep this comment\nname = \"demo\""));
        assert!(rendered.contains("# additional_packages = [\"my-shared-api\"]"));
        assert!(rendered.contains("# python_package = \"my_application\""));
        assert!(rendered.contains("# typescript_package = \"my-application\""));
        assert!(rendered.contains("# gitignore = false"));
        assert_eq!(
            rendered
                .split_once("[generated]")
                .expect("rendered config has generated settings")
                .0,
            application_source
        );
        assert!(rendered.contains("[generated.python]"));
        assert!(rendered.contains("src-py/demo/api.py"));
        assert!(rendered.contains("[generated.typescript]"));
        assert!(rendered.contains("src-ts/demo/api.ts"));
        assert!(!rendered.contains("version ="));

        fs::write(
            &path,
            customized.replace("# gitignore = false", "gitignore = false"),
        )
        .unwrap();
        let disabled = Config::read(&path).unwrap();
        assert!(!disabled.application.gitignore);
        assert!(
            disabled
                .application_fingerprint_source()
                .unwrap()
                .contains("gitignore = false")
        );
    }
}
