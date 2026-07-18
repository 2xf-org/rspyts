# rspyts

Compile one Rust contract into Python source and TypeScript packages without
maintaining host-specific mirror models.

## Supported scope

rspyts 0.4 supports one `rspyts::module!` contract crate in one Cargo workspace
pinned to one Rust/Cargo toolchain. A contract graph resolves each Cargo package
name to one exact version and may import at most one direct leaf contract.

The three supported package shapes are:

| Shape | Python | TypeScript | Dependency |
| --- | --- | --- | --- |
| Executable owner | Generated source for one Maturin abi3 wheel | Browser WASM plus `./wire` | None |
| Static leaf | Generated source for one Maturin abi3 wheel | Static ESM and declarations | None |
| Static consumer | Generated source for one Maturin abi3 wheel | Static ESM importing an owner's `./wire` | One direct WASM leaf |

Commit `rspyts.toml` and `rspyts.lock`. Ignore the fixed `.rspyts/` output.
rspyts is a Cargo-installed build tool; generated packages contain no rspyts
runtime dependency.

Unsupported configurations include standalone Python extensions, custom output
directories, absolute or out-of-workspace package paths, transitive contract
graphs, multiple versions of one package name, and custom Cargo toolchains,
configuration graphs, or compiler wrappers. Public names must already be valid
for each enabled host and must not collide with Pydantic or generated runtime
members.

## Install

The repository pins Rust and Cargo in `rust-toolchain.toml`. Pin the CLI and
runtime crates to the same exact release:

```sh
cargo install rspyts-cli --version '=0.4.6' --locked
```

Executable TypeScript also requires the pinned WebAssembly tool:

```sh
rustup target add wasm32-unknown-unknown --toolchain 1.88.0
cargo install wasm-bindgen-cli --version '=0.2.126' --locked
```

## Minimal executable contract

Use a workspace member whose library emits both an `rlib` and `cdylib`:

```toml
[lib]
crate-type = ["rlib", "cdylib"]

[dependencies]
rspyts = { version = "=0.4.6", default-features = false }
serde = { version = "1", features = ["derive"] }
wasm-bindgen = { version = "=0.2.126", optional = true }

[features]
default = []
python = ["rspyts/python-extension"]
wasm = ["rspyts/wasm", "dep:wasm-bindgen"]
```

The Rust type and behavior remain canonical:

```rust
#[derive(serde::Serialize, serde::Deserialize, rspyts::Type)]
#[serde(rename_all = "camelCase")]
pub struct Greeting {
    pub message: String,
}

#[rspyts::export]
pub fn greet(name: String) -> Greeting {
    Greeting {
        message: format!("Hello, {name}!"),
    }
}

rspyts::module!(native);
```

Configure the workspace-relative crate, generated Python package, and WASM
package. `python.source` is an optional authored source tree copied into the
fixed generated output before rspyts writes its modules.

```toml
[crate]
path = "rust"

[python]
package = "example_contract"
source = "python/src"

[typescript]
package = "@example/contract"
mode = "wasm"
```

Build, inspect, and accept the semantic contract:

```sh
rspyts build
rspyts inspect
rspyts lock
rspyts check --locked
```

`rspyts build` replaces only `.rspyts/`. Package `.rspyts/python` with Maturin
and `.rspyts/typescript` with npm; do not publish the generated directory from
the source tree.

```gitignore
.rspyts/
.rspyts.tmp-*
.rspyts.old-*
.rspyts.build.lock
.rspyts.lock.tmp-*
```

## Documentation

- [Build the executable Python/WASM shape](docs/python-and-typescript.md)
- [Build static leaves and direct consumers](docs/static-and-dependencies.md)
- [Read the exact configuration and contract reference](docs/reference.md)
- [Maintain and release rspyts](MAINTAINING.md)

Runnable neutral fixtures are the executable
[`owner`](examples/cross-package/owner/), static
[`consumer`](examples/cross-package/consumer/), and independent
[`static`](examples/static/) packages.

Licensed under [MIT](LICENSE).
