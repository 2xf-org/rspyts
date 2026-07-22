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

The new project contains an API crate and a binding crate. It also contains
one client for each language. Define the API in the API crate:

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

Link the API crate in the binding crate:

```rust
rspyts::application!(hello_rspyts_api);
```

The build writes both packages to `crates/bindings/dist`. To keep generated
artifacts outside the binding crate, pass an explicit output directory:

```sh
rspyts build --output dist/packages
rspyts check --output dist/packages
```

Relative output paths resolve from the current working directory. The CLI
refuses to replace the binding project or Cargo workspace root.

Install and use the Python package:

```sh
python -m pip install ./crates/bindings/dist/python
```

```python
from hello_rspyts.api import Greeting, greet

result: Greeting = greet("Ada")
print(result.message)
```

Install and use the TypeScript package:

```sh
npm install ./crates/bindings/dist/typescript
```

```typescript
import { greet, type Greeting } from "hello-rspyts/api";

const result: Greeting = greet("Ada");
console.log(result.message);
```

The TypeScript package loads its WebAssembly file when the program imports
the package.

Rust string enums are Python `StrEnum` classes. TypeScript emits both a string
union and a same-named frozen runtime value, so clients can use an enum without
duplicating its wire strings:

```typescript
import { RunMode } from "hello-rspyts/api";

const mode: RunMode = RunMode.Safe;
```

## Package names

rspyts makes package paths from Cargo package names and Rust module paths. You
do not add namespace settings.

For an `example` binding package and this Rust declaration:

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

* Remove the longest shared hyphen-separated prefix from the application
  Cargo package names.
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
and identifies their source. Python exports cannot use `__all__`, `__dir__`,
`__getattr__`, or the generated `_rspyts_models_` prefix.

## Requirements

The generated Python package requires CPython 3.11 or later. Its installer
adds Pydantic 2. It adds NumPy 2 when the API uses a numeric buffer.
Generated namespace packages resolve their public models and API members
lazily, while matching `__init__.pyi` files preserve static type checking and
editor completion for the documented imports.

The generated TypeScript package has no runtime npm dependencies. Use an ES
module runtime with WebAssembly. The runtime must support top-level `await`
and `import.meta.url`.

`rspyts build` does not run Python, Node.js, npm, or a TypeScript compiler.

## Commands

* `rspyts init <path>` creates a project and both clients.
* `rspyts build` builds both packages.
* `rspyts watch` rebuilds after a Rust or Cargo file changes.
* `rspyts check` checks that generated files match the Rust source.

Use `--manifest-path path/to/Cargo.toml` when a workspace has more than one
binding crate. `build`, `watch`, and `check` also accept `--output path`.

## Example

The [`example`](example/) directory contains a Rust dice API and working
clients for Python and TypeScript. It shows module paths with more than three
levels and repeated model names. It also sends a type across a module
boundary.

Licensed under [MIT](LICENSE).
