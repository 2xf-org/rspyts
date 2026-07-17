# rspyts

The Rust API for exporting one implementation to Python and TypeScript.

It provides `Type`, `Error`, `export`, and `module!`, plus the target support
used by generated PyO3 and wasm-bindgen bindings.

```toml
[dependencies]
rspyts = { version = "0.4.1", default-features = false }
```

Most projects also install
[`rspyts-cli`](https://crates.io/crates/rspyts-cli) to build and lock the
generated packages.

Start with the [project README](https://github.com/2xf-org/rspyts#readme)
or follow the [complete Python and TypeScript guide](https://github.com/2xf-org/rspyts/blob/main/docs/python-and-typescript.md).

Licensed under MIT.
