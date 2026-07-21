# rspyts-cli

This crate provides the `rspyts` build command.

The build needs Rust 1.88 or later and the `wasm32-unknown-unknown` target.
After you install Rust, run:

```sh
cargo install rspyts-cli --locked
rustup target add wasm32-unknown-unknown
```

The rspyts binary contains the matching WebAssembly binding generator.

Then create or build a project:

```sh
rspyts init my-application
cd my-application
rspyts build
rspyts watch
rspyts check
```

The CLI always builds one Python package and one TypeScript and WebAssembly
package. It finds the one binding crate in the Cargo workspace. It writes both
packages to `dist` next to that crate by default. Pass `--output path` to
`build`, `watch`, or `check` when generated artifacts belong elsewhere; a
relative path resolves from the current working directory.

The generated public paths follow the Cargo package names and Rust declaration
modules. The CLI does not use namespace configuration or namespace attributes.
Root and parent namespaces do not export items from child namespaces.

Python and Node.js are not build dependencies. The generated Python package
requires CPython 3.11 or later. Its installer adds Pydantic 2 and adds NumPy 2
only when the Rust API uses numeric buffers. The generated TypeScript package
has no runtime npm dependencies.

Read the [project README](https://github.com/2xf-org/rspyts).

Licensed under MIT.
