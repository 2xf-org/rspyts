# Reference

This is the compact lookup for the supported rspyts 0.4.1 surface.

## Packages and requirements

| Cargo package | Purpose |
| --- | --- |
| `rspyts` | Macros, contract IR, and target support |
| `rspyts-macros` | Proc-macro implementation, re-exported by `rspyts` |
| `rspyts-cli` | The `rspyts` compiler |

There is no rspyts package on PyPI or npm, and generated consumer packages do
not require one. The Rust MSRV is 1.88. Python wheels target CPython 3.11+
through `abi3-py311`. Executable TypeScript targets browser WebAssembly.

```sh
cargo install rspyts-cli --version '=0.4.1' --locked
```

## `rspyts.toml`

Unknown keys are errors. Paths are relative to `rspyts.toml`.

```toml
[crate]
path = "rust"                 # crate directory or Cargo.toml
features = ["domain"]         # optional; host artifact builds
probe-features = ["domain"]   # optional; semantic inventory build
default-features = false      # optional; defaults to false

[python]                      # optional
package = "example.contract"
mode = "source"               # source or standalone; default: standalone
source = "python/src"         # optional authored source

[typescript]                  # optional
package = "@example/contract"
mode = "wasm"                 # required: wasm or static
source = "typescript/src"     # optional authored source

[dependencies.hardware]       # optional cross-package contract
crate = "example-hardware"
lock = "../hardware/rspyts.lock"
python = "example.hardware"
typescript = "@example/hardware"
```

At least one host is required.

### Crate settings

| Key | Rule |
| --- | --- |
| `path` | Must resolve to an existing Cargo manifest. |
| `features` | Common features added when rspyts compiles a Python or WASM artifact. |
| `probe-features` | Complete feature set for the native semantic-inventory build. Defaults to `features`. |
| `default-features` | Cargo defaults are off unless explicitly enabled. |

The inventory build uses `probe-features` alone, not the union of
`probe-features` and `features`. Keep both lists backend-neutral. `python` and
`wasm` are reserved names: rspyts adds the fixed `python` feature to a
standalone extension build and `wasm` to an executable TypeScript build.
Python source mode and static TypeScript do not make those backend builds.

### Host settings

| Host mode | Output | Extra toolchain |
| --- | --- | --- |
| Python `source` | Generated Python source for a Maturin build | Maturin compiles the extension |
| Python `standalone` | Generated Python plus a compiled PyO3 extension | Native Rust toolchain |
| TypeScript `static` | Types, enums, constants, and tables | None |
| TypeScript `wasm` | TypeScript wrappers, wasm-bindgen glue, and `.wasm` | `wasm32-unknown-unknown` and wasm-bindgen CLI |

An authored `source` must be a real directory inside the project. It cannot
traverse symlinks, contain the staging directory, or collide with generated
paths. Copying skips the directories `__pycache__`, `.pytest_cache`,
`.mypy_cache`, and `.ruff_cache`, plus `.DS_Store`, `*.pyc`, and `*.pyo`
files.

For Python source mode, Maturin owns the package's only native extension:

```toml
[tool.maturin]
manifest-path = "rust/Cargo.toml"
python-source = ".rspyts/python"
module-name = "example.contract.native"
features = ["domain", "python"]
```

The final module segment is the identifier passed to `rspyts::module!`.
External packagers do not read rspyts's Cargo settings: repeat any required
`features` and default-feature policy alongside `python`.

Consumer features:

```toml
[dependencies]
rspyts = { version = "0.4.1", default-features = false }
wasm-bindgen = { version = "=0.2.126", optional = true }

[features]
python = ["rspyts/python-extension"]
wasm = ["rspyts/wasm", "dep:wasm-bindgen"]
```

`rspyts/python` is only the lower-level boundary used by Rust tests;
extensions use `rspyts/python-extension`. wasm-bindgen must be a direct
optional dependency because its macro expansion resolves the consumer crate.
The matching CLI is read from `WASM_BINDGEN` or `PATH` and should be installed
with `cargo install wasm-bindgen-cli --version '=0.2.126' --locked`.

### Contract dependencies

Each `[dependencies.ALIAS]` maps one foreign Cargo package owner to its
committed semantic lock. `crate` must match the owner in that lock. Every
dependency needs `python` when `[python]` is configured and `typescript` when
`[typescript]` is configured, even during a target-selected build.

Aliases must be unique identifiers, owners cannot be duplicated, and locks
must be relative regular files. Stale fingerprints, mismatched definitions,
undeclared transitive types, and dependency cycles are errors. Depend on
`rspyts.lock`, never another package's `.rspyts/` directory.

### Versions and fingerprints

The Cargo package version becomes `manifest.crateVersion`, is retained in
`rspyts.lock`, and becomes the generated npm package version. rspyts does not
set a Python distribution version; that comes from the consumer's Python
packager configuration, such as `[project].version` in `pyproject.toml`.

Package versions and Rust documentation are recorded but excluded from the
semantic SHA-256 fingerprint. The fingerprint covers the root contract,
configured host identities and TypeScript mode, and dependency mappings. The
lock and generated `contract.json` contain one top-level fingerprint plus each
imported package's lock fingerprint under `dependencies`; changing an imported
contract therefore changes the consumer fingerprint. Generated
`CONTRACT_FINGERPRINT` is the top-level value; imported host packages retain
their own fingerprint constants.

## Commands

Every command accepts `--config PATH` (default: `rspyts.toml`).

| Command | Result |
| --- | --- |
| `rspyts build` | Validate and atomically replace `.rspyts/`. |
| `rspyts build --target python` | Build only the configured Python host. |
| `rspyts build --target typescript` | Build only the configured TypeScript host. |
| `rspyts build --staging PATH` | Replace an explicit staging directory. |
| `rspyts check` | Build and validate the contract. |
| `rspyts check --locked` | Also require exact semantic agreement with `rspyts.lock`. |
| `rspyts check --target HOST` | Check only `python` or `typescript`. |
| `rspyts inspect` | Print the resolved manifest and fingerprint. |
| `rspyts lock` | Atomically replace `rspyts.lock`. |
| `rspyts clean` | Remove the project's default `.rspyts/` directory. |

Commands print JSON reports to stdout and diagnostics to stderr. `--target`
defaults to `all`. Every target build first extracts the same native semantic
inventory, so Python and TypeScript share one fingerprint without enabling
their backend features together. Target selection does not relax configured
dependency mappings.

A build replaces its whole staging directory; target builds do not merge.
Parallel jobs need distinct paths:

```sh
rspyts build --target python --staging .rspyts/python-job
rspyts build --target typescript --staging .rspyts/typescript-job
```

## Rust contract surface

### `#[derive(rspyts::Type)]`

Registers the existing Rust type. Supported shapes:

- named-field structs;
- one-field tuple newtypes with `#[serde(transparent)]`;
- unit-variant string enums;
- internally tagged enums with named fields;
- an explicit serialized alias through `#[rspyts(wire = T)]`.

Host shapes recognize Serde `rename`, `rename_all`, `rename_all_fields`, `tag`,
and `transparent`. `deny_unknown_fields`, field/variant `alias`,
`skip_serializing_if`, and `default` are accepted where applicable, but do not
independently widen the generated host shape. rspyts rejects ambiguous or
unsupported Serde representations.

### `#[derive(rspyts::Error)]`

Registers an error struct or enum as typed host exceptions. The stable code is
derived from the error type or enum variant, and the Rust type must implement
`Display`. Error fields remain Rust implementation detail; structured host
error payloads are not supported.

### `#[rspyts::export]`

Exports a public function, `const`, `static`, or inherent `impl`.

Functions and resources use these targets:

| Form | Function/resource target |
| --- | --- |
| `#[rspyts::export]` | Python and executable TypeScript |
| `#[rspyts::export(python)]` | Python only |
| `#[rspyts::export(typescript)]` or `#[rspyts::export(wasm)]` | Executable TypeScript only |
| `#[rspyts::export(static)]` | Rejected |

Functions must be concrete, synchronous, safe, non-variadic public functions
with simple identifier parameters. For a one-parameter result alias such as
`DomainResult<T>`, add `#[rspyts(error = crate::DomainError)]`. Ordinary
`Result<T, E>` already carries its error type.

An exported inherent `impl` defines an opaque resource:

- at least one public method has `#[rspyts(constructor)]`;
- public `&self` and `&mut self` methods are exported;
- `#[rspyts(skip)]` omits a public method;
- `#[rspyts(python)]`, `#[rspyts(typescript)]`, and `#[rspyts(wasm)]` scope members;
- consuming-`self` methods are rejected;
- `close` and `free` are reserved lifecycle names.

Every exported host must retain a constructor after filtering. Generated
Python resources have idempotent `close()` and context-manager cleanup.
TypeScript resources have idempotent `free()` and `Symbol.dispose`. Calls
after disposal fail.

Constants and statics use a different target rule:

| Form | Constant/static target |
| --- | --- |
| `#[rspyts::export]` | Python and either TypeScript mode |
| `#[rspyts::export(python)]` | Python only |
| `#[rspyts::export(typescript)]` or `#[rspyts::export(wasm)]` | Either TypeScript mode |
| `#[rspyts::export(static)]` | Static TypeScript mode only |

Static mode emits types and selected constants; it never translates function
bodies.

### `rspyts::module!`

Declare exactly one generated native module:

```rust
rspyts::module!(native);
rspyts::module!(native, python);
rspyts::module!(native, typescript);
```

The qualifier scopes wrappers, not the semantic probe. Scope the exports too
when a crate is intentionally single-host.

## Types and annotations

| Rust | Python | TypeScript |
| --- | --- | --- |
| `()` | `None` | `void` |
| `bool` | `bool` | `boolean` |
| `i8..i32`, `u8..u32` | checked `int` | checked `number` |
| `i64`, `u64` | `int` | `bigint` |
| `f32`, `f64` | `float` | `number` |
| `String`, `&str` | `str` | `string` |
| `Option<T>` | `T \| None` | `T \| null` |
| `Vec<T>`, slices | sequence | `ReadonlyArray<T>` |
| string-keyed `BTreeMap`/`HashMap` | mapping | readonly object |
| tuples of 2–8 items | tuple | readonly tuple |
| `serde_json::Value` | recursive JSON | `JsonValue` |
| aware `chrono::DateTime` | Pydantic `AwareDatetime` | RFC 3339 string |
| derived types | generated model/enum | generated interface/enum/union |

`usize` and `isize` are not contract types.

| Annotation | Use |
| --- | --- |
| `#[rspyts(bytes)]` | Byte wire policy: Python `bytes`, TypeScript `Uint8Array` |
| `#[rspyts(buffer)]` | Numeric storage as NumPy/JavaScript typed arrays |
| `#[rspyts(returns(bytes))]` | Bytes return policy on a function or method |
| `#[rspyts(returns(buffer))]` | Numeric-buffer return policy |
| `#[rspyts(required)]` | Require an `Option<T>` key while still allowing null |
| `#[rspyts(literal = V)]` | Exact bool, signed-64-bit integer, or string |
| `#[rspyts(default = V)]` | Host default for a bool, integer, or string |
| `#[rspyts(min_length = N)]` | String scalar-count or list length minimum |
| `#[rspyts(max_length = N)]` | String scalar-count or list length maximum |
| `#[rspyts(ge = N)]` | Inclusive integer minimum |
| `#[rspyts(wire = T)]` | Declared serialized shape for custom Serde |

Non-optional fields are required. `Option<T>` is optional unless marked
`required`. A non-optional Serde default needs the matching explicit rspyts
default. Literal/default values must agree and satisfy constraints.

The intended byte carrier convention is `Vec<u8>` or `&[u8]`, but the macro
currently records the byte policy without statically proving the Rust carrier.
Its actual serialization must match bytes or an unsigned-byte sequence. Bytes
and buffers are owned at the boundary. Buffers preserve dtype and count, but
not dimensions or strides. Structured floats must be finite; buffers may
contain IEEE NaN and infinity. TypeScript values are deeply readonly and
frozen, except non-empty typed arrays, which JavaScript cannot freeze.

## Generated files

| Path | Source-control policy |
| --- | --- |
| `rspyts.toml` | Commit |
| `rspyts.lock` | Commit and review as the semantic API |
| `.rspyts/**` | Ignore; generated staging |
| `.rspyts.tmp-*`, `.rspyts.old-*` | Ignore; atomic directory siblings |
| `.rspyts.lock.tmp-*`, `.rspyts.lock.old-*` | Ignore; atomic lock siblings |

Generated Python, TypeScript, JavaScript, and WASM belong inside built consumer
artifacts, not the source tree. `rspyts.lock` schema 2 is deterministic,
pretty-printed JSON.

## Deliberate limits

rspyts 0.4 does not support generics, async functions, callbacks, iterators,
streams, `impl Trait`, borrowed host lifetimes, zero-copy Rust memory, arbitrary
custom Serde without `wire`, untagged/flattened shapes, a generic C ABI, Node
native bindings, or body translation to static TypeScript.

Native file APIs should be Python-only. Browser APIs should accept owned,
portable data. A declared `wire` shape requires a consumer-owned Rust
serialization round-trip test.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Config rejected | Unknown key, invalid package name, missing host, backend feature in crate lists, or unsafe `source` path. |
| Cargo manifest missing | `crate.path` is relative to `rspyts.toml` and may name a directory or manifest. |
| `check --locked` fails | Run `rspyts inspect`, review the Rust API and lock diff, then run `rspyts lock` only if intentional. |
| Generated files are tracked | Remove them and add all five `.rspyts*` ignore patterns above. |
| Python has unresolved symbols | Use `rspyts/python-extension`, not `rspyts/python`, for a wheel extension. |
| Wheel contains two extensions | Use Python `source` mode when Maturin compiles the extension. |
| Installed Python import fails | Test the built wheel in a clean environment; verify Maturin's `python-source` and `module-name`. |
| WASM build cannot start | Install the target and matching wasm-bindgen CLI, or set `WASM_BINDGEN`. |
| Browser cannot load `.wasm` | Test the packed npm artifact and verify its exports/files include the generated asset. |
| Host shape is wrong | Correct Serde metadata or declare the real `wire` shape; do not add a mirror DTO. |
| Wrong buffer dtype | Use `buffer` for numeric arrays and `bytes` for binary data; annotate returns separately. |
| Targets produce different expectations | Keep `probe-features` complete and backend-neutral; target selection is packaging, not a separate contract. |
