# rspyts

This crate provides the Rust API for rspyts.

Use `Model`, `Error`, `export`, and `application!` to define one Rust
application API. The `rspyts-cli` crate builds the API for Python and
TypeScript.

The generated package paths follow the Cargo package names and Rust modules.
The declaration location defines the path. Rust re-exports do not change it.
All namespaces use one aggregate native extension and one aggregate
WebAssembly file.

An API crate that defines models normally needs these dependencies:

```toml
[dependencies]
rspyts = "1"
serde = { version = "1", features = ["derive"] }
```

Do not add `rspyts-macros`, PyO3, or `wasm-bindgen` directly. rspyts owns
those implementation dependencies. `thiserror` is optional. Use it only if
you want its Rust error derive. Add `chrono` or `serde_json` only if your API
source uses their types.

Read the [project README](https://github.com/2xf-org/rspyts).
