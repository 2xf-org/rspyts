# Independent Python and static TypeScript contract

This fixture combines authored Python source with generated Python contracts
and ordinary static ESM declarations for Rust-owned types, constants, fixed
bytes, and tables. Its executable function is Python-only; TypeScript does not
use WebAssembly.

Generate both hosts and verify the semantic lock from the repository root:

```sh
cargo run --locked -p rspyts-cli -- \
  build --config examples/static/rspyts.toml
cargo run --locked -p rspyts-cli -- \
  check --locked --config examples/static/rspyts.toml
```

Build the Python wheel and run the authored-source behavior test:

```sh
cd examples/static/python
python -m maturin develop
python -m pytest
```

Run the static TypeScript type and behavior tests:

```sh
cd examples/static/typescript
npm ci
npm test
```

The commands exit successfully after testing both generated hosts. See
[Static contracts and dependencies](../../docs/static-and-dependencies.md) for
the same pattern in a smaller example.
