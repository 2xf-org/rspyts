# Contract acceptance fixture

This example is the smallest end-to-end proof for rspyts 0.4. The Rust crate
contains the only authored contract. Python and TypeScript behavior tests run
against packages generated below `.rspyts/`; no generated client is checked
in.

The fixture deliberately covers nested domain values, a string enum, a typed
domain error, scalar defaults and constraints, an aware datetime,
numeric-buffer and byte transport, and a stateful resource. Its Rust tests are
also the reference behavior for both generated hosts.

The consumer enables `rspyts/python-extension` for its Python extension and
declares optional `wasm-bindgen` only for the WASM feature because its attribute
macro requires a direct Cargo dependency. Generated Python and JavaScript
contain no rspyts runtime dependency.

```console
cargo run -p rspyts-cli -- build --config examples/contract/rspyts.toml
cargo run -p rspyts-cli -- check --config examples/contract/rspyts.toml
```

The Python shell builds an abi3 wheel from source-mode staging. Release
acceptance installs that wheel in a clean environment, asserts it contains one
native extension and no `Requires-Dist: rspyts`, then runs the tests outside
the source tree. The TypeScript shell runs `npm pack` on the staging package,
installs the tarball, type-checks its exports, and exercises the installed
package in Chromium:

```console
cd examples/contract/python
python -m maturin build
cd ../typescript
npm ci
npx playwright install chromium
npm test
```

The host package shells contain only packaging metadata, acceptance scripts,
and behavior tests. They never import generated source directly.
