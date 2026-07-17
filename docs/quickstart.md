# Quickstart

This example builds one Rust contract for Python and TypeScript. A real project
may configure either target alone.

## 1. Install the compiler

```sh
cargo install rspyts-cli --version =0.4.1 --locked
```

During development in this repository, use:

```sh
cargo install --path crates/rspyts-cli --locked
```

The CLI is a build tool. Do not add `rspyts` to Python requirements or npm
dependencies.

## 2. Add the Rust dependency

```toml
[lib]
crate-type = ["rlib", "cdylib"]

[dependencies]
rspyts = { version = "0.4.1", default-features = false }
serde = { version = "1", features = ["derive"] }
thiserror = "2"
chrono = { version = "0.4", default-features = false, features = ["serde", "std"] }
wasm-bindgen = { version = "0.2.126", optional = true }

[features]
default = []
python = ["rspyts/python-extension"]
wasm = ["rspyts/wasm", "dep:wasm-bindgen"]
```

Keep native file I/O and other non-WASM dependencies behind the project's
native/Python feature. rspyts invokes Cargo with `--no-default-features` unless
`default-features = true` is explicitly configured in `rspyts.toml`.

`rspyts/python-extension` enables the PyO3 `abi3-py311` boundary and correct
extension-module link mode for the consumer cdylib. The lower-level
`rspyts/python` feature is for non-extension Rust tests; it is not sufficient
for a wheel's native library. Neither feature adds a Python runtime dependency.

The direct optional wasm-bindgen dependency is required only for the `wasm`
feature. PyO3 lets generated attributes name rspyts's hidden re-export, but
wasm-bindgen has no equivalent crate-path override and its generated code must
resolve `wasm-bindgen` in the consumer crate. This is a Cargo build dependency,
not an npm runtime dependency.

Executable TypeScript builds also require the matching wasm-bindgen CLI on the
build machine:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.126 --locked
```

## 3. Export the real API

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rspyts::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Summary {
    pub count: u64,
    pub average: f64,
}

#[derive(Debug, thiserror::Error, rspyts::Error)]
pub enum SummaryError {
    #[error("values cannot be empty")]
    Empty,
}

#[rspyts::export]
pub fn summarize(values: Vec<f64>) -> Result<Summary, SummaryError> {
    if values.is_empty() {
        return Err(SummaryError::Empty);
    }

    Ok(Summary {
        count: values.len() as u64,
        average: values.iter().sum::<f64>() / values.len() as f64,
    })
}

#[rspyts::export]
#[rspyts(returns(buffer))]
pub fn normalize(#[rspyts(buffer)] values: &[f64]) -> Result<Vec<f64>, SummaryError> {
    if values.is_empty() {
        return Err(SummaryError::Empty);
    }

    let total = values.iter().sum::<f64>();
    Ok(values.iter().map(|value| value / total).collect())
}

rspyts::module!(native);
```

This is the public domain API. Do not add `BridgeSummary`, conversion traits,
or host-specific DTOs beside it.

Field defaults and validation rules belong on the same Rust model. For
example, this contract requires version 2, defaults `quantity` to 1, rejects
empty or oversized actors, and carries an aware timestamp:

```rust
fn default_quantity() -> u32 { 1 }

#[derive(serde::Serialize, serde::Deserialize, rspyts::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    #[rspyts(literal = 2)]
    pub contract_version: u32,
    #[rspyts(min_length = 1, max_length = 32)]
    pub actor: String,
    #[serde(default = "default_quantity")]
    #[rspyts(default = 1, ge = 1)]
    pub quantity: u32,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}
```

Python receives a frozen Pydantic model with an `AwareDatetime`; TypeScript
receives a readonly shape with an RFC 3339 string, a literal `2`, and an
optional defaulted `quantity`. Generated boundary code applies the same rules
before Rust runs. The explicit rspyts default must match the Rust Serde default.

## 4. Configure the build

Create `rspyts.toml` beside the language package directories:

```toml
[crate]
path = "rust"
features = []
# probe-features = [] # defaults to features; keep this backend-neutral
# default-features = true # opt in only when the domain crate requires it

[python]
package = "example_contract"
mode = "source"
# source = "python/src" # optional authored source copied into staging

[typescript]
package = "@example/contract"
mode = "wasm"
# source = "typescript/src" # optional authored source copied into staging
```

`path` may name a crate directory or its `Cargo.toml`. TypeScript `mode` is
`wasm` for executable Rust or `static` for types, constants, and tables only.
Static mode does not need wasm-bindgen or the consumer's `wasm` feature.

`[crate].features` are common to every selected host. `probe-features` is the
complete backend-neutral feature set used only to expose the native inventory;
it defaults to the common list. It must not include Python or WASM boundary
features. The CLI automatically adds the consumer's fixed `python` feature for
a standalone Python artifact and its fixed `wasm` feature for executable
TypeScript. For example, Hardware uses common `features = []` and
`probe-features = ["native", "formats"]`; its own `python` and `wasm` Cargo
features select the appropriate backend and domain dependencies.

Python `mode = "source"` is the normal Maturin workflow: rspyts generates and
stages Python source but does not compile or copy an extension. Maturin compiles
the consumer crate once with its fixed `python` feature. Use
`mode = "standalone"` only when rspyts should compile and stage the generic
extension itself.

Add this rule to the consuming repository's `.gitignore`:

```gitignore
.rspyts/
.rspyts.tmp-*
.rspyts.old-*
.rspyts.lock.tmp-*
.rspyts.lock.old-*
```

The sibling patterns cover interrupted atomic staging replacements. Do not
ignore `rspyts.lock`.

## 5. Build and accept the contract

```sh
rspyts build
rspyts inspect
rspyts lock
git add rspyts.toml rspyts.lock
```

`--target` selects which host artifact is staged; omit it to build all
configured hosts. Contract extraction always compiles the same backend-neutral
native probe first, so target-selected builds cannot produce host-specific
semantic locks. It never enables Python and WASM together. Every build
atomically replaces its whole staging directory, so independently scheduled
targets use distinct explicit paths:

```sh
rspyts build --target python --staging .rspyts/python-job
rspyts build --target typescript --staging .rspyts/typescript-job
```

`build` writes only below `.rspyts/` unless another `--staging` path is
supplied. `lock` records the normalized semantic manifest and fingerprint as
deterministic, pretty-printed JSON so contract reviews see semantic diffs.

Use the locked check in CI:

```sh
rspyts check --locked
```

A changed lock is a public API review, not generated-source churn.

## 6. Package the result

In source mode, Maturin builds the consumer crate's sole PyO3 extension and
includes the staged Python package. TypeScript packaging includes either the
static output or the wasm-bindgen JavaScript, declarations, and `.wasm` asset. See
[Packaging](packaging.md).

The final wheel or npm package is self-contained with respect to rspyts. Test
the installed wheel and an installed `npm pack` tarball, not imports from the
staging directory. The artifacts must not declare a Python or npm runtime
dependency on rspyts.
