# rspyts-cli

This crate provides the `rspyts` build command.

The build needs Rust 1.88 or later and the `wasm32-unknown-unknown` target.
After you install Rust, run:

```sh
cargo install rspyts-cli --locked
rustup target add wasm32-unknown-unknown
```

The rspyts binary contains the matching WebAssembly code generator.

Then create or build a project:

```sh
rspyts init my-application
cd my-application
rspyts build
rspyts watch
rspyts check
```

The CLI always builds one Python package and one TypeScript and WebAssembly
package. It uses the nearest `rspyts.toml` and updates `src-py` and `src-ts`
beside the adjacent Cargo package's Rust `src` directory. The adjacent package
is linked automatically; `application.additional_packages` can link more
workspace packages.

Package manifests and root entrypoints are initialized once and then belong to
the user. Generated model, API, runtime, native, and WebAssembly files are
tracked in the generated Python and TypeScript file lists in `rspyts.toml`;
all other files are preserved.

RSPYTS maintains exact generated-file rules in `src-py/.gitignore` and
`src-ts/.gitignore` by default. Set `application.gitignore = false` to remove
those generated ignore files and allow the generated outputs to be committed.
Generated text files include an explicit do-not-edit warning.

The generated public paths follow the Cargo package names and Rust declaration
modules. The CLI does not use namespace configuration or namespace attributes.
Root and parent namespaces do not export items from child namespaces.

Python and Node.js are not `rspyts build` dependencies. The generated Python
package requires CPython 3.11 or later. The generated TypeScript source project
uses its own standard npm scripts for strict compilation and packaging and has
no runtime npm dependencies. Rust string enums are emitted as TypeScript
string unions with same-named frozen runtime values.

Read the [project README](https://github.com/2xf-org/rspyts).

Licensed under MIT.
