use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::config::CONFIG_TEMPLATE;
use crate::output::{generated_gitignore, write};

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
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let temporary = tempfile::Builder::new()
        .prefix(".rspyts-init-")
        .tempdir_in(parent)?;
    let root = temporary.path();

    write(
        &root.join("Cargo.toml"),
        &CARGO.replace("__PROJECT__", name),
    )?;
    write(&root.join(".gitignore"), GITIGNORE)?;
    write(&root.join("rspyts.toml"), CONFIG_TEMPLATE)?;
    write(&root.join("src/lib.rs"), RUST_LIB)?;
    write(
        &root.join("src-py/pyproject.toml"),
        &PYPROJECT.replace("__PROJECT__", name),
    )?;
    write(&root.join("src-py/setup.py"), PYTHON_SETUP)?;
    write(&root.join("src-py/setup.cfg"), PYTHON_SETUP_CONFIG)?;
    write(
        &root.join(format!("src-py/{rust_name}/__init__.py")),
        PYTHON_INIT,
    )?;
    write(
        &root.join("src-py/.gitignore"),
        &generated_gitignore(&[
            format!("{rust_name}/api.py"),
            format!("{rust_name}/models.py"),
            format!("{rust_name}/native.pyd"),
            format!("{rust_name}/native.so"),
            format!("{rust_name}/py.typed"),
            format!("{rust_name}/runtime.py"),
        ]),
    )?;
    write(
        &root.join("src-ts/package.json"),
        &TYPESCRIPT_PACKAGE.replace("__PROJECT__", name),
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
        &root.join("src-ts/scripts/copy-assets.mjs"),
        &TYPESCRIPT_COPY_ASSETS.replace("__PROJECT__", name),
    )?;
    write(
        &root.join("src-ts/.gitignore"),
        &generated_gitignore(&[
            format!("{name}/api.ts"),
            format!("{name}/models.ts"),
            format!("{name}/native.d.ts"),
            format!("{name}/native.js"),
            format!("{name}/native_bg.wasm"),
            format!("{name}/native_bg.wasm.d.ts"),
            format!("{name}/runtime.ts"),
        ]),
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

const CARGO: &str = r#"[package]
name = "__PROJECT__"
version = "0.1.0"
edition = "2024"
rust-version = "1.88"

[dependencies]
rspyts = "2"
serde = { version = "1", features = ["derive"] }
"#;

const GITIGNORE: &str = r"/target/
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

const PYPROJECT: &str = r#"[project]
name = "__PROJECT__"
version = "0.1.0"
requires-python = ">=3.11"
dependencies = ["pydantic>=2,<3"]

[build-system]
requires = ["setuptools>=77"]
build-backend = "setuptools.build_meta"

[tool.setuptools.packages.find]
where = ["."]

[tool.setuptools.package-data]
"*" = ["*.pyi", "*.so", "*.pyd", "py.typed"]
"#;

const PYTHON_SETUP: &str = "from setuptools import Distribution, setup\n\n\nclass BinaryDistribution(Distribution):\n    def has_ext_modules(self) -> bool:\n        return True\n\n\nsetup(distclass=BinaryDistribution)\n";

const PYTHON_SETUP_CONFIG: &str = "[bdist_wheel]\npy_limited_api = cp311\n";

const PYTHON_INIT: &str = "from .models import *\nfrom .models import __all__ as __models__\n\nfrom .api import *\nfrom .api import __all__ as __api__\n\n__all__ = [*__models__, *__api__]\n";

const TYPESCRIPT_PACKAGE: &str = r#"{
  "name": "__PROJECT__",
  "version": "0.1.0",
  "type": "module",
  "sideEffects": true,
  "scripts": {
    "build": "tsc && node scripts/copy-assets.mjs",
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

const TYPESCRIPT_COPY_ASSETS: &str = r#"import { cp, mkdir } from "node:fs/promises";

await mkdir("build/__PROJECT__", { recursive: true });
await cp("__PROJECT__/native_bg.wasm", "build/__PROJECT__/native_bg.wasm");
"#;
