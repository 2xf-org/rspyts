# rspyts-cli

This crate provides the `rspyts` build command.

```sh
cargo install rspyts-cli --version '=1.0.0' --locked
rspyts build --manifest-path bindings/Cargo.toml
rspyts check --manifest-path bindings/Cargo.toml
```

The CLI always builds one Python package and one TypeScript and WebAssembly
package. It writes both packages to `dist` next to the binding Cargo manifest.

Read the [project README](https://github.com/2xf-org/rspyts).

Licensed under MIT.
