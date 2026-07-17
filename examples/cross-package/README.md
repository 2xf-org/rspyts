# Cross-package identity acceptance

This fixture models the real Hardware -> Evaluation dependency without mirror
contracts:

- Hardware owns `SignalDefinition` and exports it to
  `neurovirtual.hardware` and `@neurovirtual/hardware`.
- Evaluation consumes the same Rust `SignalDefinition`, exports Python behavior
  from `neurovirtual.evaluation`, and emits static TypeScript vocabulary from
  `@neurovirtual/evaluation`.
- Evaluation must import the Python class owned by `neurovirtual.hardware`.
  It must not generate another `SignalDefinition` class.
- Evaluation's lock must record Hardware's package identity and exact semantic
  fingerprint.

The committed schema-2 locks record the exact cross-package dependency and host
identities. Generated output remains ignored.

Run the current compiler from the repository root:

```sh
cargo run -p rspyts-cli -- lock --config examples/cross-package/hardware/rspyts.toml
cargo run -p rspyts-cli -- build --config examples/cross-package/hardware/rspyts.toml
cargo run -p rspyts-cli -- lock --config examples/cross-package/evaluation/rspyts.toml
cargo run -p rspyts-cli -- build --config examples/cross-package/evaluation/rspyts.toml
python examples/cross-package/assert_identity.py
uv run --with 'pydantic>=2.11' --with 'numpy>=2' \
  python examples/cross-package/assert_runtime_identity.py
python examples/cross-package/assert_stale_locks.py
```

Every command is an acceptance gate. No generated client is checked in.
