# rspyts

This crate provides the Rust API for rspyts.

Use `Model`, `Error`, `export`, and `application!` to define one Rust
application API. The `rspyts-cli` crate builds the API for Python and
TypeScript.

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
