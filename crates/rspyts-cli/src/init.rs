use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::output::write;

#[derive(Debug, Serialize)]
pub(super) struct InitReport {
    status: &'static str,
    project: PathBuf,
}

pub(super) fn create(path: &Path) -> Result<InitReport> {
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
    let api_package = format!("{name}-api");
    let api_crate = format!("{rust_name}_api");
    let client_package = format!("{rust_name}_client");
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let temporary = tempfile::Builder::new()
        .prefix(".rspyts-init-")
        .tempdir_in(parent)?;
    let root = temporary.path();

    write(&root.join("Cargo.toml"), ROOT_CARGO)?;
    write(&root.join(".gitignore"), GITIGNORE)?;
    write(
        &root.join("crates/api/Cargo.toml"),
        &API_CARGO.replace("__API_PACKAGE__", &api_package),
    )?;
    write(&root.join("crates/api/src/lib.rs"), API_LIB)?;
    write(
        &root.join("crates/bindings/Cargo.toml"),
        &BINDINGS_CARGO
            .replace("__PROJECT__", name)
            .replace("__API_PACKAGE__", &api_package),
    )?;
    write(
        &root.join("crates/bindings/src/lib.rs"),
        &BINDINGS_LIB.replace("__API_CRATE__", &api_crate),
    )?;
    write(
        &root.join("clients/python/pyproject.toml"),
        &PYPROJECT
            .replace("__PROJECT__", name)
            .replace("__PYTHON_PACKAGE__", &rust_name)
            .replace("__CLIENT_PACKAGE__", &client_package),
    )?;
    write(
        &root.join(format!("clients/python/{client_package}/__init__.py")),
        &PYTHON_CLIENT.replace("__PYTHON_PACKAGE__", &rust_name),
    )?;
    write(
        &root.join("clients/python/tests/test_client.py"),
        &PYTHON_TEST.replace("__CLIENT_PACKAGE__", &client_package),
    )?;
    write(
        &root.join("clients/typescript/package.json"),
        &TYPESCRIPT_PACKAGE.replace("__PROJECT__", name),
    )?;
    write(
        &root.join("clients/typescript/src/index.ts"),
        &TYPESCRIPT_CLIENT.replace("__PROJECT__", name),
    )?;
    write(
        &root.join("clients/typescript/tsconfig.json"),
        TYPESCRIPT_CONFIG,
    )?;

    fs::rename(root, path)
        .with_context(|| format!("failed to create project at {}", path.display()))?;
    Ok(InitReport {
        status: "ok",
        project: path.to_path_buf(),
    })
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

const ROOT_CARGO: &str = r#"[workspace]
resolver = "3"
members = ["crates/api", "crates/bindings"]
default-members = ["crates/bindings"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.88"

[workspace.dependencies]
rspyts = "1"
serde = { version = "1", features = ["derive"] }
"#;

const GITIGNORE: &str = r"/target/
**/dist/
**/.venv/
**/__pycache__/
**/.pytest_cache/
**/node_modules/
**/build/
";

const API_CARGO: &str = r#"[package]
name = "__API_PACKAGE__"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
rspyts.workspace = true
serde.workspace = true
"#;

const API_LIB: &str = r#"use rspyts::Model;
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

const BINDINGS_CARGO: &str = r#"[package]
name = "__PROJECT__"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[lib]
crate-type = ["cdylib"]

[dependencies]
__API_PACKAGE__ = { path = "../api" }
rspyts.workspace = true
"#;

const BINDINGS_LIB: &str = "rspyts::application!(__API_CRATE__);\n";

const PYPROJECT: &str = r#"[project]
name = "__PROJECT__-client"
version = "0.1.0"
requires-python = ">=3.11"
dependencies = ["__PYTHON_PACKAGE__"]

[tool.uv.sources]
__PYTHON_PACKAGE__ = { path = "../../crates/bindings/dist/python" }

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.hatch.build.targets.wheel]
packages = ["__CLIENT_PACKAGE__"]

[dependency-groups]
dev = ["pytest>=8,<9"]
"#;

const PYTHON_CLIENT: &str = r#"from __PYTHON_PACKAGE__.api import Greeting, greet

__all__ = ["Greeting", "greet"]
"#;

const PYTHON_TEST: &str = r#"from __CLIENT_PACKAGE__ import greet


def test_greeting() -> None:
    assert greet("World").message == "Hello, World!"
"#;

const TYPESCRIPT_PACKAGE: &str = r#"{
  "name": "__PROJECT__-client",
  "private": true,
  "type": "module",
  "scripts": {
    "build": "tsc",
    "check": "tsc --noEmit",
    "start": "node build/index.js"
  },
  "dependencies": {
    "__PROJECT__": "file:../../crates/bindings/dist/typescript"
  },
  "devDependencies": {
    "typescript": "^5.9.0"
  }
}
"#;

const TYPESCRIPT_CLIENT: &str = r#"import { greet } from "__PROJECT__/api";

console.log(greet("World").message);
"#;

const TYPESCRIPT_CONFIG: &str = r#"{
  "compilerOptions": {
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "outDir": "build",
    "rootDir": "src",
    "strict": true,
    "target": "ES2022"
  },
  "include": ["src"]
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_the_complete_project() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("hello-world");

        create(&project).unwrap();

        for path in [
            "Cargo.toml",
            "crates/api/src/lib.rs",
            "crates/bindings/src/lib.rs",
            "clients/python/hello_world_client/__init__.py",
            "clients/python/tests/test_client.py",
            "clients/typescript/src/index.ts",
        ] {
            assert!(project.join(path).is_file(), "missing {path}");
        }
        let cargo = fs::read_to_string(project.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("rspyts = \"1\""));
        let python =
            fs::read_to_string(project.join("clients/python/hello_world_client/__init__.py"))
                .unwrap();
        assert!(python.contains("from hello_world.api import Greeting, greet"));
        let typescript =
            fs::read_to_string(project.join("clients/typescript/src/index.ts")).unwrap();
        assert!(typescript.contains("from \"hello-world/api\""));
        assert!(create(&project).is_err());
    }

    #[test]
    fn rejects_invalid_names() {
        let directory = tempfile::tempdir().unwrap();
        assert!(create(&directory.path().join("Hello_world")).is_err());
    }
}
