# 🌉 rspyts

<p align="center">
  <a href="https://github.com/2xf-org/rspyts/actions/workflows/validation.yml">
    <img src="https://github.com/2xf-org/rspyts/actions/workflows/validation.yml/badge.svg" alt="Validation">
  </a>
</p>

Define an API once in Rust, then call it from Python and TypeScript.

rspyts generates typed clients from the compiled Rust crate. Python calls a
native library through `ctypes`; TypeScript calls the same API through
WebAssembly. Both use one small, documented ABI.

## Installing

Rust 1.85+, Python 3.11+, and Node.js 22.12+ are supported.

```sh
cargo add rspyts@=0.3.1
cargo install rspyts-cli --version 0.3.1 --locked
pip install rspyts==0.3.1
npm install rspyts@0.3.1
```

The bridged Rust crate must build as a `cdylib` and depend on `rspyts`.
An `rlib` output is needed only when another Rust crate also links the crate;
a direct Serde dependency is needed only when user code names Serde itself,
for example with `#[bridge(serde)]`. The
[quickstart](docs/introduction/quickstart.md) contains a complete setup.

## Using

Annotate the Rust surface and export it once:

```rust
use rspyts::bridge;

#[bridge]
pub struct Greeting {
    pub message: String,
}

#[bridge]
pub fn greet(name: String) -> Greeting {
    Greeting {
        message: format!("Hello, {name}!"),
    }
}

rspyts::export!();
```

Generate the clients and build the native and WebAssembly artifacts:

```sh
rspyts generate
rspyts build
```

The generated Python wrapper is an ordinary typed function:

```python
from greeting.generated import greet

print(greet("Ada").message)
```

The generated TypeScript client exposes the same contract:

```ts
import { readFile } from "node:fs/promises";
import { instantiate } from "rspyts";
import { createClient } from "./generated/index.js";

const module = await instantiate(await readFile("greeting.wasm"));
console.log(createClient(module).greet("Ada").message);
```

## How it works

`#[bridge]` records types, functions, constants, errors, and stateful classes in
the compiled crate. The CLI reads that manifest and writes deterministic
Python, TypeScript, and JSON Schema output. `rspyts check` fails when committed
generated code is stale.

Calls cross a symmetric binary envelope: structured values live in JSON, while
bytes and numeric arrays travel in aligned binary attachments. Python receives
pydantic models and typed exceptions. TypeScript receives interfaces,
discriminated unions, typed arrays, and disposable handle-backed classes.

The boundary is intentionally finite. Native `i64` and `u64` become exact
decimal strings only at the wire boundary, while `serde_json::Value` remains
transparent JSON. Asynchronous functions, callbacks, arbitrary Serde codecs,
and implicit cross-language lifetimes are outside the contract. See
the [documentation](docs/README.md) for the complete model.

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

cd runtimes/python && uv sync --dev --locked && uv run pytest
cd ../typescript && npm ci && npm run build && npm test
```

The [basic example](examples/basic/README.md) exercises the full native and
WebAssembly path. Contributions are licensed under [MIT](LICENSE).
