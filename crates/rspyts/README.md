# rspyts

Define a bridge once in Rust, then generate typed Python and TypeScript clients plus JSON Schema. `rspyts` is the public Rust facade: it exports `#[bridge]`, bridge data types, error support, and `export!()` for a `cdylib`.

## Install

```toml
[lib]
crate-type = ["rlib", "cdylib"]

[dependencies]
rspyts = "0.2"
```

Install the generator separately with `cargo install rspyts-cli`.

## Minimal bridge

```rust
use rspyts::{bridge, Buf};

#[bridge]
pub struct Summary {
    pub item_count: u32,
    pub average: f64,
}

#[bridge]
pub fn summarize(values: &[f64]) -> Summary {
    let total: f64 = values.iter().sum();
    Summary {
        item_count: values.len() as u32,
        average: total / values.len() as f64,
    }
}

#[bridge]
pub fn scale(values: &[f64], factor: f64) -> Buf<f64> {
    values.iter().map(|value| value * factor).collect::<Vec<_>>().into()
}

rspyts::export!();
```

Run `rspyts init`, set the output directories in `rspyts.toml`, then run
`rspyts generate`. Supported data includes structs, transparent newtypes,
string and tagged enums (including mixed unit/data variants), typed errors,
exact `I64`/`U64` values, tuples of arity 2–12, options, lists, string-keyed
maps, constants, JSON values, opaque `Bytes`, numeric slices and owned
`Buf<T>` values, and handle-backed classes. Use `#[bridge(serde)]` to adopt an
existing supported derived Serde contract.

Start with the [quickstart](https://github.com/2xf-org/rspyts/blob/main/docs/introduction/quickstart.md). The [type-system](https://github.com/2xf-org/rspyts/blob/main/docs/design/type-system.md), [ABI](https://github.com/2xf-org/rspyts/blob/main/docs/design/abi.md), and [code generation](https://github.com/2xf-org/rspyts/blob/main/docs/design/codegen.md) documents are normative.

Licensed under MIT.
