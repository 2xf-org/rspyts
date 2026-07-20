# Example application

This example uses one aggregate binding and one linked Rust domain crate.

```text
example/
├── crates/
│   ├── bindings/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   └── dice/
│       ├── Cargo.toml
│       └── src/lib.rs
└── clients/
    ├── python/
    │   ├── example_client/__init__.py
    │   ├── tests/test_client.py
    │   ├── pyproject.toml
    │   └── uv.lock
    └── typescript/
        ├── src/index.ts
        ├── package-lock.json
        ├── package.json
        └── tsconfig.json
```

The `example-dice` crate owns the models and behavior. The `example` crate
contains only this application declaration:

```rust
rspyts::application!(example_dice);
```

Build both host packages:

```sh
cargo run -p rspyts-cli -- build
```

The build creates the generated Python and TypeScript packages in
`example/crates/bindings/dist`. Git stores this generated snapshot, including
the native Python extension and the WebAssembly binary. Run the build again to
replace the snapshot with binaries for your operating system.

Then run the authored clients:

```sh
uv run --project example/clients/python pytest -q example/clients/python/tests
npm --prefix example/clients/typescript ci
npm --prefix example/clients/typescript run check
npm --prefix example/clients/typescript run build
npm --prefix example/clients/typescript run start
```

These client commands use `uv`, Python, Node.js, and npm. They are example
development tools. `rspyts build` does not require them.

Both clients roll three seeded dice. Both clients return `[5, 4, 2]` with a
total of `11`.
