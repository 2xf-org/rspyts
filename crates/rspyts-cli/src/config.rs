//! `rspyts.toml` parsing and path resolution (codegen.md §1).
//!
//! All relative paths in the file resolve against the file's own
//! directory. Unknown keys are rejected so typos surface as errors
//! instead of silently disabling an emitter.

use crate::build::ContractOptions;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Raw deserialization target mirroring the TOML layout exactly.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(rename = "crate", default)]
    krate: RawCrate,
    python: Option<RawPython>,
    typescript: Option<RawTypescript>,
    schema: Option<RawSchema>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCrate {
    #[serde(default = "default_crate_path")]
    path: String,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default, rename = "no-default-features")]
    no_default_features: bool,
}

impl Default for RawCrate {
    fn default() -> Self {
        Self {
            path: default_crate_path(),
            features: Vec::new(),
            no_default_features: false,
        }
    }
}

fn default_crate_path() -> String {
    ".".to_string()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPython {
    out: String,
    #[serde(default)]
    imports: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTypescript {
    out: String,
    #[serde(default)]
    imports: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSchema {
    out: String,
}

/// A parsed configuration with every path already resolved to an
/// absolute path. Disabled or omitted sections are `None`.
#[derive(Debug)]
pub struct Config {
    /// Directory containing the bridged crate's `Cargo.toml`.
    pub crate_dir: PathBuf,
    pub python: Option<PythonConfig>,
    pub typescript: Option<TypescriptConfig>,
    pub schema: Option<SchemaConfig>,
    /// Cargo feature selection that defines the compiled contract.
    pub contract: ContractOptions,
}

#[derive(Debug)]
pub struct PythonConfig {
    /// The wholly-owned `_generated` package directory.
    pub out: PathBuf,
    /// Origin crate name → Python import path. Bridged types whose
    /// origin appears here are imported from that package instead of
    /// re-emitted locally (codegen.md §9).
    pub imports: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct TypescriptConfig {
    pub out: PathBuf,
    /// Origin crate name → TypeScript module specifier. Bridged types
    /// whose origin appears here are imported from that module instead
    /// of re-emitted locally (codegen.md §9).
    pub imports: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct SchemaConfig {
    pub out: PathBuf,
}

/// Load and resolve the configuration at `path`.
pub fn load(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path).with_context(|| {
        format!(
            "cannot read config `{}` (run `rspyts init` to create one)",
            path.display()
        )
    })?;
    let raw: RawConfig =
        toml::from_str(&text).with_context(|| format!("invalid config `{}`", path.display()))?;
    let base = std::path::absolute(path)
        .with_context(|| format!("cannot make config path `{}` absolute", path.display()))?;
    let base = base
        .parent()
        .context("config path has no parent directory")?;
    resolve(raw, base)
}

fn resolve(raw: RawConfig, base: &Path) -> Result<Config> {
    let join = |p: &str| -> PathBuf {
        let p = Path::new(p);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            base.join(p)
        }
    };
    let contract = ContractOptions::new(raw.krate.features, raw.krate.no_default_features)?;
    Ok(Config {
        crate_dir: join(&raw.krate.path),
        python: raw.python.map(|s| PythonConfig {
            out: join(&s.out),
            imports: s.imports,
        }),
        typescript: raw.typescript.map(|s| TypescriptConfig {
            out: join(&s.out),
            imports: s.imports,
        }),
        schema: raw.schema.map(|s| SchemaConfig { out: join(&s.out) }),
        contract,
    })
}

/// The commented starter configuration written by `rspyts init`.
pub const INIT_TEMPLATE: &str = r#"# rspyts configuration. All relative paths resolve against this file's
# own directory. See docs/design/codegen.md in the rspyts repository.

[crate]
# Directory containing the bridged crate's Cargo.toml.
path = "."
# Cargo feature selection is part of the generated contract.
features = []
no-default-features = false

[python]
out = "../python/src/my_package/_generated"
# `rspyts build` stages the host cdylib in `<out>/lib` by default.

# Bridged types defined in a dependency crate can be imported from that
# crate's own generated package instead of re-emitted here:
# [python.imports]
# "other-crate" = "other_package._generated"

[typescript]
out = "../typescript/src/generated"

# Same idea for TypeScript, mapping to a module specifier:
# [typescript.imports]
# "other-crate" = "@scope/other/generated"

[schema]
out = "../schema"
"#;

/// Write the starter config into `dir`, refusing to overwrite.
pub fn init(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("cannot create directory `{}`", dir.display()))?;
    let path = dir.join("rspyts.toml");
    ensure!(
        !path.exists(),
        "`{}` already exists; refusing to overwrite",
        path.display()
    );
    std::fs::write(&path, INIT_TEMPLATE)
        .with_context(|| format!("cannot write `{}`", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Config {
        let raw: RawConfig = toml::from_str(text).expect("config parses");
        resolve(raw, Path::new("/project")).expect("config resolves")
    }

    #[test]
    fn full_config_resolves_relative_paths() {
        let cfg = parse(
            r#"
            [crate]
            path = "rust"
            features = ["serde", "fast-path"]
            no-default-features = true

            [python]
            out = "python/_generated"

            [typescript]
            out = "ts/generated"

            [schema]
            out = "schema"
            "#,
        );
        assert_eq!(cfg.crate_dir, Path::new("/project/rust"));
        assert_eq!(cfg.contract.features, vec!["serde", "fast-path"]);
        assert!(cfg.contract.no_default_features);
        assert_eq!(
            cfg.python.as_ref().unwrap().out,
            Path::new("/project/python/_generated")
        );
        assert_eq!(
            cfg.typescript.unwrap().out,
            Path::new("/project/ts/generated")
        );
        assert_eq!(cfg.schema.unwrap().out, Path::new("/project/schema"));
    }

    #[test]
    fn imports_tables_round_trip() {
        let cfg = parse(
            r#"
            [python]
            out = "py"

            [python.imports]
            "shared-types" = "shared_types.generated"
            "other-crate" = "other_package._generated"

            [typescript]
            out = "ts"

            [typescript.imports]
            "shared-types" = "shared-types-example"
            "#,
        );
        let py = cfg.python.unwrap();
        assert_eq!(
            py.imports.get("shared-types").map(String::as_str),
            Some("shared_types.generated")
        );
        assert_eq!(
            py.imports.get("other-crate").map(String::as_str),
            Some("other_package._generated")
        );
        let ts = cfg.typescript.unwrap();
        assert_eq!(
            ts.imports.get("shared-types").map(String::as_str),
            Some("shared-types-example")
        );
    }

    #[test]
    fn imports_default_to_empty() {
        let cfg = parse("[python]\nout = \"py\"\n\n[typescript]\nout = \"ts\"\n");
        let python = cfg.python.unwrap();
        assert!(python.imports.is_empty());
        assert!(cfg.typescript.unwrap().imports.is_empty());
    }

    #[test]
    fn sections_default_to_absent_and_crate_path_defaults_to_dot() {
        let cfg = parse("");
        assert_eq!(cfg.crate_dir, Path::new("/project/."));
        assert!(cfg.python.is_none());
        assert!(cfg.typescript.is_none());
        assert!(cfg.schema.is_none());
        assert_eq!(cfg.contract, ContractOptions::default());
    }

    #[test]
    fn removed_output_knobs_are_rejected() {
        for text in [
            "[python]\nout = \"x\"\nenabled = false\n",
            "[python]\nout = \"x\"\nlibrary_search = [\"fallback\"]\n",
            "[python]\nout = \"x\"\nexclude = [\"foo\"]\n",
            "[typescript]\nout = \"x\"\nenabled = false\n",
            "[typescript]\nout = \"x\"\nexclude = [\"foo\"]\n",
            "[schema]\nout = \"x\"\nenabled = false\n",
            "[build]\nfeatures = []\n",
            "[crate]\nprofile = \"release\"\n",
            "[crate]\nlocked = true\n",
        ] {
            assert!(toml::from_str::<RawConfig>(text).is_err(), "{text}");
        }
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err = toml::from_str::<RawConfig>("[python]\nout = \"x\"\ntypo = 1\n").unwrap_err();
        assert!(
            err.to_string().contains("typo"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn invalid_contract_features_are_rejected() {
        let text = "[crate]\nfeatures = [\"bad feature\"]\n";
        let raw: RawConfig = toml::from_str(text).unwrap();
        assert!(resolve(raw, Path::new("/project")).is_err(), "{text}");
    }

    #[test]
    fn unknown_top_level_section_is_rejected() {
        assert!(toml::from_str::<RawConfig>("[pyhton]\nout = \"x\"\n").is_err());
    }

    #[test]
    fn init_template_parses() {
        let raw: RawConfig = toml::from_str(INIT_TEMPLATE).expect("template parses");
        let cfg = resolve(raw, Path::new("/p")).expect("template resolves");
        assert!(cfg.python.is_some() && cfg.typescript.is_some() && cfg.schema.is_some());
    }
}
