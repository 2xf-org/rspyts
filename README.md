# rspyts

Compile one Rust API into self-contained Python and TypeScript packages.

rspyts keeps the public types and behavior in Rust, generates the host
boundaries, and records one reviewable semantic contract for both languages.
There is no rspyts runtime to install from PyPI or npm.

## Install

```sh
cargo install rspyts-cli --version '=0.4.1' --locked
```

Executable TypeScript packages also need the WebAssembly target and the
matching wasm-bindgen CLI:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version '=0.2.126' --locked
```

## Define the Rust API

Add the library and its host features:

```toml
[lib]
crate-type = ["rlib", "cdylib"]

[dependencies]
rspyts = { version = "=0.4.1", default-features = false }
serde = { version = "1", features = ["derive"] }
wasm-bindgen = { version = "=0.2.126", optional = true }

[features]
default = []
python = ["rspyts/python-extension"]
wasm = ["rspyts/wasm", "dep:wasm-bindgen"]
```

Export the real domain type and function—no bridge model or duplicate
implementation:

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

## Configure and build

Create `rspyts.toml` beside the Rust and host package directories:

```toml
[crate]
path = "rust"
features = []

[python]
package = "example_contract"
mode = "source"

[typescript]
package = "@example/contract"
mode = "wasm"
```

Then compile both packages and accept the first semantic contract:

```sh
rspyts build
rspyts lock
git add rspyts.toml rspyts.lock
```

Python source mode is designed for Maturin; TypeScript WASM mode produces an
npm-ready staging package. The complete packaging setup is in
[Python and TypeScript](docs/python-and-typescript.md).

Once those artifacts are installed, both languages call the same Rust
implementation:

```python
from example_contract import greet

result = greet("Ada")
assert result.message == "Hello, Ada!"
```

```typescript
import init, { greet } from "@example/contract";

await init();
const result = greet("Ada");
console.log(result.message); // Hello, Ada!
```

## Keep contracts from drifting

rspyts separates the semantic contract from generated build output:

- Commit `rspyts.toml` and `rspyts.lock`.
- Review lock changes as public API changes.
- Ignore `.rspyts/` and every atomic staging sibling.
- Run `rspyts check --locked` in CI.
- Run `rspyts lock` only when intentionally accepting a contract change.

```gitignore
.rspyts/
.rspyts.tmp-*
.rspyts.old-*
.rspyts.lock.tmp-*
.rspyts.lock.old-*
```

The rule is simple: `rspyts.lock` is tracked; generated Python, TypeScript,
JavaScript, WebAssembly, and native artifacts are not. If Rust and the lock
diverge, the locked check fails with a semantic diff.

## Choose the right output

- Use Python `source` mode with Maturin for an abi3 wheel.
- Use TypeScript `wasm` mode when JavaScript must execute Rust.
- Use TypeScript `static` mode for types, enums, constants, and tables.
- Import another Rust-owned contract through its tracked lock, never its
  generated directory.

## Documentation

- [Python and TypeScript end to end](docs/python-and-typescript.md)
- [Static output and contract dependencies](docs/static-and-dependencies.md)
- [Configuration, commands, and supported Rust](docs/reference.md)

Contributing or preparing a release? See [Maintaining rspyts](MAINTAINING.md).

The repository also contains runnable
[contract](examples/contract/),
[static](examples/static/), and
[cross-package](examples/cross-package/) examples.

Licensed under [MIT](LICENSE).
