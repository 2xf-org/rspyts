# rspyts-cli

This crate provides the `rspyts` build command.

```sh
cargo install rspyts-cli --version '=1.0.0' --locked
rspyts init my-application
rspyts build
rspyts watch
rspyts check
```

The CLI always builds one Python package and one TypeScript and WebAssembly
package. It finds the one binding crate in the Cargo workspace. It writes both
packages to `dist` next to that crate.

Read the [project README](https://github.com/2xf-org/rspyts).

Licensed under MIT.
