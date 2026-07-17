# rspyts-macros

Procedural macro implementation for
[`rspyts`](https://crates.io/crates/rspyts).

Do not depend on this crate directly. Add `rspyts` instead; it exposes the
public `Type`, `Error`, `export`, and `module!` macros:

```toml
[dependencies]
rspyts = { version = "0.4.1", default-features = false }
```

See the [project README](https://github.com/2xf-org/rspyts#readme)
and [macro reference](https://github.com/2xf-org/rspyts/blob/main/docs/reference.md).

Licensed under MIT.
