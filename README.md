# 🦀 rspyts

<p align="center">
  <a href="https://github.com/2xf-org/rspyts/actions/workflows/validation.yml">
    <img src="https://github.com/2xf-org/rspyts/actions/workflows/validation.yml/badge.svg" alt="Validation">
  </a>
  <a href="LICENSE">
    <img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="MIT license">
  </a>
</p>

rspyts compiles one Rust application API into installable Python and
TypeScript packages. Python runs the API through PyO3. TypeScript runs the
same API through WebAssembly.

One build produces Pydantic models, Python type information, TypeScript
declarations, and the compiled Rust library for each host. rspyts has no
configuration file and no separate Python or TypeScript mode.

## Install

rspyts needs two build tools:

* Rust 1.88 or later.
* The `wasm32-unknown-unknown` Rust target.

Install rspyts and the WebAssembly target:

```sh
cargo install rspyts-cli --version '=1.0.0' --locked
rustup target add wasm32-unknown-unknown
```

The rspyts binary contains the matching WebAssembly binding generator.
`rspyts build` does not run Python, Node.js, npm, or a TypeScript compiler.

## Generated package requirements

The generated Python package requires CPython 3.11 or later. Its installer
adds Pydantic 2. It adds NumPy 2 only when the Rust API uses a numeric buffer.
You do not add PyO3 to the Python project.

The generated TypeScript package is an ES module with WebAssembly. It has no
runtime npm dependencies. Use a JavaScript runtime that supports ES modules,
top-level `await`, `import.meta.url`, and WebAssembly. You need Node.js and a
TypeScript compiler only if your client development workflow uses them.

## Using

Create an application and build both packages:

```sh
rspyts init hello-rspyts
cd hello-rspyts
rspyts build
```

`rspyts init` creates a Cargo workspace. It contains one API crate and one
binding crate, plus small clients for the generated packages.

A typical API crate needs only rspyts and Serde:

```toml
[dependencies]
rspyts = "1"
serde = { version = "1", features = ["derive"] }
```

The binding crate needs rspyts and the application crates that it links. Do
not add `rspyts-macros`, PyO3, or `wasm-bindgen` directly. rspyts owns those
implementation dependencies. Add `thiserror`, `chrono`, or `serde_json` only
if your Rust source uses them directly.

Write the public API in a normal Rust crate:

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

The binding crate links the application's Rust crates into one library:

```rust
rspyts::application!(hello_rspyts_api);
```

`rspyts build` writes both packages beside the binding crate:

```text
crates/bindings/dist/
├── python/
└── typescript/
```

Install and use the Python package:

```sh
python -m pip install ./crates/bindings/dist/python
```

```python
from hello_rspyts import Greeting, greet

result: Greeting = greet("Ada")
print(result.message)
# Hello, Ada!
```

Install and use the TypeScript package:

```sh
npm install ./crates/bindings/dist/typescript
```

```typescript
import { greet, type Greeting } from "hello-rspyts";

const result: Greeting = greet("Ada");
console.log(result.message);
// Hello, Ada!
```

The TypeScript package loads its WebAssembly module when you import it.

## Commands

* `rspyts init <path>` creates the Cargo workspace and both clients.
* `rspyts build` builds both packages.
* `rspyts watch` rebuilds when a Rust or Cargo file changes.
* `rspyts check` fails when `dist` does not match the Rust source.

Pass `--manifest-path path/to/Cargo.toml` when the workspace contains more
than one binding crate.

## Example

The [`example`](example/) directory contains a Rust dice API, one binding
crate, and clients in both languages.

Licensed under [MIT](LICENSE).
