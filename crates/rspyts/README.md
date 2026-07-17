# rspyts

`rspyts` is the public Rust crate for compiling one curated Rust API into
Python and TypeScript consumer packages. It provides the `Type` and `Error`
derives, the `export` attribute, `module!`, semantic contract types, and the
target-gated PyO3/wasm-bindgen boundary support used by generated packages.

Version 0.4 is a clean-slate API. It has no compatibility layer for 0.3, no
generic application-call C ABI, and no installed Python or npm runtime.

```toml
[dependencies]
rspyts = { version = "0.4.0", default-features = false }
serde = { version = "1", features = ["derive"] }
wasm-bindgen = { version = "0.2.126", optional = true }

[features]
default = []
python = ["rspyts/python-extension"]
wasm = ["rspyts/wasm", "dep:wasm-bindgen"]
```

`python-extension` selects the PyO3 ABI and extension link mode for consumer
cdylibs. wasm-bindgen remains an optional direct dependency because its
attributes must resolve in the consumer crate. See the
[workspace quickstart](https://github.com/2xf-org/rspyts/blob/main/docs/quickstart.md)
for the complete feature wiring.

```rust
#[derive(serde::Serialize, serde::Deserialize, rspyts::Type)]
pub struct Greeting {
    pub message: String,
}

#[rspyts::export]
pub fn greet(name: String) -> Greeting {
    Greeting {
        message: format!("Hello, {name}!"),
    }
}

rspyts::module!(native);
```

Generated staging output belongs below `.rspyts/` and is ignored. Commit the
deterministic, pretty-printed semantic `rspyts.lock` produced by `rspyts-cli`
instead.

Licensed under MIT.
