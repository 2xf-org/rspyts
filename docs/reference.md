# Reference

This reference describes the intentionally constrained rspyts 0.4 product.

## Requirements

| Component | Supported value |
| --- | --- |
| Rust and Cargo | One workspace pinned to `1.88.0` |
| rspyts crates and CLI | One exact matching version, currently `0.4.6` |
| Python artifact | Generated source packaged as one CPython 3.11+ abi3 wheel |
| TypeScript runtime | Browser WASM built with wasm-bindgen `0.2.126` |
| Static TypeScript | ESM, declarations, and JSON-safe values |
| Contract dependency | Zero or one direct leaf lock |

Install rspyts from Cargo. Generated artifacts contain no rspyts runtime
dependency.

## `rspyts.toml`

Unknown keys are errors. All filesystem paths are non-empty, relative to
`rspyts.toml`, contained in the pinned workspace, and free of symlink traversal.

```toml
[crate]
path = "rust"
features = ["bindings"]        # optional common artifact features
probe-features = ["bindings"]  # optional semantic inventory features
default-features = false       # optional; default false

[python]
package = "example.contract"
source = "python/src"          # optional authored source tree

[typescript]
package = "@example/contract"
mode = "wasm"                  # wasm or static

[dependencies.owner]           # optional; at most one
crate = "example-owner"
lock = "../owner/rspyts.lock"
python = "example.owner"
typescript = "@example/owner"
```

At least Python and one TypeScript mode are used by the supported package
shapes. Python has no mode: rspyts always emits source for Maturin. `source`,
when present, is copied into the fixed `.rspyts/python` output before generated
modules are written. It is not an output override.

### Crate

`crate.path` identifies one workspace member containing one library contract.
The library must include `crate-type = ["rlib", "cdylib"]` and declare exactly
one `rspyts::module!`.

`features` are enabled for host artifacts. `probe-features` define the complete
backend-neutral semantic inventory and default to `features`. rspyts adds only
the fixed `python` or `wasm` backend feature. The workspace owns one toolchain,
Cargo lock, target settings, and dependency resolution.

rspyts does not provide a second toolchain resolver and does not support custom
Cargo config/include graphs, compiler wrappers, target/build directory schemes,
or switching toolchains between probe and artifact builds.

### Python

`package` is a dot-separated Python package. Each segment must be a public
identifier, not a Python keyword, and not a Pydantic or generated runtime
collision. Leading-underscore and `model_*` names are rejected when they would
become generated members.

`source` is optional authored Python inside the project. It may add ordinary
package files but cannot replace generated modules such as `models.py`,
`functions.py`, `resources.py`, `constants.py`, `errors.py`, or `codecs.py`.

Maturin owns the one native extension:

```toml
[tool.maturin]
manifest-path = "../rust/Cargo.toml"
python-source = "../.rspyts/python"
module-name = "example.contract.native"
features = ["python"]
```

The final module segment must equal the identifier passed to
`rspyts::module!`. Generated wheels declare normal host dependencies such as
Pydantic or NumPy, never a Python `rspyts` runtime.

### TypeScript

`mode = "wasm"` emits executable browser wrappers, declarations, the compiled
WebAssembly module, and a canonical `./wire` export. Install the exact matching
CLI on `PATH`:

```sh
cargo install wasm-bindgen-cli --version '=0.2.126' --locked
```

`mode = "static"` emits types, enums, constants, and tables. It never translates
Rust function bodies. A static contract rejects TypeScript-targeted functions
and resources.

Package names must be valid npm package names and must not collide with emitted
globals or exports. rspyts rejects unsafe names rather than sanitizing them.

### Direct owner

A consumer may configure one `[dependencies.ALIAS]`. The lock must describe a
leaf contract with no dependency of its own. `crate` must equal the owner's Cargo
package name; the host names must equal the owner lock; and the exact crate
version and fingerprint are retained in the consumer lock.

The supported consumer shape is Python source plus static TypeScript importing
one direct WASM owner. Python imports the owner's generated classes. TypeScript
imports the owner's complete JSON-safe types from `./wire`. Neither host copies
foreign definitions.

One Cargo package name resolves to one exact version. Aliases do not create new
owners or permit simultaneous versions. Stale fingerprints, undeclared imports,
duplicate host identities, transitive locks, and cycles are errors.

## Commands

Every command accepts `--config PATH`; the default is `rspyts.toml`.

| Command | Result |
| --- | --- |
| `rspyts build` | Validate and replace the fixed `.rspyts/` output. |
| `rspyts build --target python` | Prepare only generated Python source. |
| `rspyts build --target typescript` | Prepare only configured TypeScript output. |
| `rspyts check --locked` | Require the authored contract and exact metadata to match the lock. |
| `rspyts inspect` | Print the resolved manifest and fingerprint. |
| `rspyts lock` | Atomically accept the current semantic contract. |
| `rspyts clean` | Remove the fixed `.rspyts/` output. |

Target-specific builds replace `.rspyts/`; they do not merge with an earlier
target build. Use the default build when packaging both hosts.

## Rust surface

### Types and errors

`#[derive(rspyts::Type)]` supports named-field structs, transparent one-field
newtypes, unit-variant string enums, internally tagged enums with named fields,
and explicit `#[rspyts(wire = T)]` serialized aliases.

`#[derive(rspyts::Error)]` creates a typed host exception from a Rust error
struct or enum. Error fields remain implementation detail. Boundary validation
uses the declared error class with code `invalid_argument`.

Supported portable values include:

| Rust | Python | Static TypeScript | WASM TypeScript |
| --- | --- | --- | --- |
| `bool`, strings | native scalar | native scalar | native scalar |
| `i8..i32`, `u8..u32` | checked `int` | checked `number` | checked `number` |
| `i64`, `u64` | checked `int` | only exact safe values | `bigint` |
| `f32`, `f64` | finite `float` | finite `number` | finite `number` |
| `Option<T>` | `T | None` | `T | null` | `T | null` |
| vectors and slices | sequence | readonly array | readonly array |
| `[u8; N]` with `bytes` | length-checked `bytes` | exact number tuple | checked `Uint8Array` |
| numeric `buffer` | NumPy array | readonly number array | typed array |
| aware `chrono::DateTime` | Pydantic `AwareDatetime` | RFC 3339 string | RFC 3339 string |

Maps require string keys. Tuples support two through eight items. JSON numbers
must be finite; integer-valued JSON numbers must fit JavaScript's safe range.

Host names follow Serde's canonical `rename`, `rename_all`, `tag`, and supported
default rules. Aliases, flattening, untagged enums, directional names, and
automatic repair of invalid or colliding names are rejected.

Field contracts support scalar `literal` and `default` values, string/list
`min_length` and `max_length`, and integer `ge` and `le` bounds. Generated
Pydantic models are strict, so booleans, strings, and floats are not coerced
into integer fields.

### Exports

`#[rspyts::export]` exposes a function, constant, static, or inherent `impl` to
its valid configured hosts. Target qualifiers narrow the export:

| Form | Surface |
| --- | --- |
| `#[rspyts::export]` | Python and configured TypeScript |
| `#[rspyts::export(python)]` | Python only |
| `#[rspyts::export(wasm)]` | Executable TypeScript only |
| `#[rspyts::export(static)]` | Static TypeScript root or WASM `./wire` |

Functions are public, concrete, synchronous, safe, and non-variadic. An
exported inherent `impl` defines an opaque resource with at least one marked
constructor. Python resources have idempotent `close()` and context management;
WASM resources have idempotent `free()`. Those lifecycle names are reserved.

## Discovery ABI v1

An executable contract normally uses both host capabilities:

```rust
rspyts::module!(native);
```

A Python-plus-static contract may declare only the Python runtime capability:

```rust
rspyts::module!(native, python);
```

The module emits package-scoped native discovery symbols:

```text
rspyts_discovery_v1_contract__<cargo-package-name>
rspyts_discovery_v1_contract_free__<cargo-package-name>
```

The first returns an owned status, capability mask, and UTF-8 payload. Every
non-null payload is released exactly once through the matching free symbol,
including ordinary error payloads. Panics are contained and return a null
payload. The CLI and crate must be the same exact rspyts version.

## Locks, output, and privacy

| Path | Policy |
| --- | --- |
| `rspyts.toml` | Commit. |
| `rspyts.lock` | Commit the compact semantic snapshot. |
| `.rspyts/**` | Ignore generated Python, JavaScript, declarations, native code, and WASM. |
| `.rspyts.tmp-*`, `.rspyts.old-*` | Ignore transactional siblings. |
| `.rspyts.build.lock` | Ignore the project command lock. |
| `.rspyts.lock.tmp-*` | Ignore lock temporary files. |

The lock records host identities, exact package versions, definitions,
constraints, the optional direct owner snapshot, and the SHA-256 fingerprint.
Generated owner guards run at Python or JavaScript import time; npm artifacts
must retain those side effects.

Native and WASM release artifacts must not expose checkout paths. rspyts remaps
its own builds and strips WASM debug information. External Maturin builds must
remap the physical workspace root and disable or sanitize path-bearing SBOMs as
shown in the executable guide. Release source, exact crate archives, wheels,
npm tarballs, and release notes are scanned for protected content.

## Deliberate limits

rspyts 0.4 does not support standalone Python, custom output directories,
absolute or out-of-workspace contract paths, multiple contract dependencies,
transitive contract graphs, simultaneous versions of one package name, custom
Cargo toolchains/config/include graphs/compiler wrappers, hostile-name
sanitization, generics, async, callbacks, streams, borrowed host lifetimes,
zero-copy Rust memory, arbitrary custom Serde, Node native bindings, or function
translation to static TypeScript.
