# rspyts

The Rust contract API behind rspyts-generated Python and TypeScript packages.

Use one exact version, one `rspyts::module!` per contract crate, and enable only
the target feature needed by the host build:

```toml
[dependencies]
rspyts = { version = "=0.4.6", default-features = false }
```

The public surface includes `Type`, `Error`, `export`, and `module!`. The
[`rspyts-cli`](https://crates.io/crates/rspyts-cli) validates the contract,
generates the fixed `.rspyts/` tree, and maintains the semantic lock.

See the [project README](https://github.com/2xf-org/rspyts/blob/main/README.md)
and [contract reference](https://github.com/2xf-org/rspyts/blob/main/docs/reference.md).

Licensed under MIT.
