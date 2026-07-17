# Packaging

rspyts stages bindings; it does not replace Maturin, an npm packager, or a
project's task runner. Packaging must consume the paths from the `rspyts build`
JSON report rather than copying generated files into tracked source trees.

## Python

The Python target uses PyO3 with `abi3-py311`. A wheel includes:

- one private native extension;
- generated public Python models, functions, errors, and resource classes;
- `py.typed` and embedded package-specific helpers.

For a Maturin package, set `[python].mode = "source"`, run `rspyts build`, and
point Maturin's staged Python source at the generated tree. Source mode never
compiles or copies a native extension: Maturin compiles the consumer crate once
with its fixed `python` feature and owns the wheel's sole ABI-tagged extension.
Authored Python modules may be copied into that same temporary staging tree
with `[python].source`. rspyts rejects symlinks and authored/generated
collisions. Do not copy generated files back into the repository.

Standalone mode instead asks rspyts to compile and stage the consumer crate's
PyO3 extension. It is for a package that does not also ask Maturin to compile a
second extension. Inspect the finished wheel and fail the release if more than
one native extension is present.

In source mode, Maturin's `module-name` must be the configured Python package
plus the identifier passed to `rspyts::module!`, such as
`example.contract.native`. Its `python-source` points at the staged
`.rspyts/python` tree and it enables the consumer's fixed `python` feature.
When Maturin does not discover a staged package automatically (especially for
namespaced packages or staging outside the pyproject directory), include that
package explicitly in the wheel:

```toml
[tool.maturin]
python-source = ".rspyts/python"
module-name = "example.contract.native"
features = ["python"]
include = [{ path = "example/**/*", format = "wheel" }]
```

The consumer crate's fixed `python` feature enables
`rspyts/python-extension`. This selects the PyO3 `abi3-py311` boundary and
correct extension linking without a direct PyO3 dependency. The lower-level
`rspyts/python` feature is reserved for non-extension Rust tests.

The installed wheel may depend on normal application libraries such as NumPy
or Pydantic when its generated surface requires them. It must not depend on a
Python package named `rspyts`.

Test the actual wheel in a fresh environment that does not contain the CLI or
the source checkout. A successful source-tree import is not sufficient proof.

## Executable TypeScript

Use `mode = "wasm"` when TypeScript must execute Rust. The npm artifact includes:

- wasm-bindgen JavaScript glue;
- generated TypeScript declarations and public wrappers;
- the `.wasm` asset;
- package-specific embedded helpers.

The Rust consumer crate must declare optional wasm-bindgen directly and include
`dep:wasm-bindgen` in its fixed `wasm` feature alongside `rspyts/wasm`.
wasm-bindgen has no supported attribute crate-path override, so the generated
Rust expansion must be able to resolve that crate. This does not become an npm
dependency and does not create a separately installed host runtime.

The build machine also needs the `wasm32-unknown-unknown` target and matching
wasm-bindgen CLI. `rspyts build` uses `wasm-bindgen` from `PATH` unless
`WASM_BINDGEN` points to another executable. The optional Rust dependency,
target, and CLI tool are separate requirements.

The generated executable module exports a default asynchronous `init()`.
Consumers must await it before calling Rust-backed functions or constructing
resources. Concurrent and repeated calls share the first in-flight or
successful initialization promise, so the first call's input wins. If
initialization rejects, rspyts clears the cached promise and a later call may
retry.

Run `npm pack` against the staged package, install that tarball in a clean
consumer, and load the installed package in a real browser for release
verification. Importing `.rspyts/typescript/index.js` directly does not test
the package `files`, exports, or WASM asset. Node can
be a useful secondary smoke test, but browser behavior is the 0.4 contract.
The package must not depend on an npm package named `rspyts`.

## Static TypeScript

Use `mode = "static"` for Rust-owned types, enums, constants, and tables that
do not execute Rust in the host. Static mode does not translate function
bodies. Small generic lookups over generated tables remain authored TypeScript,
or the package switches to WASM for executable Rust behavior.

Static output is still generated below `.rspyts/` and copied into the package's
temporary `dist`; it is not checked into source control.

Authored TypeScript may be copied safely into staging with
`[typescript].source`. A collision with compiler-owned output fails the build.

## Build ordering

Only one task may own and atomically replace a package's `.rspyts/` directory.
Python and TypeScript packaging should depend on that task instead of invoking
concurrent generators. A typical graph is:

```text
rspyts check --locked
           |
      rspyts build
       /         \
Maturin wheel   npm pack
```

Run `rspyts lock` only as an explicit contract-acceptance step, never as part
of an ordinary build or CI check.

Target-selected builds let the two packaging jobs run independently when each
owns a distinct staging directory:

```sh
rspyts build --target python --staging .rspyts/python-job
rspyts build --target typescript --staging .rspyts/typescript-job
```

An invocation atomically replaces its whole staging directory; target jobs must
not share one. They still extract the contract with the same backend-neutral
native probe. Only the selected host features are added afterward, and the
unselected backend compilation and staging work is skipped. The semantic
fingerprint remains shared.
