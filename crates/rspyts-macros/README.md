# rspyts-macros

`rspyts-macros` implements the `#[bridge]` procedural macro re-exported by [rspyts](https://crates.io/crates/rspyts).

Most applications should not depend on this crate directly. Add `rspyts` to the bridged Rust crate and import the macro from the facade:

```rust
use rspyts::bridge;

#[bridge]
pub struct Summary {
    pub item_count: u32,
    pub average: f64,
}
```

The macro validates the supported bridge type system at compile time, emits
panic-contained C ABI shims for functions and classes, and registers manifest
declarations consumed by `rspyts-cli`. Default `#[bridge]` owns the Serde
derives; `#[bridge(serde)]` adopts an existing derived Serde contract and
reflects the supported rename/tag/transparent vocabulary.

See the [macro and type-system guide](https://github.com/2xf-org/rspyts/blob/main/docs/design/type-system.md) and [code generation specification](https://github.com/2xf-org/rspyts/blob/main/docs/design/codegen.md) for the supported surface.

Licensed under MIT.
