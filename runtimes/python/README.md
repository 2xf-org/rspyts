# rspyts (Python runtime)

The pure-Python runtime used by packages generated with `rspyts generate`.
It provides pydantic contracts, typed bridge errors, and the native `Library`
loader/caller. Python 3.11 through 3.14 is
supported; numpy and pydantic v2 are its only runtime dependencies.

## Application API

- `Contract` is the unknown-field-rejecting pydantic base for generated models.
- `Library` locates and calls an ABI 3 Rust `cdylib`.
- `BridgeError`, `RspytsPanicError`, and `StaleHandleError` are the public
  exception hierarchy.

Generated APIs use ordinary Python `int` for Rust `i64`/`u64` and
`typing.Any` for Rust `serde_json::Value`. Exact range and JSON portability
checks live at the generated wire boundary rather than in public wrapper
types.

Generated packages pass call-scoped error mappings. There is no mutable global
error registry, so independent generated packages cannot collide on codes.

`Library` checks a library-specific override such as
`RSPYTS_LIBRARY_DEMO_CRATE` before explicit and packaged paths. ABI 3
libraries must export `rspyts_contract_fingerprint`. The loader always validates
that export and generated packages pass `expected_contract_fingerprint` to fail
before the first call if the Python bindings and native module differ.

## Generated-code contract

Low-level codec primitives live in `rspyts._internal`, not at package top
level. Each generated package owns a private `_codecs.py`, calls
`_internal.require_emitter_api(3)`, uses the single `Library.call()` path, and
passes its `_internal.Response` through every schema-directed decoder. A
response explicitly owns both `value` and `tail`; container decoders return
child `Response` values carrying the same tail.

ABI 3 `serde_json::Value` is transparent on the wire. `json_to_wire` validates
and returns the JSON value unchanged, while `json_from_wire` stops traversal at
the schema-declared JSON position. Objects whose keys look like attachment
metadata therefore remain ordinary JSON.

This `_internal` surface is an emitter/runtime contract, not an application
API. `EMITTER_API_VERSION` changes whenever generated code must migrate.

## Native wheels with Hatch

Rust-backed packages can use the build hook shipped by this distribution
instead of maintaining a project-specific packaging script:

```toml
[build-system]
requires = ["hatchling", "rspyts[hatch]==0.3.0"]
build-backend = "hatchling.build"

[tool.hatch.build.hooks.rspyts]
config = "../rspyts.toml"
```

`config` is the hook's only setting and is relative to the Python project.
The matching `rspyts` CLI must be on `PATH`; the hook rejects a CLI whose
version differs from the Python runtime. Standard builds first run the
read-only binding check, then build one locked release host cdylib in a
temporary directory. The resulting wheel is tagged `py3-none-<platform>` and
contains the library under the generated package's fixed `lib` directory.

Editable builds use the same check and atomically stage a locked development
cdylib directly into `<python.out>/lib`. Add that directory to `.gitignore`.
Source distributions are rejected because they cannot contain the built native
library. Platform tags are deliberately host-only: macOS uses the Mach-O slice
deployment target and a verified `@rpath` install name, Linux requires the
interpreter's matching manylinux or musllinux policy, and Windows requires the
matching architecture and `.dll` artifact. Standard wheel builds fail closed
if the cdylib depends on a non-system shared library: statically link or bundle
that library, or use `auditwheel`, `delocate`, or `delvewheel` in a dedicated
packaging pipeline. Editable builds remain local and do not impose distribution
portability policy. rspyts does not reimplement those wheel-repair tools.

## License

This project is licensed under the MIT license.
