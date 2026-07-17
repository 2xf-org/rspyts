# rspyts-cli

The compiler and build command for
[`rspyts`](https://crates.io/crates/rspyts).

```sh
cargo install rspyts-cli --version '=0.4.3' --locked
rspyts build
rspyts check --locked
```

`rspyts.toml` describes one constrained contract package. Generated Python,
TypeScript, native, and WASM files always live in the ignored `.rspyts/`
directory. Commit `rspyts.lock`; run `rspyts lock` only after reviewing an
intentional semantic change.

See the [project README](https://github.com/2xf-org/rspyts/blob/main/README.md)
and [CLI reference](https://github.com/2xf-org/rspyts/blob/main/docs/reference.md).

Licensed under MIT.
