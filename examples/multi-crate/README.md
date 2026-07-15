# multi-crate

Two bridged crates, one set of types. [`shared/rust`](shared/rust/src/lib.rs)
defines `Point` and `Axis`; [`app/rust`](app/rust/src/lib.rs) depends on it and
bridges `translate` and `mirror` over those types without redefining them.

The point of the example: the app's manifest records `Point`'s true origin
(`shared-types`), and the `[python.imports]` / `[typescript.imports]` tables in
[`app/rspyts.toml`](app/rspyts.toml) opt into importing it from shared-types'
own generated packages instead of re-emitting it locally. The mappings
intentionally couple these generated packages. Python reuses the same runtime
model class, which the smoke test proves with
`multi_crate_app.Point is shared_types.Point`. TypeScript imports the shared
package's declarations, establishing one declaration provenance and source of
truth; mutual assignability alone would not prove identity in TypeScript's
structural type system. Its smoke tests cover that assignability plus a live
round-trip.

Without those mappings, rspyts would validly emit the foreign DTOs into the app
package, keeping it self-contained but giving Python a separate model class and
TypeScript a local structural declaration. Because this example chooses
imports, regenerate shared-types before the app and keep both generated
packages on the same rspyts release; run `rspyts check` for both configs when
upgrading.

The app still generates codecs locally for the retained foreign
types. Only the public host declaration is shared, so packages do not expose or
couple themselves to another package's generator internals.

One structural rule to notice: `shared-types` is both the reusable rlib and
the standalone cdylib. Its `standalone-module` feature conditionally adds
`rspyts::export!()` for its own codegen config. The app dependency leaves that
feature disabled and supplies the app's exporter, so each compiled module has
exactly one exporter without a re-export facade.

## Layout

```
shared/rust/        reusable types + feature-gated standalone exporter
shared/rspyts.toml  enables standalone-module for shared package codegen
app/rust/           bridged functions over the shared types (+ export!())
app/rspyts.toml     codegen config with [python.imports]/[typescript.imports]
app/python/tests/   shared-import smoke tests (Python)
app/typescript/     shared-import smoke tests (TypeScript, WASM)
```

## Running

From the repository root:

```sh
# 1. Stage native + WASM artifacts and run codegen for both crates.
cargo run -p rspyts-cli -- build --config examples/multi-crate/shared/rspyts.toml \
  --target host --target wasm32-unknown-unknown --locked
cargo run -p rspyts-cli -- build --config examples/multi-crate/app/rspyts.toml \
  --target host --target wasm32-unknown-unknown --locked
cargo run -p rspyts-cli -- generate --config examples/multi-crate/shared/rspyts.toml
cargo run -p rspyts-cli -- generate --config examples/multi-crate/app/rspyts.toml

# 2. Python shared-import smoke tests.
cd examples/multi-crate/app/python && uv sync && uv run pytest

# 3. TypeScript shared-import smoke tests (the staged app WASM artifact).
cd ../typescript && npm install && npm test
```
