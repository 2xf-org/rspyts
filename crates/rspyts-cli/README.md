# rspyts-cli

Compiler and build orchestrator for [`rspyts`](https://crates.io/crates/rspyts).
It extracts the semantic contract from a consumer crate, validates and locks
that contract, and stages self-contained Python and TypeScript package output.

```sh
cargo install rspyts-cli --version =0.4.1 --locked
rspyts build
rspyts lock
rspyts check --locked
```

Generated implementation and compiled staging artifacts live below
`.rspyts/`. Its atomic replacement siblings `.rspyts.tmp-*`,
`.rspyts.old-*`, `.rspyts.lock.tmp-*`, and `.rspyts.lock.old-*` should be
ignored. `rspyts.lock` is deterministic,
pretty-printed semantic review data and should be committed.

The CLI is distributed only as a Cargo crate. It does not publish or require a
Python or npm rspyts runtime.

See the [quickstart](https://github.com/2xf-org/rspyts/blob/main/docs/quickstart.md)
and [reference](https://github.com/2xf-org/rspyts/blob/main/docs/reference.md).

Licensed under MIT.
