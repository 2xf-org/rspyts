# Python and TypeScript from one Rust API

rspyts lets one Rust API be the implementation used by Python and TypeScript.
You write the domain types, errors, functions, and stateful resources once.
rspyts generates the host-facing package code, while Maturin builds the Python
wheel and wasm-bindgen supplies the WebAssembly used by the npm package.

This guide builds a small signal-analysis library with Rust types, typed
errors, numeric buffers, bytes, and a stateful resource. The result is a native
Python wheel and a browser-ready TypeScript package.

The generated excerpts below are intentionally abridged. They show what the
compiler produces, but generated files remain build output and are never
edited or committed. A related, fully executable acceptance fixture lives in
[`examples/contract`](../examples/contract); it exercises the same features
with its own package names and additional validation coverage.

## What you will build

Start with this layout:

```text
signal-contract/
├── .gitignore
├── rspyts.toml
├── rust/
│   ├── Cargo.toml
│   └── src/
│       └── lib.rs
└── python/
    └── pyproject.toml
```

After a build, rspyts adds an ignored staging directory:

```text
.rspyts/
├── contract.json
├── python/
│   └── signal_contract/
│       ├── __init__.py
│       ├── codecs.py
│       ├── constants.py
│       ├── contract.json
│       ├── errors.py
│       ├── functions.py
│       ├── models.py
│       ├── py.typed
│       └── resources.py
└── typescript/
    ├── contract.json
    ├── index.d.ts
    ├── index.js
    ├── native.d.ts
    ├── native.js
    ├── native_bg.wasm
    ├── native_bg.wasm.d.ts
    └── package.json
```

Python source mode does not put a native extension in `.rspyts/`. Maturin
compiles the extension later and combines it with these staged sources.

The only generated contract artifact committed to Git is `rspyts.lock`. It is
a semantic description of the API, not generated Python or TypeScript source.

## Install the build tools

rspyts 0.4.1 is distributed as Cargo crates. It does not install a Python or
npm runtime dependency.

```sh
cargo install rspyts-cli --version '=0.4.1' --locked
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version '=0.2.126' --locked
python3.11 -m pip install "maturin>=1.9,<2"
```

The wasm-bindgen Rust dependency and CLI must use compatible versions. This
guide pins both to `0.2.126`.

## Author the Rust package

Create `rust/Cargo.toml`:

```toml
[package]
name = "signal-contract"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib", "rlib"]

[features]
default = []
python = ["rspyts/python-extension"]
wasm = ["rspyts/wasm", "dep:wasm-bindgen"]

[dependencies]
rspyts = { version = "=0.4.1", default-features = false }
serde = { version = "1", features = ["derive"] }
thiserror = "2"
wasm-bindgen = { version = "=0.2.126", optional = true }
```

The fixed `python` and `wasm` feature names are part of the v0.4 build
convention:

- `python` enables the PyO3 `abi3-py311` extension boundary;
- `wasm` enables the WASM boundary and the consumer's direct wasm-bindgen
  dependency; and
- neither backend is a default feature.

Now create `rust/src/lib.rs`:

```rust
use rspyts::{Error, Type};
use serde::{Deserialize, Serialize};

/// The source channel attached to an analysis result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Channel {
    pub id: String,
    pub sample_rate_hz: u32,
}

/// A portable quality classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Good,
    Noisy,
}

/// The result returned by the Rust implementation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Summary {
    pub channel: Channel,
    pub quality: Quality,
    pub count: u64,
    pub average: f64,
    #[rspyts(buffer)]
    pub normalized: Vec<f64>,
    #[rspyts(bytes)]
    pub fingerprint: Vec<u8>,
}

/// Failures callers can handle without parsing an error message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Error)]
pub enum AnalyzeError {
    #[error("values cannot be empty")]
    Empty,
    #[error("scale must be finite and greater than zero")]
    InvalidScale,
    #[error("fingerprint must contain exactly four bytes")]
    InvalidFingerprint,
}

/// Analyze one signal using the real Rust implementation.
#[rspyts::export]
pub fn summarize(
    channel: Channel,
    #[rspyts(buffer)] values: &[f64],
    #[rspyts(bytes)] fingerprint: &[u8],
    scale: f64,
) -> Result<Summary, AnalyzeError> {
    if values.is_empty() {
        return Err(AnalyzeError::Empty);
    }
    if !scale.is_finite() || scale <= 0.0 {
        return Err(AnalyzeError::InvalidScale);
    }
    if fingerprint.len() != 4 {
        return Err(AnalyzeError::InvalidFingerprint);
    }

    let average = values.iter().sum::<f64>() / values.len() as f64;
    let normalized = values.iter().map(|value| value * scale).collect();
    let quality = if values.iter().any(|value| value.abs() > 100.0) {
        Quality::Noisy
    } else {
        Quality::Good
    };

    Ok(Summary {
        channel,
        quality,
        count: values.len() as u64,
        average,
        normalized,
        fingerprint: fingerprint.to_vec(),
    })
}

/// Stateful Rust behavior exposed without a second host implementation.
pub struct Analyzer {
    channel: Channel,
    scale: f64,
    calls: u64,
}

#[rspyts::export]
impl Analyzer {
    #[rspyts(constructor)]
    pub fn new(channel: Channel, scale: f64) -> Result<Self, AnalyzeError> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err(AnalyzeError::InvalidScale);
        }

        Ok(Self {
            channel,
            scale,
            calls: 0,
        })
    }

    pub fn summarize(
        &mut self,
        #[rspyts(buffer)] values: &[f64],
        #[rspyts(bytes)] fingerprint: &[u8],
    ) -> Result<Summary, AnalyzeError> {
        let result = summarize(
            self.channel.clone(),
            values,
            fingerprint,
            self.scale,
        )?;
        self.calls += 1;
        Ok(result)
    }

    pub fn calls(&self) -> u64 {
        self.calls
    }
}

rspyts::module!(native);
```

This is the only authored implementation. There is no `BridgeSummary`,
host-specific request model, conversion layer, or duplicate Python/TypeScript
algorithm.

`Type` and `Error` register the existing Rust types. `#[rspyts::export]`
exports real functions and resource methods. The `buffer` and `bytes`
annotations preserve efficient host representations, and
`rspyts::module!(native)` names the private compiled module.

## Configure both hosts

Create `rspyts.toml` in the project root:

```toml
[crate]
path = "rust"
features = []

[python]
package = "signal_contract"
mode = "source"

[typescript]
package = "@acme/signal-contract"
mode = "wasm"
```

`mode = "source"` tells rspyts to stage Python source and let Maturin compile
the project's one native extension. `mode = "wasm"` stages a complete
JavaScript/TypeScript package backed by the same Rust implementation.

The native contract probe runs before either host is built. Python-only,
TypeScript-only, and combined builds therefore extract the same semantic
contract and use the same fingerprint. The Python and WASM features are never
enabled together.

Ignore all staging output and interrupted atomic-replacement siblings:

```gitignore
.rspyts/
.rspyts.tmp-*
.rspyts.old-*
.rspyts.lock.tmp-*
.rspyts.lock.old-*
```

Do not ignore `rspyts.lock`.

## Build the packages

From the project root:

```sh
rspyts build
```

The command validates the Rust contract and atomically replaces `.rspyts/`
with the package staging inputs consumed by Maturin and npm packaging.

You can build one host when iterating:

```sh
rspyts build --target python
rspyts build --target typescript
```

Target selection changes compilation and staging work, not the semantic
fingerprint.

## What Python receives

rspyts generates frozen Pydantic models, typed exceptions, typed wrappers, and
resource lifecycle support. The generated model surface is equivalent to this
abridged excerpt:

```python
# Generated by rspyts 0.4. Do not edit.
# Abridged: imports and formatting omitted.

class Channel(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
        extra="forbid",
        frozen=True,
    )
    id: str
    sample_rate_hz: int = Field(alias="sampleRateHz")


class Quality(str, Enum):
    Good = "good"
    Noisy = "noisy"


class Summary(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
        extra="forbid",
        frozen=True,
        arbitrary_types_allowed=True,
    )
    channel: Channel
    quality: Quality
    count: int
    average: float
    normalized: NDArray[np.float64]
    fingerprint: bytes


class AnalyzeError(ContractError):
    pass
```

The generated wrappers encode boundary values, call the private native module,
translate typed Rust errors, and validate returned models. `Analyzer` becomes
a context manager with idempotent `close()`.

## Package the Python wheel

Create `python/pyproject.toml`:

```toml
[build-system]
requires = ["maturin>=1.9,<2"]
build-backend = "maturin"

[project]
name = "signal-contract"
version = "0.1.0"
requires-python = ">=3.11"
dependencies = ["numpy>=2", "pydantic>=2.11"]

[tool.maturin]
manifest-path = "../rust/Cargo.toml"
python-source = "../.rspyts/python"
module-name = "signal_contract.native"
features = ["python"]
include = [{ path = "signal_contract/**/*", format = "wheel" }]
```

Build rspyts staging first, then let Maturin compile and package the extension:

```sh
rspyts build --target python
cd python
python -m maturin build --release --out dist
cd ..
```

The resulting wheel contains the generated package and one native
`abi3-py311` extension. It has ordinary runtime dependencies on NumPy and
Pydantic, but no Python dependency named `rspyts`.

Test the artifact as a consumer, outside the source tree:

```sh
python -m venv /tmp/signal-contract-python
. /tmp/signal-contract-python/bin/activate
python -m pip install python/dist/*.whl
```

```python
import numpy as np

from signal_contract import AnalyzeError, Analyzer, Channel, Quality, summarize

channel = Channel(id="c3", sample_rate_hz=256)
result = summarize(
    channel,
    np.array([1.0, 2.0, 3.0], dtype=np.float64),
    b"\x01\x02\x03\x04",
    2.0,
)

assert result.count == 3
assert result.average == 2.0
assert result.quality is Quality.Good
np.testing.assert_array_equal(
    result.normalized,
    np.array([2.0, 4.0, 6.0], dtype=np.float64),
)

try:
    summarize(
        channel,
        np.array([], dtype=np.float64),
        b"\x01\x02\x03\x04",
        1.0,
    )
except AnalyzeError as error:
    assert error.code == "empty"

with Analyzer(channel, 0.5) as analyzer:
    result = analyzer.summarize(
        np.array([2.0, 4.0], dtype=np.float64),
        b"\x09\x08\x07\x06",
    )
    assert analyzer.calls() == 1
```

Testing an installed wheel catches missing package files, incorrect Maturin
configuration, accidental duplicate extensions, and source-tree imports that
would hide packaging defects.

## What TypeScript receives

The generated declarations preserve names, integer width, readonly structures,
typed arrays, errors, and resource lifetime:

```ts
// Generated by rspyts 0.4. Do not edit.
// Abridged: initialization support declarations omitted.

export interface Channel {
  readonly id: string;
  readonly sampleRateHz: number;
}

export enum Quality {
  Good = "good",
  Noisy = "noisy",
}

export interface Summary {
  readonly channel: Channel;
  readonly quality: Quality;
  readonly count: bigint;
  readonly average: number;
  readonly normalized: Float64Array;
  readonly fingerprint: Uint8Array;
}

export declare class AnalyzeError extends globalThis.Error {
  readonly code: string;
  constructor(message: string, code: string);
}

export function summarize(
  channel: Channel,
  values: Float64Array,
  fingerprint: Uint8Array,
  scale: number,
): Summary;

export declare class Analyzer {
  constructor(channel: Channel, scale: number);
  summarize(values: Float64Array, fingerprint: Uint8Array): Summary;
  calls(): bigint;
  free(): void;
  [Symbol.dispose](): void;
}
```

`u64` is a Python `int` and a TypeScript `bigint`; it is not silently narrowed
to JavaScript's safe `number` range. `Vec<f64>` marked as a buffer becomes a
NumPy `float64` array and a `Float64Array`. Bytes become Python `bytes` and a
`Uint8Array`.

## Package and consume the npm artifact

The staged TypeScript directory is already a package root. Pack that directory,
not generated files copied into an authored `src/` tree:

```sh
rspyts build --target typescript
mkdir -p dist/npm
npm pack .rspyts/typescript --pack-destination dist/npm
```

The tarball contains `index.js`, `index.d.ts`, `native.js`, and
`native_bg.wasm`. It has no npm dependency named `rspyts`.

Install the tarball in a browser project:

```sh
npm create vite@latest /tmp/signal-contract-web -- --template vanilla-ts
cd /tmp/signal-contract-web
npm install /absolute/path/to/signal-contract/dist/npm/acme-signal-contract-0.1.0.tgz
```

Then use the package from `src/main.ts`:

```ts
import init, {
  AnalyzeError,
  Analyzer,
  Quality,
  summarize,
} from "@acme/signal-contract";

await init();

const channel = { id: "c3", sampleRateHz: 256 };
const result = summarize(
  channel,
  new Float64Array([1, 2, 3]),
  new Uint8Array([1, 2, 3, 4]),
  2,
);

console.assert(result.count === 3n);
console.assert(result.average === 2);
console.assert(result.quality === Quality.Good);
console.assert([...result.normalized].join(",") === "2,4,6");

try {
  summarize(
    channel,
    new Float64Array(),
    new Uint8Array([1, 2, 3, 4]),
    1,
  );
} catch (error) {
  if (!(error instanceof AnalyzeError) || error.code !== "empty") {
    throw error;
  }
}

const analyzer = new Analyzer(channel, 0.5);
try {
  const analyzed = analyzer.summarize(
    new Float64Array([2, 4]),
    new Uint8Array([9, 8, 7, 6]),
  );
  console.assert(analyzer.calls() === 1n);
  console.assert([...analyzed.normalized].join(",") === "1,2");
} finally {
  analyzer.free();
}
```

Run this in a real browser during release verification. A direct import from
`.rspyts/typescript` does not prove that the npm tarball contains its
declarations, exports, or WASM asset.

## Keep Python and TypeScript from drifting

Generated code is disposable. The semantic lock is the review boundary.

When the contract is first accepted:

```sh
rspyts build
rspyts inspect
rspyts lock
git add rust rspyts.toml rspyts.lock python/pyproject.toml
```

`rspyts.lock` records:

- the Rust package and native module;
- both host package names and the TypeScript mode;
- exported types and their wire names;
- defaults and constraints;
- functions, errors, and resources;
- cross-package contract dependencies; and
- one SHA-256 semantic fingerprint.

For ordinary development and CI:

```sh
rspyts check --locked
```

If authored Rust changes the public contract, that command fails. Review and
accept the change explicitly:

```sh
rspyts inspect
rspyts lock
git diff -- rspyts.lock
git add rspyts.lock
```

Do not run `rspyts lock` automatically in CI. CI should detect an unreviewed
contract change, not approve it.

Do not commit `.rspyts/`. Rebuilding it should produce packages that match the
committed semantic lock. This keeps code review focused on the Rust API and its
meaning rather than thousands of generated lines.

## Minimal CI

This GitHub Actions job checks Rust behavior, rejects lock drift, and proves
that both release artifacts can be built:

```yaml
name: contracts

on:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read

jobs:
  contract:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown

      - uses: actions/setup-python@v5
        with:
          python-version: "3.11"

      - uses: actions/setup-node@v4
        with:
          node-version: "22"

      - name: Install contract tools
        run: |
          cargo install rspyts-cli --version '=0.4.1' --locked
          cargo install wasm-bindgen-cli --version '=0.2.126' --locked
          python -m pip install "maturin>=1.9,<2"

      - name: Test the Rust implementation
        run: cargo test --manifest-path rust/Cargo.toml --no-default-features

      - name: Reject semantic drift
        run: rspyts check --locked

      - name: Build the Python wheel
        working-directory: python
        run: python -m maturin build --release --out dist

      - name: Pack the TypeScript package
        run: |
          mkdir -p dist/npm
          npm pack .rspyts/typescript --pack-destination dist/npm
```

That is the minimum useful gate. A release pipeline should additionally install
the wheel into a clean virtual environment and install the npm tarball into a
clean browser test project, then run the same behavior assertions used above.

## The model

The whole workflow reduces to four rules:

1. Rust owns the API and behavior.
2. rspyts generates ignored host package staging.
3. Maturin and npm package that staging.
4. `rspyts.lock` is committed so CI can reject drift.

There is no second Python implementation, second TypeScript implementation,
checked-in generated client, or host-side rspyts runtime to keep synchronized.
