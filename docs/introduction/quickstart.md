# Quickstart

This walkthrough builds a small crate from nothing and calls it from both Python and TypeScript. It touches every moving part once: `#[bridge]`, `export!()`, `rspyts.toml`, `rspyts generate`, and the two runtimes. It is the shipped example at [examples/basic](../../examples/basic), rebuilt by hand.

Note that stable Rust, Python 3.13+, and Node 20+ are required. The TypeScript half also needs the WASM target:

```
rustup target add wasm32-unknown-unknown
cargo install rspyts-cli
```

## 1. Create the crate

The project holds one Rust crate and the two language surfaces next to it:

```
mkdir basic-example && cd basic-example
cargo new --lib rust --name basic-example
```

Edit `rust/Cargo.toml`:

```toml
[package]
name = "basic-example"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
rspyts = "0.1"
serde = { version = "1", features = ["derive"] }
```

Note that `"cdylib"` is required — it is what rspyts loads, natively via dlopen and as the linkable output for wasm32. `"rlib"` keeps the crate usable as a normal dependency and by `cargo test`. `serde` with the `derive` feature is also required, because `#[bridge]` expands to serde derives in your crate.

## 2. Define the contract

Replace `rust/src/lib.rs` (this is a trimmed version of [the shipped example](../../examples/basic/rust/src/lib.rs)):

```rust
use rspyts::{bridge, Buf};

#[bridge]
/// Summary statistics for a list of numbers.
pub struct Summary {
    /// How many values were summarized.
    pub item_count: u32,
    pub total: f64,
    pub average: f64,
    /// Optional label passed through untouched.
    pub label: Option<String>,
}

#[bridge(error)]
#[derive(Debug)]
pub enum BasicError {
    /// The input contained no values.
    EmptyInput,
    /// The scale factor must be non-zero.
    ZeroFactor { factor: f64 },
}

impl std::fmt::Display for BasicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BasicError::EmptyInput => write!(f, "input is empty"),
            BasicError::ZeroFactor { factor } => {
                write!(f, "factor must be non-zero, got {factor}")
            }
        }
    }
}

/// Summarize a list of numbers.
#[bridge]
pub fn summarize(values: &[f64], label: Option<String>) -> Result<Summary, BasicError> {
    if values.is_empty() {
        return Err(BasicError::EmptyInput);
    }
    let total: f64 = values.iter().sum();
    Ok(Summary {
        item_count: values.len() as u32,
        total,
        average: total / values.len() as f64,
        label,
    })
}

/// Multiply every value by `factor`.
#[bridge]
pub fn scale(values: &[f64], factor: f64) -> Result<Buf<f64>, BasicError> {
    if factor == 0.0 {
        return Err(BasicError::ZeroFactor { factor });
    }
    Ok(Buf::new(values.iter().map(|v| v * factor).collect()))
}

/// A counter that lives in Rust; Python and TypeScript hold an opaque
/// handle to it. Deliberately not annotated — the #[bridge] impl below
/// makes it a class. A type is data or a class, never both.
pub struct Counter {
    value: i32,
}

#[bridge]
impl Counter {
    /// Create a counter starting at `start`.
    #[bridge(constructor)]
    pub fn new(start: i32) -> Self {
        Self { value: start }
    }

    /// Increase the counter and return the new value.
    pub fn increment(&mut self, by: i32) -> i32 {
        self.value += by;
        self.value
    }

    /// Current value, without modifying it.
    pub fn current_value(&self) -> i32 {
        self.value
    }
}

rspyts::export!();
```

A few things worth knowing about what you just wrote:

- `&[f64]` parameters cross as raw pointer+length. numpy arrays and `Float64Array`s pass through with no JSON, and no copy on the Rust side.
- `Buf<f64>` returns bulk data the same way. Use `Vec<f64>` only for small collections — it serializes as a JSON array.
- The error enum's variant names become wire `code`s (`"emptyInput"`), `Display` becomes the message, and named fields become structured `data`.
- Doc comments propagate into the generated Python docstrings, TypeScript doc comments, and JSON Schema descriptions.
- An unsupported type (`u64` in a field, a tuple) does not compile. That is the [type system](../design/type-system.md) working as intended.
- `export!()` appears exactly once per compiled module — in the crate you build as the cdylib. It defines the four module-level ABI symbols.

## 3. Configure and generate

`rspyts init` writes a commented starter `rspyts.toml`. Edit it to match this layout:

```toml
[crate]
path = "rust"

[python]
enabled = true
out = "python/src/basic_example/generated"
# Searched for the compiled cdylib at import time, relative to the
# generated package directory. RSPYTS_LIBRARY always wins (see below).
library_search = ["../../../../rust/target/debug", "../../../../rust/target/release"]

[typescript]
enabled = true
out = "typescript/src/generated"

[schema]
enabled = true
out = "schema"
```

All relative paths resolve against the directory containing `rspyts.toml`. Now generate:

```
rspyts generate
```

This builds the crate, loads the resulting cdylib, checks the ABI version, extracts the manifest from the compiled module, and runs the emitters:

```
python/src/basic_example/generated/   pydantic models, constants, exceptions, typed wrappers, the loader
typescript/src/generated/             types.ts constants.ts errors.ts client.ts index.ts
schema/                               schema.json
```

Every file starts with a `DO NOT EDIT` header and the manifest hash. The output directories are wholly owned by the generator — do not put your own files in them.

## 4. Call it from Python

```
pip install rspyts
```

`demo.py` in the project root:

```python
import numpy as np

from basic_example.generated import (
    BasicErrorEmptyInput,
    Counter,
    scale,
    summarize,
)

summary = summarize(np.array([2.0, 4.0, 6.0]), "demo")
print(summary.average, summary.label)     # 4.0 demo

doubled = scale(np.array([1.0, 2.0, 3.0]), 2.0)
print(doubled)                            # [2. 4. 6.]  (a fresh np.ndarray)

try:
    summarize(np.array([]), None)
except BasicErrorEmptyInput as e:
    print(e.code)                         # emptyInput

with Counter(10) as counter:
    counter.increment(5)
    print(counter.current_value())        # 15
```

```
PYTHONPATH=python/src python demo.py
```

The generated package loads the shared library lazily, on the first call. Resolution order: the `RSPYTS_LIBRARY` environment variable (a full path, always wins), then a programmatic override on the library singleton, then each `library_search` directory joined with the platform filename — `libbasic_example.dylib` on macOS, `libbasic_example.so` on Linux, `basic_example.dll` on Windows. Hyphens in the crate name are normalized to underscores. Because `rspyts generate` already ran `cargo build`, the debug cdylib is in place and the search list finds it — no env var needed during development.

## 5. Call it from TypeScript

Build the WASM module — same crate, same source, different target:

```
cargo build --manifest-path rust/Cargo.toml --target wasm32-unknown-unknown
cd typescript
npm init -y && npm pkg set type=module
npm install rspyts tsx
```

`"type": "module"` in `package.json` is required — the demo below is ES
modules, and `npx tsx` needs it to run them.

`typescript/demo.mts`:

```ts
import { readFile } from "node:fs/promises";
import { instantiate } from "rspyts";
import { BasicErrorEmptyInput, createClient } from "./src/generated/index.js";

const mod = await instantiate(
  await readFile("../rust/target/wasm32-unknown-unknown/debug/basic_example.wasm"),
);
const client = createClient(mod);

const summary = client.summarize(new Float64Array([2, 4, 6]), "demo");
console.log(summary.average, summary.label);              // 4 demo

const doubled = client.scale(new Float64Array([1, 2, 3]), 2);
console.log(doubled);                                     // Float64Array [2, 4, 6]

try {
  client.summarize(new Float64Array(0), null);
} catch (e) {
  if (e instanceof BasicErrorEmptyInput) console.log(e.code); // emptyInput
}

using counter = new client.Counter(10); // freed automatically at scope exit
counter.increment(5);
console.log(counter.currentValue());                      // 15
```

```
npx tsx demo.mts
```

For browser and bundler patterns, see the [TypeScript guide](../typescript.md).

## 6. Lock it in CI

Commit the generated code, and let `rspyts check` fail the build whenever it drifts from the Rust definitions (exit 1, with a unified diff):

```yaml
- run: cargo install rspyts-cli --locked
- run: rspyts check
```

A change to the contract is now always a reviewable diff in the same PR — in Rust, Python, TypeScript, and JSON Schema at once. Why this model: [ADR-3](../design/decisions.md#adr-3-commit-generated-code-gate-it-with-a-drift-check).

## The shipped example

The full version of this walkthrough lives at [examples/basic](../../examples/basic), with end-to-end test suites in both languages. From the repository root:

```
cargo build -p basic-example
cargo run -p rspyts-cli -- generate --config examples/basic/rspyts.toml

cd examples/basic/python && uv sync && uv run pytest

cargo build -p basic-example --target wasm32-unknown-unknown
cd ../typescript && npm install && npm test
```

## Where to go next

- [How rspyts works](how-rspyts-works.md) — what actually crosses the boundary.
- [Python guide](../python.md) — models, exceptions, numpy semantics, the GIL, packaging.
- [TypeScript guide](../typescript.md) — instantiation patterns, typed arrays, handle disposal, bundling.
- [Type system](../design/type-system.md) — exactly what can cross, and why some things can't.
