# 🌉 rspyts

<p align="center">
  <a href="https://github.com/2xf-org/rspyts/actions/workflows/validation.yml">
    <img src="https://github.com/2xf-org/rspyts/actions/workflows/validation.yml/badge.svg">
  </a>
  <a href="https://opensource.org/licenses/MIT">
    <img src="https://img.shields.io/badge/License-MIT-yellow.svg">
  </a>
  <a href="https://crates.io/crates/rspyts">
    <img src="https://img.shields.io/crates/v/rspyts.svg">
  </a>
  <a href="https://pypi.org/project/rspyts/">
    <img src="https://img.shields.io/pypi/v/rspyts.svg">
  </a>
  <a href="https://www.npmjs.com/package/rspyts">
    <img src="https://img.shields.io/npm/v/rspyts.svg">
  </a>
</p>

Define it once in Rust, call it from Python and TypeScript.

## Installing

```
cargo add rspyts             # in your Rust crate
cargo install rspyts-cli     # the `rspyts` binary
pip install rspyts           # Python runtime
npm install rspyts           # TypeScript runtime
```

Note that your crate's `crate-type` must include `"cdylib"`, and that `serde` (with the `derive` feature) is required as a direct dependency.

## Documentation

Documentation can be found [**here**](docs/). It covers the quickstart, how rspyts works, the Python and TypeScript guides, the architecture, and the normative design specs.

## Using

Annotate the Rust you want to share:

```rust
use rspyts::bridge;

#[bridge]
/// Summary statistics for a list of numbers.
pub struct Summary {
    pub item_count: u32,
    pub total: f64,
    pub average: f64,
    pub label: Option<String>,
}

#[bridge]
/// Summarize a list of numbers.
pub fn summarize(values: &[f64], label: Option<String>) -> Result<Summary, BasicError> {
    // ...
}

rspyts::export!();
```

Run `rspyts generate`. Call it from Python:

```python
import numpy as np
from basic_example.generated import summarize

summary = summarize(np.array([2.0, 4.0, 6.0]), "demo")
print(summary.average, summary.item_count)  # 4.0 3
```

And from TypeScript:

```ts
const client = createClient(await instantiate(await readFile(wasmPath)));
console.log(client.summarize(new Float64Array([2, 4, 6]), "demo").average); // 4
```

`BasicError` is a `#[bridge(error)]` enum that becomes real exception classes on both sides. The full crate — including a stateful `Counter` class held by handle — lives at [examples/basic](examples/basic).

## Questions & Answers

**How does this project differ from** `PyO3` **+** `wasm-bindgen`**?** Depth. PyO3 and wasm-bindgen are excellent, deep integrations into CPython and the JS host — tens of thousands of lines each, with their own semantics for ownership, errors, and garbage collection. rspyts needs less: move typed data, call functions, hold opaque state. Everything crosses one small, hand-specified C ABI, and both runtimes are thin consumers of it (Python via stock `ctypes`, TypeScript via plain WASM exports). The trade is no fine-grained object integration — you cannot subclass a Rust type in Python.

**What can cross the boundary?** Structs, string enums, tagged data enums, typed errors, `Option`, `Vec`, string-keyed maps, constants (real importable values in both languages), `rspyts::Json` for schemaless payloads, and bulk numeric data as raw buffers (`&[T]` in, `Buf<T>` out — numpy arrays and JS typed arrays, no JSON in the hot path). Stateful objects stay in Rust behind opaque handles, built by a constructor or by static factories (`Recording::open`-style, including factory-only classes). Types shared between bridged crates are imported, not duplicated.

**What is not supported?** `u64`/`i64` (JSON's 2^53 ceiling), async, callbacks, datetimes, UUIDs, tuples, and generics. Each is a compile-time error at the Rust definition site, never a runtime surprise. The reasoning is written down in [docs/design/decisions.md](docs/design/decisions.md).

## License

This project is licensed under the [MIT license](LICENSE).
