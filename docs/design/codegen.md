# Code generation

The generator reads the API that Rust actually compiled. It does not parse
source files or guess which `cfg` branches are active.

## Requirements

- Rust 1.85 or newer
- Python 3.11 or newer for generated Python packages
- Node.js 22.12 or newer for the TypeScript runtime
- `wasm32-unknown-unknown` when generating a WebAssembly artifact

The bridged crate must include `crate-type = ["cdylib", "rlib"]`, depend on
`rspyts`, and depend directly on Serde with `features = ["derive"]`.

## Configuration

Run `rspyts init` to create a commented `rspyts.toml`. Relative filesystem
paths resolve from that file, not from the current shell directory.

```toml
[crate]
path = "rust"

[build]
features = []
no-default-features = false
profile = "dev"
targets = ["wasm32-unknown-unknown"]
locked = true

[python]
enabled = true
out = "python/src/example/generated"
library_search = ["../../../../../target/debug"]

[typescript]
enabled = true
out = "typescript/src/generated"

[schema]
enabled = true
out = "schema"
```

Unknown keys are errors. An omitted output section is disabled. Setting
`enabled = false` also disables it.

Build flags are shared by `generate`, `check`, `build`, and `manifest`:

- `features` selects Cargo features;
- `no-default-features` controls Cargo defaults;
- `profile` accepts `dev`, `release`, or a custom profile;
- `targets` lists artifacts staged by `build`;
- `locked` passes `--locked` to Cargo.

Command-line build flags override the file for one invocation.

## Commands

```text
rspyts init
rspyts generate [--config rspyts.toml]
rspyts check [--config rspyts.toml]
rspyts build [--config rspyts.toml]
rspyts manifest [--config rspyts.toml]
rspyts diff old.json new.json [--fail-on breaking|any]
```

`generate` builds the host cdylib, loads its manifest, validates the complete
surface, and rewrites enabled outputs.

`check` runs the same pipeline without writing. It prints a unified diff and
exits `1` if committed output is stale. Configuration errors exit `2`; build
or load failures exit `3`.

`build` stages the host artifact and configured targets under:

```text
target/rspyts/<target-triple>/<profile>/
```

`--target` replaces configured targets. `--no-targets` builds only the host.

`manifest` prints the canonical manifest JSON. Save snapshots when a library
needs an explicit compatibility gate. `diff` treats a new top-level
declaration as additive, documentation-only changes as informational, and any
other declaration change as breaking. It is intentionally conservative.

## Generated Python

The Python output directory is wholly owned by the generator:

```text
__init__.py
classes.py
constants.py
errors.py
functions.py
library.py
models.py
```

Models use pydantic, reject unknown input fields, and preserve exact wire
aliases. Functions validate returned structured values. Each generated call
passes only its declared error map to the runtime.

The generated `library.py` creates one lazy `rspyts.Library`. Resolution order
is:

1. the `RSPYTS_LIBRARY` environment variable;
2. `Library.set_path()`;
3. configured `library_search` directories;
4. platform loader lookup.

Python slices are copied into private, aligned numpy arrays for the duration
of a native call. Returned buffers are host-owned copies.

## Generated TypeScript

The TypeScript output is:

```text
client.ts
constants.ts
errors.ts
index.ts
types.ts
```

`createClient(module)` binds the typed surface to an instantiated
`BridgeModule`. Structs become interfaces, string enums become literal unions,
data enums become discriminated unions, and classes wrap opaque handles.

Generated classes expose `free()` and `[Symbol.dispose]()`. A
`FinalizationRegistry` is a best-effort fallback, not the primary ownership
mechanism.

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

Mapped types are imported instead of emitted again. A missing mapping for a
foreign origin is an error; silently duplicating a type would break identity.

Symbols do not compose across modules. Each generated client calls only the
module whose manifest it loaded.

## Projection exclusions

Python and TypeScript may exclude items independently:

```toml
[python]
out = "python/src/example/generated"
exclude = ["browser_*", "Session.browser_only"]
```

Patterns are simple globs. Functions, classes, types, and constants match by
name. Methods and statics match as `Class.member`. Excluding a declaration
still required by another emitted declaration is a validation error.

Prefer `#[bridge(target = "python")]` or `#[bridge(target = "typescript")]`
when the distinction belongs to the Rust API itself. Use projection exclusions
for packaging choices.

## Determinism

Every generated file contains the rspyts version and a hash of the canonical
manifest. Declarations and imports are stably sorted. There are no timestamps,
machine paths, or random identifiers. Files are written only when their bytes
change.

Generated directories should be committed. CI should run `rspyts check` for
every configuration, build both runtimes, and exercise the real native and
WebAssembly artifacts.

## Validation boundary

The macro rejects local mistakes such as unsupported signatures, invalid
attributes, and unbridgeable field types. The CLI rejects conflicts that need
the full manifest: duplicate projected names, missing foreign imports,
reserved generated names, reference cycles, invalid exclusions, or an
unsupported ABI.

Validation finishes before any emitter writes. A failed generation therefore
does not leave a half-updated package.
