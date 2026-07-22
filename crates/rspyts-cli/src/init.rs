use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use semver::Version;
use serde::Serialize;

use crate::config::CONFIG_TEMPLATE;
use crate::output::{generated_gitignore, write};

#[derive(Debug, Serialize)]
pub(super) struct InitReport {
    status: &'static str,
    project: PathBuf,
}

pub(super) fn create(path: &Path, version: &Version) -> Result<InitReport> {
    if path.exists() {
        bail!(
            "{} already exists; rspyts will not overwrite it",
            path.display()
        );
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .context("the project path must end with a UTF-8 name")?;
    validate_name(name)?;
    let rust_name = name.replace('-', "_");
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let temporary = tempfile::Builder::new()
        .prefix(".rspyts-init-")
        .tempdir_in(parent)?;
    let root = temporary.path();
    let version = version.to_string();

    write(&root.join("Cargo.toml"), &template(CARGO, name, &version))?;
    write(&root.join(".gitignore"), GITIGNORE)?;
    write(&root.join("rspyts.toml"), CONFIG_TEMPLATE)?;
    write(&root.join("src/lib.rs"), RUST_LIB)?;
    write(
        &root.join("src-py/pyproject.toml"),
        &template(PYPROJECT, name, &version).replace("__PYTHON_PACKAGE__", &rust_name),
    )?;
    write(
        &root.join(format!("src-py/{rust_name}/__init__.py")),
        PYTHON_INIT,
    )?;
    write(
        &root.join("src-py/.gitignore"),
        &generated_gitignore(&[
            format!("{rust_name}/api.py"),
            format!("{rust_name}/models.py"),
            format!("{rust_name}/native/__init__.py"),
            format!("{rust_name}/native/native.abi3.so"),
            format!("{rust_name}/native/native.pyd"),
            format!("{rust_name}/py.typed"),
            format!("{rust_name}/runtime.py"),
        ]),
    )?;
    write(
        &root.join("src-ts/package.json"),
        &template(TYPESCRIPT_PACKAGE, name, &version),
    )?;
    write(
        &root.join(format!("src-ts/{name}/index.ts")),
        TYPESCRIPT_INDEX,
    )?;
    write(
        &root.join("src-ts/tsconfig.json"),
        &TYPESCRIPT_CONFIG.replace("__PROJECT__", name),
    )?;
    write(
        &root.join("src-ts/.gitignore"),
        &generated_gitignore(&[
            format!("{name}/api.ts"),
            format!("{name}/models.ts"),
            format!("{name}/native/native.d.ts"),
            format!("{name}/native/native.js"),
            format!("{name}/runtime.ts"),
            format!("build/{name}/native/native_bg.wasm"),
        ]),
    )?;

    fs::rename(root, path)
        .with_context(|| format!("failed to create project at {}", path.display()))?;
    Ok(InitReport {
        status: "ok",
        project: path.to_path_buf(),
    })
}

fn template(source: &str, project: &str, version: &str) -> String {
    source
        .replace("__PROJECT__", project)
        .replace("__VERSION__", version)
}

fn validate_name(name: &str) -> Result<()> {
    let mut characters = name.chars();
    let starts_correctly = characters
        .next()
        .is_some_and(|character| character.is_ascii_lowercase());
    if !starts_correctly
        || !characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        || name.ends_with('-')
        || name.contains("--")
    {
        bail!("project name `{name}` must use lower-case letters, numbers, and single hyphens");
    }
    Ok(())
}

const CARGO: &str = r#"[package]
name = "__PROJECT__"
version = "__VERSION__"
edition = "2024"
rust-version = "1.88"

[dependencies]
rspyts = "3"
serde = { version = "1", features = ["derive"] }
"#;

const GITIGNORE: &str = r"/target/
/.rspyts-build.lock
**/.venv/
**/__pycache__/
**/node_modules/
**/build/
";

const RUST_LIB: &str = r#"use rspyts::Model;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Model)]
pub struct Greeting {
    pub message: String,
}

#[rspyts::export]
pub fn greet(name: String) -> Greeting {
    Greeting {
        message: format!("Hello, {name}!"),
    }
}
"#;

const PYPROJECT: &str = r#"# User-owned package configuration; rspyts never modifies this file.
# Keep project.version aligned with Cargo.toml and add dependencies or tooling here.

[project]
name = "__PROJECT__"
version = "__VERSION__"
requires-python = ">=3.11"
dependencies = ["pydantic>=2,<3"]

[build-system]
requires = ["pdm-backend>=2.4,<3"]
build-backend = "pdm.backend"

[tool.pdm.build]
includes = ["__PYTHON_PACKAGE__"]
is-purelib = false
"#;

const PYTHON_INIT: &str = "from .models import *\nfrom .models import __all__ as __models__\n\nfrom .api import *\nfrom .api import __all__ as __api__\n\n__all__ = [*__models__, *__api__]\n";

const TYPESCRIPT_PACKAGE: &str = r#"{
  "//": "User-owned package configuration; rspyts never modifies this file. Keep version aligned with Cargo.toml and add dependencies here.",
  "name": "__PROJECT__",
  "version": "__VERSION__",
  "type": "module",
  "sideEffects": true,
  "scripts": {
    "build": "tsc",
    "check": "tsc --noEmit"
  },
  "exports": {
    ".": {
      "types": "./build/__PROJECT__/index.d.ts",
      "import": "./build/__PROJECT__/index.js"
    },
    "./api": {
      "types": "./build/__PROJECT__/api.d.ts",
      "import": "./build/__PROJECT__/api.js"
    },
    "./models": {
      "types": "./build/__PROJECT__/models.d.ts",
      "import": "./build/__PROJECT__/models.js"
    },
    "./*": {
      "types": "./build/__PROJECT__/*/index.d.ts",
      "import": "./build/__PROJECT__/*/index.js"
    }
  },
  "files": ["build"],
  "devDependencies": {
    "typescript": "^5.9.0"
  }
}
"#;

const TYPESCRIPT_INDEX: &str = "export * from \"./models.js\";\nexport * from \"./api.js\";\n";

const TYPESCRIPT_CONFIG: &str = r#"{
  // User-owned compiler configuration; rspyts never modifies this file.
  "compilerOptions": {
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "outDir": "build",
    "rootDir": ".",
    "allowJs": true,
    "checkJs": false,
    "declaration": true,
    "lib": ["ES2022", "DOM", "ESNext.Disposable"],
    "strict": true,
    "target": "ES2022"
  },
  "include": ["__PROJECT__/**/*"]
}
"#;
