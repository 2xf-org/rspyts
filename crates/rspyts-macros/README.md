# rspyts-macros

`rspyts-macros` implements the procedural macros re-exported by the [`rspyts`](https://crates.io/crates/rspyts) crate:

- `#[derive(rspyts::Model)]`
- `#[derive(rspyts::Error)]`
- `#[rspyts::export]`
- `rspyts::application!`

Application code should depend on `rspyts`, not this crate. The public crate supplies the contract types and runtime required by every expansion, and it pins a compatible macro implementation.

```toml
[dependencies]
rspyts = "3"
```

See the [`rspyts` contract reference](https://github.com/2xf-org/rspyts/tree/main/crates/rspyts) for supported Rust declarations and attributes, or the [CLI reference](https://github.com/2xf-org/rspyts/tree/main/crates/rspyts-cli) to build Python and TypeScript packages.
