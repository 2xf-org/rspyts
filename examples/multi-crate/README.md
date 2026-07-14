# multi-crate

Two bridged crates, one set of types. [`shared/rust`](shared/rust/src/lib.rs) defines `Point` and `Axis`; [`app/rust`](app/rust/src/lib.rs) depends on it and bridges `translate` and `mirror` over those types without redefining them.

The point of the example: the app's manifest records `Point`'s true origin (`shared-types`), and the `[python.imports]` / `[typescript.imports]` tables in [`app/rspyts.toml`](app/rspyts.toml) tell the emitters to *import* it from shared-types' own generated packages instead of re-emitting a lookalike. The smoke tests pin exactly that — `multi_crate_app.Point is shared_types.Point` in Python, mutual assignability plus a live round-trip in TypeScript.

One structural rule to notice: `shared-types` is a plain rlib with no `rspyts::export!()`, because a compiled module has exactly one exporter and this crate is linked into the app's cdylib. The tiny [`shared/module`](shared/module/src/lib.rs) crate re-exports it and adds the export, giving `rspyts generate` a standalone cdylib for shared-types' own packages.

## Layout

```
shared/rust/        the types (#[bridge], no export!())
shared/module/      leaf cdylib: pub use shared_types::*; rspyts::export!()
shared/rspyts.toml  codegen config -> shared/python, shared/typescript
app/rust/           bridged functions over the shared types (+ export!())
app/rspyts.toml     codegen config with [python.imports]/[typescript.imports]
app/python/tests/   shared-import smoke tests (Python)
app/typescript/     shared-import smoke tests (TypeScript, WASM)
```

## Running

From the repository root:

```sh
# 1. Stage native + WASM artifacts and run codegen for both crates.
cargo run -p rspyts-cli -- build --config examples/multi-crate/shared/rspyts.toml
cargo run -p rspyts-cli -- build --config examples/multi-crate/app/rspyts.toml
cargo run -p rspyts-cli -- generate --config examples/multi-crate/shared/rspyts.toml
cargo run -p rspyts-cli -- generate --config examples/multi-crate/app/rspyts.toml

# 2. Python shared-import smoke tests.
cd examples/multi-crate/app/python && uv sync && uv run pytest

# 3. TypeScript shared-import smoke tests (the staged app WASM artifact).
cd ../typescript && npm install && npm test
```
