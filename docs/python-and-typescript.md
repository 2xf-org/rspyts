# Executable Python and TypeScript contract

This guide covers the executable owner shape: one Rust contract crate produces
generated Python source for a Maturin abi3 wheel and a browser TypeScript
package backed by WebAssembly. The complete fixture is
[`examples/cross-package/owner`](../examples/cross-package/owner).

## Workspace layout

Keep the contract in one pinned Cargo workspace:

```text
vector-contract/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── rspyts.toml
├── rspyts.lock
├── rust/
│   ├── Cargo.toml
│   └── src/lib.rs
└── python/
    ├── pyproject.toml
    └── src/vector_contract/
```

```toml
# rust-toolchain.toml
[toolchain]
channel = "1.88.0"
profile = "minimal"
targets = ["wasm32-unknown-unknown"]
```

```toml
# Cargo.toml
[workspace]
members = ["rust"]
resolver = "2"
```

Generated files are disposable. Commit the config and semantic lock, not the
host output:

```gitignore
.rspyts/
.rspyts.tmp-*
.rspyts.old-*
.rspyts.build.lock
.rspyts.lock.tmp-*
```

## Rust contract

The contract crate uses exact rspyts and wasm-bindgen versions:

```toml
# rust/Cargo.toml
[package]
name = "vector-contract"
version = "0.1.0"
edition = "2024"
rust-version = "1.88"

[lib]
crate-type = ["rlib", "cdylib"]

[dependencies]
rspyts = { version = "=0.4.2", default-features = false }
serde = { version = "1", features = ["derive"] }
thiserror = "2"
wasm-bindgen = { version = "=0.2.126", optional = true }

[features]
default = []
python = ["rspyts/python-extension"]
wasm = ["rspyts/wasm", "dep:wasm-bindgen"]
```

The authored API is ordinary Rust:

```rust
use rspyts::{Error, Type};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VectorSpec {
    pub name: String,
    pub dimensions: u32,
}

#[derive(Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Calculation {
    pub vector: VectorSpec,
    pub count: u32,
    pub mean: f64,
    #[rspyts(buffer)]
    pub scaled: Vec<f64>,
    #[rspyts(bytes)]
    pub checksum: [u8; 4],
}

#[derive(Debug, thiserror::Error, Error)]
pub enum CalculationError {
    #[error("values cannot be empty")]
    Empty,
}

#[rspyts::export]
pub fn calculate(
    vector: VectorSpec,
    #[rspyts(buffer)] values: &[f64],
    #[rspyts(bytes)] checksum: &[u8; 4],
) -> Result<Calculation, CalculationError> {
    if values.is_empty() {
        return Err(CalculationError::Empty);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    Ok(Calculation {
        vector,
        count: u32::try_from(values.len()).expect("accepted length fits u32"),
        mean,
        scaled: values.iter().map(|value| value * 2.0).collect(),
        checksum: *checksum,
    })
}

rspyts::module!(native);
```

One crate declares exactly one `rspyts::module!`. The package version and module
become part of the locked contract and ABI-v1 discovery identity.

## rspyts configuration

Python has only a package name and optional workspace-relative authored source.
There is no Python mode or standalone build. TypeScript uses WASM here:

```toml
[crate]
path = "rust"

[python]
package = "vector_contract"
source = "python/src"

[typescript]
package = "@example/vector-contract"
mode = "wasm"
```

The authored `python/src` tree is copied into `.rspyts/python`; it does not
change the fixed output location. Paths cannot be absolute, escape the project,
or traverse symlinks.

Install the matching tools, then build and lock:

```sh
cargo install rspyts-cli --version '=0.4.2' --locked
rustup target add wasm32-unknown-unknown --toolchain 1.88.0
cargo install wasm-bindgen-cli --version '=0.2.126' --locked
rspyts build
rspyts lock
rspyts check --locked
```

The generated tree contains Python modules and a complete npm-ready WASM
package:

```text
.rspyts/
├── contract.json
├── python/vector_contract/
│   ├── __init__.py
│   ├── codecs.py
│   ├── constants.py
│   ├── errors.py
│   ├── functions.py
│   ├── models.py
│   └── py.typed
└── typescript/
    ├── index.d.ts
    ├── index.js
    ├── native.js
    ├── native_bg.wasm
    ├── package.json
    ├── wire.d.ts
    └── wire.js
```

## Generated host surfaces

The generated Python is equivalent to this abridged excerpt:

```python
class VectorSpec(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    name: str
    dimensions: int = Field(ge=0, le=4_294_967_295)


class Calculation(BaseModel):
    model_config = ConfigDict(arbitrary_types_allowed=True, extra="forbid", frozen=True)
    vector: VectorSpec
    count: int = Field(ge=0, le=4_294_967_295)
    mean: float
    scaled: NDArray[np.float64]
    checksum: Annotated[bytes, Field(min_length=4, max_length=4)]


def calculate(
    vector: VectorSpec,
    values: NDArray[np.float64],
    checksum: Annotated[bytes, Field(min_length=4, max_length=4)],
) -> Calculation: ...
```

The executable TypeScript surface preserves typed arrays:

```ts
export interface VectorSpec {
  readonly name: string;
  readonly dimensions: number;
}

export interface Calculation {
  readonly vector: VectorSpec;
  readonly count: number;
  readonly mean: number;
  readonly scaled: Float64Array;
  readonly checksum: Uint8Array;
}

export function calculate(
  vector: VectorSpec,
  values: Float64Array,
  checksum: Uint8Array,
): Calculation;
```

The package also exports `@example/vector-contract/wire`. That static surface
uses JSON-safe readonly arrays and includes only complete types that can be
represented without losing information.

## Build the Python wheel

Maturin owns the one native extension:

```toml
# python/pyproject.toml
[build-system]
requires = ["maturin>=1.9,<2"]
build-backend = "maturin"

[project]
name = "vector-contract"
version = "0.1.0"
requires-python = ">=3.11"
dependencies = ["numpy>=2", "pydantic>=2.11"]

[tool.maturin]
manifest-path = "../rust/Cargo.toml"
python-source = "../.rspyts/python"
module-name = "vector_contract.native"
features = ["python"]

[tool.maturin.sbom]
rust = false
```

Disable the path-bearing default Rust SBOM or generate a separately sanitized
SBOM. Remap the physical workspace path when Maturin invokes Cargo:

```sh
workspace_root=$(pwd -P)
python3.11 -m venv /tmp/vector-contract-builder
builder=/tmp/vector-contract-builder/bin/python
"$builder" -m pip install 'maturin>=1.9,<2'

flags=(
  "--remap-path-prefix=$HOME=/home"
  "--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo"
  "--remap-path-prefix=$workspace_root=/workspace"
)
export CARGO_ENCODED_RUSTFLAGS=$(IFS=$'\x1f'; printf '%s' "${flags[*]}")
unset RUSTFLAGS
(
  cd python
  "$builder" -m maturin build --release --out dist
)
```

Install and test the wheel from outside the source tree. It contains generated
source and one `abi3-py311` extension, with no Python `rspyts` dependency.

## Pack the TypeScript package

Pack the fixed generated directory, install that tarball in a browser project,
and test the installed artifact:

```sh
mkdir -p dist/typescript
npm pack .rspyts/typescript --pack-destination dist/typescript
```

```ts
import init, { calculate } from "@example/vector-contract";

await init();
const result = calculate(
  { name: "example", dimensions: 3 },
  new Float64Array([1, 2, 3]),
  new Uint8Array([1, 2, 3, 4]),
);
console.assert(result.mean === 2);
```

Run release behavior tests in a real browser so missing exports or WASM assets
cannot be hidden by the source tree.

## Keep both hosts synchronized

Rust, host identities, the exact package version, and any direct owner snapshot
produce one semantic fingerprint:

```sh
rspyts inspect
rspyts check --locked
rspyts lock  # only after reviewing an intentional change
```

CI should generate both targets, build and install the wheel, pack and install
the npm artifact, and run behavior tests against those installed artifacts.
