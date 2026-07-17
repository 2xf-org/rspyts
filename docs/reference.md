# Reference

## Cargo packages

| Package | Purpose |
| --- | --- |
| `rspyts` | Public macros, semantic IR, contract registry, and target-gated boundary support |
| `rspyts-macros` | Proc-macro implementation; consumed through `rspyts` |
| `rspyts-cli` | Contract compiler, emitters, lock handling, and build orchestration |

Only these three Cargo packages are released. There is no `rspyts-core`, PyPI
distribution, npm package, or separately installed host runtime in 0.4.

## Configuration

`rspyts.toml` is strict: unknown keys are errors.

```toml
[crate]
path = "rust/Cargo.toml" # or the containing directory
features = ["native"]    # optional features common to every Cargo invocation
probe-features = ["native", "formats"] # optional; defaults to features
default-features = false # default; true is an explicit opt-in

[python]                 # optional
package = "example.contract"
mode = "source"         # "source" or "standalone"
source = "python/src"    # optional authored source directory

[typescript]             # optional
package = "@example/contract"
mode = "wasm"            # "wasm" or "static"
source = "typescript/src" # optional authored source directory

[dependencies.hardware]  # optional imported Rust contract owner
crate = "neurovirtual-hardware"
lock = "../hardware/rspyts.lock"
python = "neurovirtual.hardware"
typescript = "@neurovirtual/hardware"
```

At least one host section is required. Relative paths resolve from the
directory containing `rspyts.toml`.

`[crate].features` is the common feature set for every selected host build.
`probe-features` defaults to that common set; when specified, it is the complete
feature set used only for target-independent native inventory extraction. Keep
it backend-neutral. In particular, do not put a PyO3 or wasm-bindgen boundary
feature in `probe-features`.

The CLI automatically adds the consumer's Cargo feature named `python` when it
compiles a standalone Python extension, and `wasm` when it compiles executable
TypeScript. Those two names are the fixed v0.4 convention; there are no
per-host `features` keys in `rspyts.toml`. Cargo default features are disabled
unless the configuration explicitly sets `default-features = true`.

`source` is resolved relative to `rspyts.toml` and copied into that host's
temporary staging tree before generated files are written. It must be a real
in-project directory with no symlink traversal. A generated/authored path
collision is a hard error; rspyts never overwrites authored source.

Each `[dependencies.ALIAS]` table maps one foreign Cargo package owner to its
committed semantic lock and host package names. The alias is only a stable local
configuration key. `crate` must exactly match the owner recorded by the macros,
and `lock` resolves relative to `rspyts.toml`. A host mapping is required when
the consuming output references that dependency in the host. Lock schema 2
records and fingerprints these mappings. Missing owners, stale fingerprints,
shape mismatches, duplicate owner aliases, undeclared transitive types, and
dependency cycles are errors. A dependency never points at another package's
`.rspyts/` staging directory.

If `mode` is omitted, Python defaults to `standalone`. Python
`mode = "source"` stages generated and optional authored Python source,
but never compiles or copies a native extension. This is the Maturin mode:
Maturin enables the fixed `python` feature and produces the package's sole
ABI-tagged extension. `mode = "standalone"` tells rspyts to compile and stage
the consumer crate's PyO3 extension itself. Do not combine standalone mode with
a second Maturin-built extension in the same wheel.

In source mode, Maturin's module name must append the identifier passed to
`rspyts::module!` to the configured Python package:

```toml
[tool.maturin]
manifest-path = "rust/Cargo.toml"
python-source = ".rspyts/python"
module-name = "example.contract.native"
features = ["python"]
```

Python consumer cdylibs enable rspyts's extension feature:

```toml
[dependencies]
rspyts = { version = "0.4.1", default-features = false }

[features]
python = ["rspyts/python-extension"]
```

`rspyts/python-extension` includes the `abi3-py311` PyO3 boundary and
extension-module link mode. Use the lower-level `rspyts/python` feature only in
non-extension Rust tests. The consumer does not need a direct PyO3 dependency
solely for rspyts, and neither feature creates an installed Python dependency.

WASM consumers also declare wasm-bindgen directly and enable it with the fixed
feature:

```toml
[dependencies]
rspyts = { version = "0.4.1", default-features = false }
wasm-bindgen = { version = "0.2.126", optional = true }

[features]
wasm = ["rspyts/wasm", "dep:wasm-bindgen"]
```

This is required because wasm-bindgen's expanded code resolves its own crate in
the consumer; unlike PyO3, it offers no supported crate-path override. The
dependency is compile-time Rust support and does not add an npm rspyts or
wasm-bindgen runtime dependency.

For `mode = "wasm"`, the build environment also needs a wasm-bindgen CLI
compatible with the Rust dependency and the `wasm32-unknown-unknown` Rust
target. `rspyts build` invokes `wasm-bindgen` from `PATH`, or the executable
named by `WASM_BINDGEN`. Static mode needs neither the target, CLI, nor direct
dependency.

The generated executable package's default `init()` is idempotent. Concurrent
and repeated calls share the first in-flight or successful promise, and the
first call's input wins. A rejection clears the cache so a later call can
retry.

## Commands

All commands accept `--config PATH`; the default is `rspyts.toml`.

| Command | Effect |
| --- | --- |
| `rspyts build` | Compile and validate the contract, then replace `.rspyts/` atomically |
| `rspyts build --staging PATH` | Write package staging output to an explicit build directory |
| `rspyts build --target python` | Stage only the configured Python package |
| `rspyts build --target typescript` | Stage only the configured TypeScript package |
| `rspyts check` | Build and validate the current contract |
| `rspyts check --target HOST` | Check and stage only `python` or `typescript` |
| `rspyts check --locked` | Also require an exact match with `rspyts.lock` |
| `rspyts lock` | Replace `rspyts.lock` with the current semantic contract |
| `rspyts inspect` | Print the resolved manifest and SHA-256 fingerprint |
| `rspyts clean` | Remove `.rspyts/` |

Machine-readable command output is JSON. Diagnostics and a non-zero exit code
report invalid configuration, unsupported contracts, build failures, or lock
drift.

`--target` defaults to `all` and controls host compilation and staging only.
Before any selected host is emitted, the CLI extracts one semantic manifest
from the backend-neutral native probe configured by `[crate].features` and
`probe-features`. This ensures a Python build, a TypeScript build, and an
all-host build share the same fingerprint and lock without enabling their
backend features together. Target selection never invokes the unselected
host's compiler or wasm-bindgen CLI.

Each invocation atomically replaces its complete staging directory. Do not run
two target-selected jobs against the same output and expect their files to be
merged. Use `--staging .rspyts/python-job` and
`--staging .rspyts/typescript-job`, or let one `--target all` build own the
default `.rspyts/` directory.

## Public Rust surface

### `#[derive(rspyts::Type)]`

Registers the existing struct, enum, or newtype as a contract type. It does not
create another Rust type. Supported Serde naming is reflected in the semantic
manifest rather than guessed from generated text.

The derive accepts named structs, single-field tuple newtypes marked
`#[serde(transparent)]`, unit-variant string enums, and internally tagged enums
whose variants use named fields. Generic types, unit structs, unions, ordinary
tuple structs, tuple enum variants, and ambiguous enum representations are
rejected.

### `#[derive(rspyts::Error)]`

Registers an existing error enum or error struct. Host exceptions retain the
error type, stable code, and display message. Enum codes identify variants. A
struct error has one code derived from its type name (or supported rename) and
does not expose its fields as a host payload. Error enum variants may carry
fields for Rust's internal behavior, but those fields are not registered or
exposed through generated host exceptions. Structured error payloads are not a
0.4 feature.

### `#[rspyts::export]`

Exports an existing public function, `const`, `static`, or inherent impl.
`#[rspyts::export(python)]` limits a native-only function to Python;
`#[rspyts::export(typescript)]` limits it to executable TypeScript. Use
`#[rspyts::export(static)]` on static TypeScript vocabulary. Constants and
statics are static-output data; static mode never translates function bodies.

For a normal `Result<T, E>`, rspyts reads the error type directly. A domain
alias with only one visible type parameter must name its fixed error explicitly:

```rust
type DomainResult<T> = Result<T, DomainError>;

#[rspyts::export]
#[rspyts(error = crate::DomainError)]
pub fn evaluate() -> DomainResult<Summary> { /* ... */ }
```

The same `error = path::Type` helper is available on exported resource
constructors and methods. It is required only when the return syntax does not
contain the error type; it must not contradict an ordinary two-parameter
`Result<T, E>`.

### `#[rspyts::export]` on an inherent `impl`

Declares an opaque resource backed by the real Rust value. The curated impl must
contain at least one public `#[rspyts(constructor)]`; `new` is the primary host
constructor when present, otherwise the first declared constructor is primary.
Additional constructors become named class/static factories. Public `&self`
and `&mut self` methods are exported, `#[rspyts(skip)]` omits a public method,
and methods that consume `self` are rejected.

Constructors and methods may add `python` or `typescript` in the same helper
attribute to limit genuinely host-specific operations:

```rust
#[rspyts::export]
impl Recording {
    #[rspyts(constructor, python)]
    pub fn open_path(path: String) -> Result<Self, OpenError> { /* ... */ }

    #[rspyts(constructor)]
    pub fn open_bytes(#[rspyts(bytes)] bytes: &[u8]) -> Result<Self, OpenError> {
        /* ... */
    }

    #[rspyts(python)]
    pub fn native_path(&self) -> String { /* ... */ }
}
```

Every backend included by the resource's `#[rspyts::export(...)]` target must
retain at least one constructor after member target filtering. Selecting one
host in `rspyts.toml` or with `--target` does not relax that compile-time rule.
Constructors and methods are compiled into target-native wrappers. Host wrappers
own their resource and expose deterministic disposal:

- Python: idempotent `close()` and context-manager cleanup;
- TypeScript: idempotent `free()` and `Symbol.dispose`.

Using a disposed resource is an error. Finalization is a leak backstop, not the
normal lifetime mechanism and is not a portability guarantee. `close` and
`free` are host lifecycle names and cannot also be exported as domain resource
methods.

### `rspyts::module!`

Appears once in the exporting crate and names the private native module. It
exports the build-time semantic manifest and registers only the selected target
wrappers. The manifest inspection symbol is not an application call ABI.

```rust
rspyts::module!(native);             // Python and TypeScript/WASM wrappers
rspyts::module!(native, python);     // Python wrappers only
rspyts::module!(native, typescript); // TypeScript/WASM wrappers only
```

The qualifier selects the module wrapper only. In a single-host crate, scope
its exported functions and resources to the same host as well; an unqualified
`#[rspyts::export]` still declares both backend wrappers under their Cargo
feature predicates. The compiler probe remains available in every module form.

## Contract types

The core type vocabulary is deliberately closed:

| Rust | Python | TypeScript |
| --- | --- | --- |
| `()` | `None` | `void` |
| `bool` | `bool` | `boolean` |
| `i8`–`i32`, `u8`–`u32` | `int` with checked range | `number` with checked range |
| `i64`, `u64` | `int` | `bigint` |
| `f32`, `f64` | `float` | `number` |
| `String`, `&str` | `str` | `string` |
| `Option<T>` | `T | None` | `T | null` |
| `Vec<T>`, slices | sequence | `ReadonlyArray<T>` |
| `BTreeMap<String, T>`, `HashMap<String, T>` | mapping | readonly string-keyed object |
| tuples of 2–8 values | tuple | readonly tuple |
| `serde_json::Value` | recursive JSON value | recursive `JsonValue` |
| `chrono::DateTime<Utc>` or `DateTime<FixedOffset>` | Pydantic `AwareDatetime` | aware RFC 3339 `string` |
| derived named types | generated model/enum | generated interface/union |

`usize` and `isize` are intentionally absent. Public contracts use fixed-width
integers and perform checked conversion at internal indexing sites.

### Bytes and numeric buffers

`#[rspyts(bytes)]` preserves a byte value as Python `bytes` and TypeScript
`Uint8Array`. `#[rspyts(buffer)]` preserves the exact numeric dtype and maps to
an owned NumPy array or JavaScript typed array.

Use `bytes` with `Vec<u8>`, `&[u8]`, or a custom serializer whose real wire
value is bytes or an unsigned-byte sequence. The macro does not currently prove
the Rust carrier type. Numeric buffers preserve dtype and element count, not
shape or strides: multidimensional Python input is made contiguous and
flattened, and generated buffer outputs are one-dimensional.

Use the annotation directly on a field or parameter. For a function or
resource-method return, declare the policy on the callable:

```rust
#[rspyts::export]
#[rspyts(returns(buffer))]
pub fn samples() -> Vec<f32> { /* ... */ }

#[rspyts::export]
impl Blob {
    #[rspyts(returns(bytes))]
    pub fn encode(&self) -> Vec<u8> { /* ... */ }
}
```

Every boundary value is owned or copied before the call completes. rspyts 0.4
does not expose borrowed Rust memory and does not promise zero-copy transport.
Structured floats must be finite; numeric buffers retain IEEE NaN and infinity.
Generated TypeScript value objects, ordinary arrays, tuples, and maps are
recursively frozen on Rust output and declared deeply readonly. Owned typed
arrays remain mutable and unfrozen because JavaScript cannot freeze a non-empty
typed array; their memory is not borrowed from Rust.

### Defaults and constraints

Field annotations preserve a small, exact validation vocabulary across Rust,
Python, and TypeScript:

| Annotation | Applies to | Meaning |
| --- | --- | --- |
| `#[rspyts(literal = VALUE)]` | bool, integer, string | the value must equal the declared scalar |
| `#[rspyts(min_length = N)]` | string, list | minimum Unicode-scalar or element count |
| `#[rspyts(max_length = N)]` | string, list | maximum Unicode-scalar or element count |
| `#[rspyts(ge = N)]` | integer | inclusive minimum |
| `#[rspyts(default = VALUE)]` | bool, integer, string | insert this value when the field is absent |

`VALUE` is a boolean, signed 64-bit integer, or string literal and must match
the Rust field kind. A literal/default pair must contain the same value, a
default cannot be combined with `required`, and a default or literal must also
satisfy `ge`. Length bounds must be ordered. Constraints and defaults are part
of the semantic fingerprint and lock.

Generated Python models express these rules with Pydantic `Literal`, `Field`,
and `AwareDatetime`. Generated TypeScript uses readonly properties, literal
types, optional properties for defaulted fields, and RFC 3339 strings. Runtime
normalization applies defaults and validates inputs before calling Rust, then
validates Rust output as well. Datetimes must include a UTC offset; naive
timestamps are rejected.

### Declared wire shapes

`#[rspyts(wire = T)]` describes the existing serialized shape of a validated or
custom-Serde type. The type's own Serialize/Deserialize implementation remains
the conversion and validation boundary. A compile-time derive cannot prove
arbitrary serializer behavior, so each declared wire shape needs a Rust
round-trip test in the consuming crate.

### Requiredness

Non-optional fields are required by default. `Option<T>` is optional unless
`#[rspyts(required)]` says the key must be present; requiredness and
nullability are separate, so a required `Option<T>` may still contain null.

A non-optional Serde default must be paired with an explicit scalar
`#[rspyts(default = ...)]`. rspyts cannot execute or infer the result of an
arbitrary Rust default function during host model generation, so that scalar
must match the function or `Default` implementation used by Serde. The field is
then optional in both hosts and the compiler inserts the declared value when it
is absent. An `Option<T>` may use Serde's null/default behavior without an
explicit scalar default.

## Generated and tracked files

| Path | Policy |
| --- | --- |
| `.rspyts/**` | Generated staging output; ignore it |
| `.rspyts.tmp-*`, `.rspyts.old-*` | Transient atomic replacement siblings; ignore them |
| `.rspyts.lock.tmp-*`, `.rspyts.lock.old-*` | Transient semantic-lock replacement siblings; ignore them |
| `rspyts.lock` | Semantic public-contract review; commit it |
| `rspyts.toml` | Authored configuration; commit it |
| generated Python/TypeScript copied into artifacts | Publish it inside the consumer artifact; do not commit it as source |

Generated implementation can be large. It is compiler-owned build output, not
a second application-maintained contract.
