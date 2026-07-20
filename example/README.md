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
    └── typescript/
```

The `example-dice` crate owns the models and behavior. The `example` crate
contains only this application declaration:

```rust
rspyts::application!(native; example_dice);
```

Build both host packages:

```sh
cargo run -p rspyts-cli -- build --manifest-path example/crates/bindings/Cargo.toml
```

Then run the authored clients:

```sh
uv run --project example/clients/python pytest -q example/clients/python/tests
npm --prefix example/clients/typescript install
npm --prefix example/clients/typescript run check
npm --prefix example/clients/typescript run build
npm --prefix example/clients/typescript run start
```

Both clients roll three seeded dice. Both clients return `[5, 4, 2]` with a
total of `11`.
