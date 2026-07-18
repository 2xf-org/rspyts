# rspyts-macros

Procedural macro implementation for
[`rspyts`](https://crates.io/crates/rspyts).

Do not depend on this crate directly. Depend on the exact matching `rspyts`
release, which exposes `Type`, `Error`, `export`, and `module!`:

```toml
[dependencies]
rspyts = { version = "=0.4.4", default-features = false }
```

Each contract crate declares exactly one `rspyts::module!`. See the
[project README](https://github.com/2xf-org/rspyts/blob/main/README.md) and
[macro reference](https://github.com/2xf-org/rspyts/blob/main/docs/reference.md).

Licensed under MIT.
