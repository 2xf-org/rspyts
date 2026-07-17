# Cross-package identity acceptance

This fixture models the real Catalog -> Reports dependency without mirror
contracts:

- Catalog owns `SignalDefinition` and exports it to
  `example.catalog` and `@example/catalog`.
- Reports consumes the same Rust `SignalDefinition`, exports Python behavior
  from `example.reports`, and emits static TypeScript vocabulary from
  `@example/reports`.
- Reports must import the Python class owned by `example.catalog`.
  It must not generate another `SignalDefinition` class.
- Reports's lock must record Catalog's package identity and exact semantic
  fingerprint.

The committed schema-2 locks record the exact cross-package dependency and host
identities. Generated output remains ignored.

Run the current compiler from the repository root:

```sh
cargo run -p rspyts-cli -- lock --config examples/cross-package/catalog/rspyts.toml
cargo run -p rspyts-cli -- build --config examples/cross-package/catalog/rspyts.toml
cargo run -p rspyts-cli -- lock --config examples/cross-package/reports/rspyts.toml
cargo run -p rspyts-cli -- build --config examples/cross-package/reports/rspyts.toml
python examples/cross-package/assert_identity.py
uv run --with 'pydantic>=2.11' --with 'numpy>=2' \
  python examples/cross-package/assert_runtime_identity.py
python examples/cross-package/assert_stale_locks.py
```

Every command is an acceptance gate. No generated client is checked in.
