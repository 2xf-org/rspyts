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

rspyts builds one Rust API as typed packages for Python and TypeScript. In Python, it generates a native extension and Pydantic models, and in Typescript it generates a WebAssembly package and type declarations. Built as means to keep backend and frontend code fully synchronized via Rust.

## Installing

rspyts requires Rust 1.88 or newer.

```console
cargo install rspyts-cli --locked
rustup target add wasm32-unknown-unknown
```

## Usage

```console
rspyts init example --version 0.1.0 && cd example
rspyts build
```

`init` creates a working Rust `greet` example and gives Cargo, Python, and npm the same version. `build` compiles it for Python and WebAssembly, then writes the generated package files beside the Rust source.

```text
example/
├── .gitignore
├── Cargo.toml
├── rspyts.toml
├── src/
│   └── lib.rs
├── src-py/
│   ├── .gitignore
│   ├── pyproject.toml
│   └── example/
│       ├── __init__.py
│       ├── api.py
│       ├── models.py
│       ├── runtime.py
│       ├── py.typed
│       └── native/
│           ├── __init__.py
│           └── native.abi3.so # native.pyd on Windows
└── src-ts/
    ├── .gitignore
    ├── package.json
    ├── tsconfig.json
    ├── example/
    │   ├── index.ts
    │   ├── api.ts
    │   ├── models.ts
    │   ├── runtime.ts
    │   └── native/
    │       ├── native.js
    │       └── native.d.ts
    └── build/
        └── example/
            └── native/
                └── native_bg.wasm
```

### Generated Code

rspyts replaces only the paths recorded in `rspyts.toml`. Generated text files carry an overwrite warning, while native and Wasm files are tracked by their paths. Package manifests, root entrypoints, and unlisted files remain under your control.

#### Python

Python models use Pydantic, and exported Rust functions call the compiled extension in `example/native`. The source project uses only `pyproject.toml`; its build backend packages the existing native artifact without compiling or copying it. Numeric `buffer` boundaries use NumPy arrays, while ordinary `Vec<T>` values become Python lists.

```console
python -m pip install ./src-py
```

```python
from example import greet

print(greet("Ada").message)
```

#### TypeScript

The TypeScript package contains strict ESM source and declarations. Rust is compiled to WebAssembly (Wasm), which rspyts writes directly to `src-ts/build/example/native`; the generated `native.js` loads it and converts values at the boundary. The package has no runtime npm dependencies, and `npm run build` only runs the TypeScript compiler.

```console
npm --prefix src-ts install
npm --prefix src-ts run build
```

Install the built source project from your client:

```console
npm install ../example/src-ts
```

Then import the package by name:

```typescript
import { greet } from "example";

console.log(greet("Ada").message);
```

### Custom Code

Add Python files inside `src-py/example` and TypeScript files inside `src-ts/example`. rspyts preserves any file that is not listed under `[generated.python]` or `[generated.typescript]`.

```python
# src-py/example/convenience/__init__.py
from ..models import Greeting


def shout(greeting: Greeting) -> str:
    return greeting.message.upper()
```

```typescript
// src-ts/example/convenience/index.ts
import type { Greeting } from "../models.js";

export function shout(greeting: Greeting): string {
  return greeting.message.toUpperCase();
}
```

Import them directly with `from example.convenience import shout` and `import { shout } from "example/convenience"`, or edit the user-owned `example/__init__.py` and `example/index.ts` entrypoints to re-export them.

### Config

`rspyts.toml` sits next to `Cargo.toml`. Edit `[application]` to change package names, include other Cargo workspace packages, or commit generated files. rspyts updates the `[generated]` tables after each build; do not edit those tables by hand. Files you add to `src-py` or `src-ts` are preserved unless their paths appear in a generated list. Package versions do not live in `rspyts.toml`: `init --version` writes the initial value into all three package manifests, and later builds require those values to stay aligned.

```toml
# Edit [application]. rspyts updates only the [generated] tables.

[application]
# Override the public application name.
# Defaults to the adjacent Cargo package name.
# name = "example"

# Link other library packages from the same Cargo workspace.
# The adjacent package is always linked.
# additional_packages = ["example-models"]

# Override the Python import package.
# Defaults to the application name with each `-` changed to `_`.
# python_package = "example"

# Override the npm package name.
# Defaults to the application name and must match src-ts/package.json.
# typescript_package = "example"

# Generate src-py/.gitignore and src-ts/.gitignore for generated files.
# Defaults to true. Set false to commit generated files.
# gitignore = false

[generated]
# Fingerprint of the Rust and Cargo sources plus [application].
source_fingerprint = "..."

[generated.python]
# Python files rspyts may overwrite or remove.
files = [
    "src-py/.gitignore",
    "src-py/example/api.py",
    "src-py/example/models.py",
    "src-py/example/native/__init__.py",
    "src-py/example/py.typed",
    "src-py/example/runtime.py",
]

# Extension basename; the platform supplies .abi3.so or .pyd.
native_modules = ["src-py/example/native/native"]

[generated.typescript]
# TypeScript, wasm-bindgen, and Wasm files rspyts may overwrite or remove.
files = [
    "src-ts/.gitignore",
    "src-ts/build/example/native/native_bg.wasm",
    "src-ts/example/api.ts",
    "src-ts/example/models.ts",
    "src-ts/example/native/native.d.ts",
    "src-ts/example/native/native.js",
    "src-ts/example/runtime.ts",
]
```
