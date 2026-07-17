# Troubleshooting

## `rspyts.toml` is rejected

The file denies unknown keys. Check section names, use a dot-separated Python
package, and use either a normal or scoped npm package name. Python mode is
exactly `source` or `standalone`; TypeScript mode is exactly `static` or
`wasm`. At least one host section is required.

Cargo default features are disabled unless `[crate]` explicitly sets
`default-features = true`. Optional `source` paths must be relative directories
inside the project and may not traverse symlinks.

## The Cargo manifest cannot be found

`[crate].path` resolves relative to `rspyts.toml`. It may point to a crate
directory or directly to `Cargo.toml`.

## `check --locked` reports a mismatch

Inspect the change before accepting it:

```sh
rspyts inspect > /tmp/current-contract.json
git diff -- rspyts.lock
```

If the Rust public API change is intentional, run `rspyts lock` and review the
new lock. Never make CI update the lock automatically.

## A generated file appears in Git

Remove it from the tracked source tree and include `.rspyts/`,
`.rspyts.tmp-*`, `.rspyts.old-*`, `.rspyts.lock.tmp-*`, and
`.rspyts.lock.old-*` in `.gitignore`.
Packaging should consume `rspyts build` output directly. `rspyts.lock` is the
only generated contract file intended for source control.

## Python imports work only from the source tree

Build the wheel, install it into a clean environment, and test the installed
package. Ensure the Maturin configuration includes the staged generated
package and native extension. The clean environment should not install rspyts.

## The Python extension has unresolved Python symbols

Make the consumer's fixed `python` feature enable
`rspyts/python-extension`, not only `rspyts/python`. The extension feature adds
the required PyO3 link mode; the lower-level feature is intended for Rust tests
that do not build an extension module.

## The wheel contains two native extensions

Use `[python].mode = "source"` for Maturin packaging. Source mode stages the
Python package but leaves extension compilation entirely to Maturin. Standalone
mode already compiles and stages an extension and must not be combined with a
second Maturin extension build.

## The browser cannot find the WASM asset

Test the packed npm artifact rather than the source directory. Confirm the
wasm-bindgen JavaScript and `.wasm` file are both included and that the bundler
preserves the generated asset URL.

## `wasm-bindgen` is not installed

WASM mode needs both the consumer's optional Rust dependency and a matching
wasm-bindgen CLI on the build machine. Install the CLI or set `WASM_BINDGEN` to
its executable. Static TypeScript mode does not invoke wasm-bindgen.

## A custom Serde type has the wrong host shape

Add an explicit `#[rspyts(wire = T)]` declaration only when it describes the
type's real serialized representation. Add a Rust round-trip test covering
valid and invalid values. Do not solve the mismatch with a mirror DTO.

## A Serde default is rejected

A non-optional field with `#[serde(default)]` or
`#[serde(default = "path")]` also needs an explicit scalar
`#[rspyts(default = ...)]`. The compiler cannot infer an arbitrary Rust
function's result. Keep the declared bool, integer, or string equal to the Rust
default and cover that agreement with a Rust test.

## A buffer has the wrong host dtype

Use `#[rspyts(buffer)]` on numeric storage and `#[rspyts(bytes)]` on binary
data. An unannotated `Vec<T>` is a normal sequence. The two annotations are
semantically distinct even when `T` is `u8`.

For return values, put `#[rspyts(returns(buffer))]` or
`#[rspyts(returns(bytes))]` on the exported function or resource method. A
parameter or field annotation does not describe the return value.

## A target-selected build changed the lock

`--target` selects packaging work, not semantic extraction. The compiler loads
one manifest using `[crate].probe-features`, so all targets should produce the
same fingerprint. Check that the probe features expose the complete domain
inventory without enabling Python or WASM. Target-specific `cfg` gates may hide
host behavior, but they may not create incompatible data contracts.
