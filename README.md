# 🌉 rspyts

<p align="center">
  <a href="https://crates.io/crates/rspyts">
    <img src="https://img.shields.io/crates/v/rspyts.svg" alt="Crates.io">
  </a>
  <a href="https://github.com/2xf-org/rspyts/actions/workflows/validation.yml">
    <img src="https://github.com/2xf-org/rspyts/actions/workflows/validation.yml/badge.svg" alt="Validation">
  </a>
  <a href="https://docs.rs/rspyts">
    <img src="https://docs.rs/rspyts/badge.svg" alt="Documentation">
  </a>
  <a href="LICENSE">
    <img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="MIT license">
  </a>
</p>

rspyts builds one Rust API as typed packages for Python and TypeScript. It
generates Pydantic models and type declarations. Each application uses one
Python native extension and one WebAssembly file.

## Installing

Install rspyts and the WebAssembly target:

```sh
cargo install rspyts-cli --locked
rustup target add wasm32-unknown-unknown
```

rspyts requires Rust 1.88 or later.

## Using

Create a project and build both packages:

```sh
rspyts init hello-rspyts
cd hello-rspyts
rspyts build
```

The new project contains one Rust package with the greeting example and
persistent `src-py` and `src-ts` package projects. Define the API normally in
the Rust package:

```rust
use rspyts::Model;
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
```

The build updates RSPYTS-owned files inside the package source projects:

```text
hello-rspyts/
├── src/
├── src-py/hello_rspyts/
├── src-ts/hello-rspyts/
└── rspyts.toml
```

Add ordinary Python and TypeScript files alongside the generated `api`,
`models`, and `runtime` modules. Subsequent builds preserve every user-owned
file and only replace files listed in `rspyts.toml` beside `Cargo.toml`.
Generated text files carry an explicit do-not-edit header.

By default, RSPYTS also owns `src-py/.gitignore` and `src-ts/.gitignore` and
updates them with exact paths for generated files. To allow generated outputs
to be committed instead, opt out in `rspyts.toml`:

```toml
[application]
gitignore = false
```

The nested ignore files never include `pyproject.toml`, `package.json`,
`tsconfig.json`, root entrypoints, or other authored files.

Install and use the Python package:

```sh
python -m pip install ./src-py
```

```python
from hello_rspyts import Greeting, greet

result: Greeting = greet("Ada")
print(result.message)
```

Install and use the TypeScript package:

```sh
npm --prefix src-ts install
npm --prefix src-ts run build
npm install ./src-ts
```

```typescript
import { greet, type Greeting } from "hello-rspyts";

const result: Greeting = greet("Ada");
console.log(result.message);
```

The TypeScript package loads its WebAssembly file when the program imports
the package.

Rust string enums are Python `StrEnum` classes. TypeScript emits both a string
union and a same-named frozen runtime value, so clients can use an enum without
duplicating its wire strings:

```typescript
import { RunMode } from "hello-rspyts";

const mode: RunMode = RunMode.Safe;
```

## Package names

rspyts makes package paths from Cargo package names and Rust module paths. You
do not add namespace settings.

For an `example` application and this Rust declaration:

```text
Cargo package: example-catalog
Rust module:   inventory::shelf::position
Type:          Position
```

Use these imports:

```python
from example.catalog.inventory.shelf.position import Position
```

```typescript
import type { Position } from "example/catalog/inventory/shelf/position";
```

rspyts applies these rules:

* Remove the longest shared hyphen-separated prefix between the public
  application name and each linked Cargo package name.
* Add the Rust module path where the item is declared.
* Replace each Cargo hyphen with `_` in Python. Keep hyphens in TypeScript.
* Keep the declaration path when Rust re-exports an item.
* Export only the items declared in each module. Do not copy child items into
  a parent module.

Paths can have any depth. All paths share the same native extension and the
same WebAssembly file.

Model, error, function, resource, and constant names can repeat in different
modules. Public names must remain unique within one namespace. Python model
modules must not have a dependency cycle. `rspyts build` reports these errors
and identifies their source. Python exports cannot use `api`, `models`, the
generated `_rspyts_models_` prefix, or double-underscore module attributes
other than `__version__`.
A root export also cannot use `runtime` or the configured native module name,
and an export cannot have the same name as a direct child package. Namespace
paths cannot shadow the generated `api`, `models`, `runtime`, or native
modules or use reserved double-underscore package segments.

## Requirements

The generated Python package requires CPython 3.11 or later. Its initialized,
user-owned `pyproject.toml` includes Pydantic 2. APIs with numeric buffers must
also declare NumPy 2; `rspyts build` reports the missing dependency. Package
entrypoints re-export the explicit `__all__` values from generated model and
API modules.

The generated TypeScript package has no runtime npm dependencies. Its
initialized project compiles strict TypeScript and copies the WebAssembly
asset into the package build. Use an ES module runtime that supports top-level
`await` and `import.meta.url`.

`rspyts build` does not run Python, Node.js, npm, or a TypeScript compiler.

## Commands

* `rspyts init <path>` creates Rust, Python, and TypeScript source projects.
* `rspyts build [--config <path>]` regenerates RSPYTS-owned sources and binaries.
* `rspyts watch [--config <path>]` rebuilds after Rust or Cargo files change.
* `rspyts check [--config <path>]` checks generated files against the Rust source.

Commands use the nearest `rspyts.toml`. From elsewhere in a workspace, they
select its only RSPYTS application or require `--config path/to/rspyts.toml`
when more than one exists. The adjacent Cargo package is always linked; list
other workspace packages in `application.additional_packages`.

## Example

The [`example`](example/) directory contains a Rust dice API and working
clients for Python and TypeScript. It shows module paths with more than three
levels and repeated model names. It also sends a type across a module
boundary.

## Development

The repository uses integration tests at public Rust, CLI, Python, and
TypeScript boundaries. See [`TESTING.md`](TESTING.md) for the test layout and
commands.

Licensed under [MIT](LICENSE).
