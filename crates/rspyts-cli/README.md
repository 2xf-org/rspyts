# rspyts-cli

The compiler and build command for
[`rspyts`](https://crates.io/crates/rspyts).

```sh
cargo install rspyts-cli --version '=0.4.1' --locked
rspyts build
rspyts lock
rspyts check --locked
```

Generated build output lives in `.rspyts/`; the reviewed semantic contract
lives in `rspyts.lock`.

See the [project README](https://github.com/2xf-org/rspyts#readme)
and [CLI reference](https://github.com/2xf-org/rspyts/blob/main/docs/reference.md).

Licensed under MIT.
