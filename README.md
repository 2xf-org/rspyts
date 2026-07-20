# rspyts

rspyts builds one Rust application API for Python and TypeScript.

You write the API in Rust. rspyts generates these two packages:

- A Python package with Pydantic models and a PyO3 native extension.
- A TypeScript package with types and a WebAssembly module.

rspyts has one operation model. It always builds both packages. It does not
use a separate config file, package modes, contract locks, or generated
package dependencies.

## Requirements

- Rust 1.88 or later.
- Python 3.11 or later.
- `wasm32-unknown-unknown`.
- `wasm-bindgen-cli` 0.2.126.

Install the tools:

```sh
cargo install rspyts-cli --version '=1.0.0' --locked
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version '=0.2.126' --locked
```

## Project structure

Keep domain code in normal Rust crates. Add one small application crate that
links all exported crates.

```text
project/
├── Cargo.toml
├── domain/
│   ├── Cargo.toml
│   └── src/lib.rs
└── bindings/
    ├── Cargo.toml
    └── src/lib.rs
```

The domain crate owns the API:

```toml
# domain/Cargo.toml
[features]
default = []
python = ["rspyts/python"]
wasm = ["rspyts/wasm", "dep:wasm-bindgen"]

[dependencies]
rspyts = { version = "=1.0.0", default-features = false }
serde = { version = "1", features = ["derive"] }
wasm-bindgen = { version = "=0.2.126", optional = true }
```

```rust
use rspyts::Model;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Model)]
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

The application crate creates one binding:

```toml
# bindings/Cargo.toml
[lib]
crate-type = ["cdylib", "rlib"]

[features]
default = []
python = ["rspyts/python-extension", "domain/python"]
wasm = ["rspyts/wasm", "domain/wasm"]

[dependencies]
domain = { path = "../domain", default-features = false }
rspyts = { version = "=1.0.0", default-features = false }
```

```rust
rspyts::application!(native; domain);
```

Use Cargo package metadata only when the host package names must differ from
the application crate name:

```toml
[package.metadata.rspyts]
python = "my_application"
typescript = "@my-org/my-application"
```

## Build

Run one command:

```sh
rspyts build --manifest-path bindings/Cargo.toml
```

rspyts writes `bindings/dist/python` and `bindings/dist/typescript`.

Use the other two commands during development:

```sh
rspyts watch --manifest-path bindings/Cargo.toml
rspyts check --manifest-path bindings/Cargo.toml
```

`watch` rebuilds when a Rust or Cargo file changes. `check` fails when `dist`
does not match the Rust source.

## Host code

Python imports Pydantic models and Rust functions from one package:

```python
from my_application import Greeting, greet

result: Greeting = greet("Ada")
```

TypeScript imports the same API from one package. The package loads its
WebAssembly module during import.

```typescript
import { greet, type Greeting } from "@my-org/my-application";

const result: Greeting = greet("Ada");
```

## Example

The [`example`](example/) directory contains the Rust application, its linked
domain crate, and Python and TypeScript clients.

Licensed under [MIT](LICENSE).
