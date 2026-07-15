# Code generation

The generator reads the API that Rust actually compiled. It does not parse
source files or guess which `cfg` branches are active.

## Requirements

- Rust 1.85 or newer
- Python 3.11 or newer for generated Python packages
- Node.js 22.12 or newer for the TypeScript runtime
- `wasm32-unknown-unknown` when generating a WebAssembly artifact

The bridged crate must include `crate-type = ["cdylib"]` and depend on
`rspyts`. Add `rlib` only when another Rust crate must link it. The default
bridge macros use the facade's internal Serde re-export, so a direct Serde
dependency is required only when user code names or derives it, including
`#[bridge(serde)]` adoption.

## Configuration

Run `rspyts init` to create a commented `rspyts.toml`. Relative filesystem
paths resolve from that file, not from the current shell directory.

```toml
[crate]
path = "rust"
features = []
no-default-features = false

[python]
out = "python/src/example/generated"

[typescript]
out = "typescript/src/generated"

[schema]
out = "schema"
```

Unknown keys are errors. An omitted output section is disabled.

The Rust contract inputs are shared by `generate`, `check`, `build`, and
`manifest`:

- `[crate].features` selects Cargo features;
- `[crate].no-default-features` controls Cargo defaults.

The positive command-line overrides are `--features`,
`--no-default-features`, `--release`, and `--locked`. Commands otherwise use
Cargo's development profile without lock enforcement. Artifact targets are
selected only by `build`.

## Commands

```text
rspyts init
rspyts generate [--config rspyts.toml]
rspyts check [--config rspyts.toml]
rspyts manifest [--config rspyts.toml]
rspyts build [--config rspyts.toml] [--target host|TRIPLE...] [--out-dir DIR] [--output-format text|json]
```

`generate` builds the host cdylib, loads its manifest, validates the complete
surface, and rewrites enabled outputs.

`check` runs the same source-generation pipeline without rewriting generated
source or staging a library. It prints a unified diff and exits `1` if
committed output is stale. `manifest` likewise loads and validates the host
module without staging it. Configuration errors exit `2`; build or load
failures exit `3`.

Only `build` publishes compiled artifacts. Its default host build is staged
under:

```text
<python.out>/lib/<platform filename>
```

This is the generated loader's fixed search directory. Without Python output,
the host fallback is `target/rspyts/native/<profile>/`. Explicit non-host
targets are staged under:

```text
target/rspyts/<target-triple>/<profile>/
```

With no `--target`, `build` selects the host. Once `--target` is present it
selects exactly the listed values. The literal `host` includes the host beside
explicit triples. A target-only WASM build is therefore
`build --target wasm32-unknown-unknown`. `--out-dir` stages one selected
artifact directly in an explicit directory, which lets packaging consume the
JSON-reported artifact without mutating an editable source package. Staged
replacements are atomic.

`build` only compiles and stages artifacts. It does not load the manifest,
validate the projected contract, or compare generated source; pair it with
`check` in CI and packaging workflows.

The default text output prints one staged path per artifact. With
`--output-format json`, stdout instead contains only a versioned build report;
progress and compiler diagnostics stay on stderr. Report format version 1
includes the crate name and version, resolved features/default-feature/profile/
lockfile settings, configured Python output, and staged
artifacts with their `native` or `target` kind, target triple, and absolute
path. The `python` member is absent when that output is disabled.

`manifest` prints the canonical manifest JSON. Save snapshots outside the
generator when a library needs a project-specific compatibility gate.

## Generated Python

The Python output contains generated source plus a separately staged native
artifact:

```text
__init__.py
codecs.py
classes.py
constants.py
errors.py
functions.py
lib/<platform native library>
library.py
models.py
```

Models use pydantic, reject unknown input fields, and preserve exact wire
aliases. Public model construction keeps pydantic's normal host-side
ergonomics, while generated calls use schema-directed exact-wire codecs for
successful returns. Those constructors validate scalar and container shapes
recursively before strict model construction, then explicitly convert exact
integers, enums, tuples, and attachments to their Python host forms. Each
generated call passes only its declared error map to the runtime.

The generated `library.py` creates one lazy `rspyts.Library`. Resolution order
is:

1. the library-specific `RSPYTS_LIBRARY_<NAME>` environment variable, where
   the generated library name is uppercased and non-alphanumeric characters
   become underscores;
2. `Library.set_path()`;
3. `<python.out>/lib`.

For example, `example_catalog` uses
`RSPYTS_LIBRARY_EXAMPLE_CATALOG`. The scoped override lets multiple
generated libraries resolve independently in one process.

Python slices are copied into private, aligned numpy arrays for the duration
of a native call. Returned buffers are host-owned copies.

## Generated TypeScript

The TypeScript output is:

```text
client.ts
codecs.ts
constants.ts
errors.ts
index.ts
types.ts
```

`createClient(module)` binds the typed surface to an instantiated
`BridgeModule`. Structs become interfaces, string enums become literal unions,
data enums become discriminated unions, and classes wrap opaque handles.
Successful returns are validated recursively before entering those public
types: primitive kinds, bounded integers, container and tuple shapes, exact
object fields, enum tags, attachment dtypes, exact 64-bit strings, finite f32
range, nullability, and class handles are all checked. Schemaless `Json` remains
transparent: generated codecs validate portable JSON only at schema-declared
`Json` positions without adding or removing protocol wrappers. Attachment
placeholders are materialized only at declared `Bytes` and `Buf<T>` positions.

Generated classes expose `free()` and `[Symbol.dispose]()`. A
`FinalizationRegistry` is a best-effort fallback, not the primary ownership
mechanism. If that global is unavailable, generated clients install a no-op
fallback so module loading and deterministic disposal still work.

## JSON Schema

The schema emitter writes `schema.json`. It covers declared data types and the
same closed wire shapes used by both runtimes. It does not describe executable
symbols or host-specific class behavior.

## Shared types across crates

Types carry their defining crate in the manifest. A consuming crate can map
that origin to an existing generated package:

```toml
[python.imports]
"shared-types" = "shared_types.generated"

[typescript.imports]
"shared-types" = "@example/shared-types"
```

Import mappings are opt-in. When a foreign origin is not mapped, its DTOs are
emitted into the consuming package, which keeps that package self-contained but
gives Python a separate model class and TypeScript a locally emitted structural
declaration. This is often the right choice when the generated package is a
private implementation detail and consumers do not exchange those model
objects directly.

Add a mapping only when the Python or TypeScript packages are intentionally one
generated package graph and must share the dependency package's host-language
types. Mapped types are imported instead of emitted again. In Python this
preserves one runtime model class. In TypeScript it preserves shared declaration
provenance and one source of truth, not nominal identity: structurally equal
interfaces remain mutually assignable even when emitted separately. Import
mappings also couple the packages' generated internals. Every package in that
imported graph must be regenerated with the same rspyts codegen release,
dependencies before consumers. Python exact-return decoders deliberately use
generated wire helpers on imported models, while TypeScript output assumes the
imported generated declarations and error helpers follow the same manifest
semantics. Mixing a newly generated consumer with a stale generated dependency
is therefore unsupported. Run `rspyts check` for every config in the graph as
one upgrade gate.

Symbols do not compose across modules. Each generated client calls only the
module whose manifest it loaded.

## Determinism

Every generated Python and TypeScript source file opens with a prominent
do-not-edit header containing the rspyts generator version, a portable path to
the bridged Rust source tree relative to the generated file, and a hash of the
canonical manifest. `schema.json` carries the same warning in `$comment` and
records the generator version, relative Rust source, bridged crate name, crate
version, and manifest hash under `x-rspyts`; `x-rspyts.version` remains the
bridged crate version. These hashes record provenance and let `rspyts check`
detect drift. Every compiled module exports the same contract fingerprint, and
generated Python and TypeScript clients require an exact match when loading it.
A stale client therefore fails before its first bridge call rather than
silently diverging.

Declarations and imports are stably sorted. There are no timestamps, absolute
machine paths, or random identifiers. Files are written only when their bytes
change.

Generated Python/TypeScript source and `schema.json` should be committed.
Native libraries staged under `<python.out>/lib` and WASM files under
`target/rspyts` are build artifacts: exclude them from version control, package
them where needed, and rebuild them for the target platform. CI should run
`rspyts check` for every configuration, build both runtimes, and exercise the
real native and WebAssembly artifacts.

## Validation boundary

The macro rejects local mistakes such as unsupported signatures, invalid
attributes, and unbridgeable field types. The CLI rejects conflicts that need
the full manifest: duplicate projected names, reserved generated names,
reference cycles, invalid type shapes, or an unsupported ABI. An absent
cross-crate import mapping is not an error; it selects local re-emission as
described above.

The CLI loads and validates the fresh Cargo artifact before source generation
or drift comparison. `generate` begins writing only after validation;
`check` and `manifest` remain read-only with respect to generated source and
staged artifacts. `build` is the only artifact publication boundary, and its
staged replacements are atomic.
