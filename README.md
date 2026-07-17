# rspyts

rspyts compiles one authored Rust API into Python and TypeScript packages.
Rust owns the public types and behavior; rspyts owns the bindings and their
semantic contract.

Version 0.4 is a clean-slate release. It has no compatibility layer for 0.3,
no generic C call ABI, and no Python or npm rspyts runtime. The only published
rspyts packages are the three Cargo crates:

- `rspyts` — public macros, contract IR, and target support;
- `rspyts-macros` — proc-macro implementation;
- `rspyts-cli` — the `rspyts` compiler and build command.

Consumer wheels use PyO3's `abi3-py311` interface. Executable TypeScript
packages use wasm-bindgen; vocabulary-only packages can use static TypeScript
output. Generated source and compiled staging artifacts live in `.rspyts/` and
must not be committed. The deterministic, pretty-printed `rspyts.lock`
semantic contract is intended to be diffed, reviewed, and committed.

The consumer crate keeps `default-features = false`. Its `rspyts.toml` names
backend-neutral probe features separately from the features used to build each
host. A project may configure both hosts and build either one independently;
every build extracts the same native inventory first, so one reviewed lock
governs both without enabling Python and WASM together.

```rust
#[derive(serde::Serialize, serde::Deserialize, rspyts::Type)]
#[serde(rename_all = "camelCase")]
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

```sh
cargo install rspyts-cli --version =0.4.0 --locked
rspyts build
rspyts lock
rspyts check --locked
```

The result is a generated package implementation below `.rspyts/`; the
consumer's normal Maturin or TypeScript packaging step includes that output in
its wheel or npm artifact.

Start with the [quickstart](docs/quickstart.md). The
[reference](docs/reference.md), [limitations](docs/limitations.md), and
[architecture](docs/design/v0.4.md) describe the exact 0.4 contract.

Licensed under [MIT](LICENSE).
