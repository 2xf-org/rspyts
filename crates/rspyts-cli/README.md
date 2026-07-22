# rspyts-cli

`rspyts-cli` creates and maintains a Rust application with installable Python and TypeScript source projects beside it. The installed executable is named `rspyts`.

## Install

```console
cargo install rspyts-cli --locked
rustup target add wasm32-unknown-unknown
```

rspyts requires Rust 1.88 or newer. Generation invokes Cargo for the native and `wasm32-unknown-unknown` builds. It does not invoke Python, Node.js, npm, or the TypeScript compiler.

## Create an application

```console
rspyts init path/to/hello-rspyts --version 1.2.3
```

The destination must not exist. Its last path component becomes the Cargo package name and must contain lower-case ASCII letters, numbers, and single hyphens. It must start with a letter and cannot end with a hyphen. The version must be valid semantic versioning and defaults to `0.1.0`.

The same name is used for the npm package. Each hyphen becomes `_` in the default Python import package:

| Project name | Python import | npm import |
| --- | --- | --- |
| `hello-rspyts` | `hello_rspyts` | `hello-rspyts` |

Initialization writes the Rust greeting example, `rspyts.toml`, a Python source project under `src-py`, and a TypeScript source project under `src-ts`. The same version is written to `Cargo.toml`, `src-py/pyproject.toml`, and `src-ts/package.json`. Manifests and root language entrypoints are scaffolded once.

## Configure an application

`rspyts.toml` is the authority for application selection and generated-file ownership. It lives next to the application’s `Cargo.toml`.

The editable portion is `[application]`:

```toml
[application]
# Override the public application name.
# Defaults to the adjacent Cargo package name.
# name = "my-application"

# Link other library packages from this Cargo workspace.
# The adjacent package is always linked.
# additional_packages = ["my-shared-api"]

# Override the Python import package.
# Defaults to the application name with each hyphen changed to `_`.
# python_package = "my_application"

# Override the npm package name.
# Defaults to the application name and must match src-ts/package.json.
# typescript_package = "my-application"

# Maintain nested ignore files for generated paths.
# Defaults to true.
# gitignore = false
```

All five settings are optional.

| Setting | Default | Effect |
| --- | --- | --- |
| `name` | Adjacent Cargo package name | Selects the public application identity |
| `additional_packages` | `[]` | Adds named workspace library packages to the same application |
| `python_package` | Application name with `-` changed to `_` | Selects the generated Python import path |
| `typescript_package` | Application name | Selects the generated TypeScript source directory and must match the npm manifest |
| `gitignore` | `true` | Creates owned ignore files containing every generated path |

The remaining tables are maintained by rspyts:

```toml
[generated]
source_fingerprint = "..."

[generated.python]
files = ["..."]
native_modules = ["..."]

[generated.typescript]
files = ["..."]
```

`files` contains project-relative paths that a future build may overwrite or remove. `native_modules` stores Python extension basenames because the platform chooses `.abi3.so` or `.pyd`. The source fingerprint covers Rust source files, Cargo manifests and locks, the active application settings, and the rspyts generator version.

Do not hand-edit the generated tables. Change the Rust contract or `[application]`, then run `rspyts build`.

## Select an application

`build`, `watch`, and `check` use the same discovery order:

1. Use the file or directory passed with `--config`.
2. Otherwise, use the nearest `rspyts.toml` in the current directory or an ancestor.
3. Otherwise, inspect the enclosing Cargo workspace and use its sole rspyts application.

If a workspace contains more than one application, select one explicitly:

```console
rspyts build --config crates/server/rspyts.toml
```

Passing the application directory is also valid:

```console
rspyts build --config crates/server
```

## Build

```console
rspyts build
```

A build:

1. Resolves the adjacent Cargo package and every `additional_packages` entry.
2. Compiles an internal bridge as a native library and reads its contract.
3. Validates names, namespaces, model references, and the user-owned package manifests.
4. Compiles the same application for WebAssembly.
5. Renders Python, TypeScript, native, and WebAssembly files in a temporary directory.
6. Publishes the Python extension under `src-py/<package>/native` and the Wasm module directly under `src-ts/build/<package>/native`.
7. Publishes generated Python, TypeScript, and wasm-bindgen source files into their owned paths.

The JSON report identifies both source roots and public package names:

```json
{
  "status": "ok",
  "pythonSource": "/path/to/hello-rspyts/src-py",
  "typescriptSource": "/path/to/hello-rspyts/src-ts",
  "pythonPackage": "hello_rspyts",
  "typescriptPackage": "hello-rspyts"
}
```

Both language manifests must declare the same version as the adjacent Cargo package. The Python manifest must declare `pydantic`; if the contract uses `#[rspyts(buffer)]`, it must also declare `numpy`. The npm manifest’s `name` must match the configured TypeScript package. rspyts reports these mismatches but never rewrites either manifest.

## Preserve authored files

Only paths from the generated tables are mutable. During publication, rspyts:

- overwrites generated paths that remain present;
- removes generated paths no longer produced by the contract;
- preserves every unlisted file;
- rejects an unlisted destination collision before mutation;
- restores the previous generated set and ownership metadata if publication fails.

Text files produced by rspyts begin with an explicit generated-file banner:

```text
=============================================================================
AUTO-GENERATED BY rspyts - DO NOT EDIT.

This file is overwritten by `rspyts build`.
=============================================================================
```

Comment syntax differs by language. Native extensions and Wasm files are binary and are identified by `rspyts.toml` instead.

The default nested `.gitignore` files list exact generated paths. Set `gitignore = false` and rebuild to remove those ignore files and include generated files in version control.

## Watch

```console
rspyts watch
```

`watch` builds immediately, then monitors Rust files, Cargo manifests and locks, and `[application]`. A failed rebuild is printed to standard error, and the watcher continues so the next source change can recover.

## Check

```console
rspyts check
```

`check` generates the expected application in a temporary directory and compares it with the recorded generated state. It fails when an owned file is modified, missing, unexpectedly retained, or derived from an old source fingerprint. Extra user-owned files do not affect the result.

Use this command in CI after `build` when generated files are produced during the job, or directly when generated files are included in version control.

## Package the language projects

rspyts generates source projects; their ecosystem tools create release archives:

```console
python -m pip wheel --no-deps ./src-py

npm --prefix src-ts install
npm --prefix src-ts run check
npm --prefix src-ts run build
npm pack ./src-ts
```

The Python project uses PDM’s build backend through `pyproject.toml` to package the prebuilt native module without a setup script or compiler step. rspyts places the Wasm asset in its final npm output directory, so the npm build script only invokes `tsc`.

See the [repository README](https://github.com/2xf-org/rspyts) for a first project and the [example guide](https://github.com/2xf-org/rspyts/tree/main/example) for a self-contained application.
