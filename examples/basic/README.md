# basic-example

The end-to-end rspyts example. A small, plain library defined once in
[`rust/src/lib.rs`](rust/src/lib.rs) and called from Python and TypeScript
through generated, fully typed surfaces.

Plain as it is, it exercises a broad representative slice of the portable type
system: structs, string enums, mixed tagged enums, error enums with data, exact
`i64`/`u64` values, fixed tuples, optional and `#[bridge(required)]` nullable
fields, null-valued `()` data, borrowed `&[f64]` inputs,
nested `Buf`/`Bytes`, fallible and infallible functions, bridged constants,
schemaless `serde_json::Value` passthrough, a Python-only function
(`#[bridge(target = "python")]`), and a stateful `Counter` handle class with
statics and a factory.

## Layout

```
rust/         the single source of truth (#[bridge] + rspyts::export!())
python/       pip package wrapping generated models, codecs, and calls
typescript/   npm package wrapping the generated code (src/generated)
schema/       generated JSON Schema bundle
rspyts.toml   codegen config
```

The Python/TypeScript source under the two `generated/` directories and
`schema/schema.json` is produced by `rspyts generate` and committed. The native
library staged under the Python `generated/lib` directory is intentionally
ignored and rebuilt per platform. CI fails when generated source drifts from
the Rust definitions (`rspyts check`).

## Running

From the repository root:

```sh
# 1. Stage native + WASM artifacts, then regenerate/check the clients.
cargo run -p rspyts-cli -- build --config examples/basic/rspyts.toml \
  --target host --target wasm32-unknown-unknown --locked
cargo run -p rspyts-cli -- generate --config examples/basic/rspyts.toml

# 2. Python end-to-end tests (ctypes + numpy + pydantic).
cd examples/basic/python && uv sync && uv run pytest

# 3. TypeScript end-to-end tests (the staged WASM artifact).
cd ../typescript && npm install && npm test
```

To build the same thing yourself from scratch, follow the [quickstart](../../docs/introduction/quickstart.md).
