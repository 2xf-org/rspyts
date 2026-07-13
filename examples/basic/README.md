# basic-example

The end-to-end rspyts example. A small, plain library defined once in [`rust/src/lib.rs`](rust/src/lib.rs) and called from Python and TypeScript through generated, fully typed surfaces.

Plain as it is, it exercises the whole portable type system: structs, string enums, tagged data enums, error enums with data, optional values, zero-copy `&[f64]` inputs, `Buf<f64>` outputs, fallible and infallible functions, bridged constants, schemaless `Json` passthrough, a Python-only function (`#[bridge(target = "python")]`), and a stateful `Counter` handle class with statics and a factory.

## Layout

```
rust/         the single source of truth (#[bridge] + rspyts::export!())
python/       pip package wrapping the generated code (src/basic_example/generated)
typescript/   npm package wrapping the generated code (src/generated)
schema/       generated JSON Schema bundle
rspyts.toml   codegen config
```

Everything under the two `generated/` directories and `schema/` is produced by `rspyts generate` and committed. CI fails if it drifts from the Rust definitions (`rspyts check`).

## Running

From the repository root:

```sh
# 1. Native library + codegen.
cargo build -p basic-example
cargo run -p rspyts-cli -- generate --config examples/basic/rspyts.toml

# 2. Python end-to-end tests (ctypes + numpy + pydantic).
cd examples/basic/python && uv sync && uv run pytest

# 3. TypeScript end-to-end tests (WASM).
cargo build -p basic-example --target wasm32-unknown-unknown
cd ../typescript && npm install && npm test
```

To build the same thing yourself from scratch, follow the [quickstart](../../docs/introduction/quickstart.md).
