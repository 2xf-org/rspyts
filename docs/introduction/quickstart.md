# Quickstart

This builds one Rust function and calls it from Python and TypeScript.

## Requirements

- Rust 1.85 or newer
- Python 3.11 or newer
- Node.js 22.12 or newer
- the `wasm32-unknown-unknown` Rust target

Install the generator and target:

```sh
cargo install rspyts-cli
rustup target add wasm32-unknown-unknown
```

## 1. Create the Rust crate

Create `rust/Cargo.toml`:

```toml
[package]
name = "demo-bridge"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"

[lib]
crate-type = ["rlib", "cdylib"]

[dependencies]
rspyts = "0.2"
serde = { version = "1", features = ["derive"] }
```

Create `rust/src/lib.rs`:

```rust
use rspyts::bridge;

#[bridge]
/// Summary statistics for a list of numbers.
pub struct Summary {
    pub count: u32,
    pub total: f64,
    pub average: f64,
}

#[bridge]
/// Summarize a numeric slice.
pub fn summarize(values: &[f64]) -> Summary {
    let total: f64 = values.iter().sum();
    Summary {
        count: values.len() as u32,
        total,
        average: if values.is_empty() {
            0.0
        } else {
            total / values.len() as f64
        },
    }
}

rspyts::export!();
```

`cdylib` produces the native library and WebAssembly module. `rlib` keeps the
crate usable by Rust tests and dependencies.

## 2. Configure generation

Create `rspyts.toml` beside the `rust` directory:

```toml
[crate]
path = "rust"

[build]
profile = "dev"
targets = ["wasm32-unknown-unknown"]
locked = false

[python]
out = "python/src/demo/generated"
library_search = ["../../../../rust/target/debug"]

[typescript]
out = "typescript/generated"

[schema]
out = "schema"
```

Generate the host code and stage native/WASM artifacts:

```sh
rspyts generate
rspyts build
```

Generation reads the manifest from the compiled native library. Build stages
stable target paths under Cargo's target directory. You should now have:

```text
python/src/demo/generated/             generated Python package
typescript/generated/                  generated TypeScript client
schema/schema.json                     generated JSON Schema
rust/target/rspyts/<host>/debug/       staged native library
rust/target/rspyts/wasm32-unknown-unknown/debug/demo_bridge.wasm
```

## 3. Call it from Python

Install the runtime:

```sh
python -m pip install rspyts numpy
```

With `python/src` on `PYTHONPATH`:

```python
import numpy as np

from demo.generated import summarize

summary = summarize(np.array([2.0, 4.0, 6.0], dtype=np.float64))
print(summary.average)  # 4.0
```

The generated package loads the native library on the first call. It checks
`RSPYTS_LIBRARY` first, then an explicit `Library.set_path()` override, then
the generated `library_search` entries.

## 4. Call it from TypeScript

Install the runtime:

```sh
npm install rspyts
```

Use the generated client after your application loads the `.wasm` bytes:

```ts
import { readFile } from "node:fs/promises";
import { instantiate } from "rspyts";
import { createClient } from "./generated/index.js";

const wasm = await readFile(
  "../rust/target/rspyts/wasm32-unknown-unknown/debug/demo_bridge.wasm",
);
const client = createClient(await instantiate(wasm));

const summary = client.summarize(new Float64Array([2, 4, 6]));
console.log(summary.average); // 4
```

For a browser, pass `fetch(wasmUrl)` to `instantiate` instead.

## 5. Keep generated code current

Commit the generated files and run this in CI:

```sh
rspyts check
```

It exits with status 1 and a unified diff when output is missing, stale, or
unexpected. Use `rspyts generate` to accept a deliberate contract change.

The repository's [basic example](../../examples/basic) covers errors,
constants, exact integers, buffers, JSON, target scoping, and stateful classes.
