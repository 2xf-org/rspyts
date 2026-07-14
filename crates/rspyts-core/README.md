# rspyts-core

`rspyts-core` contains the internal wire types, manifest IR, symmetric request/response envelopes, binary attachment codec, handle storage, and registry used by [rspyts](https://crates.io/crates/rspyts) and [rspyts-cli](https://crates.io/crates/rspyts-cli).

Most applications should not depend on this crate directly. Use the `rspyts` facade for the public Rust API and the `rspyts` command-line package to generate Python, TypeScript, and JSON Schema projections.

The ABI and manifest format are specified in the repository:

- [ABI specification](https://github.com/2xf-org/rspyts/blob/main/docs/design/abi.md)
- [Type system](https://github.com/2xf-org/rspyts/blob/main/docs/design/type-system.md)
- [Architecture](https://github.com/2xf-org/rspyts/blob/main/docs/architecture.md)

This crate's implementation modules are not a separate compatibility surface. Public bridge development should go through `rspyts`.

Licensed under MIT.
