# rspyts-cli

`rspyts-cli` provides the `rspyts` code generator for crates built with the [rspyts](https://crates.io/crates/rspyts) facade. It builds a bridged Rust `cdylib`, reads its embedded manifest, validates the bridge surface, and emits Python, TypeScript, and JSON Schema files.

## Install

```sh
cargo install rspyts-cli
```

## Commands

```text
rspyts init [--dir <path>]
rspyts generate [--config <path>] [build overrides]
rspyts check [--config <path>] [build overrides]
rspyts build [--config <path>] [build overrides] [--target <triple>...]
rspyts manifest [--config <path>] [build overrides]
rspyts diff [--fail-on=breaking|any] <old.json> <new.json>
```

- `init` writes a commented starter `rspyts.toml` without overwriting an existing file.
- `generate` builds the configured crate and replaces enabled generated outputs.
- `check` runs the same pipeline without writing and exits nonzero when committed output has drifted.
- `build` compiles the host plus configured targets and stages stable artifacts
  beneath Cargo's target directory at
  `rspyts/<target>/<profile>/<platform filename>`.
- `manifest` builds the host module and writes its complete, validated,
  pretty-printed manifest to stdout. Build diagnostics stay on stderr, so
  redirecting stdout produces a clean snapshot.
- `diff` compares two complete manifest snapshots. It reports removals and
  every non-documentation change inside an existing declaration as breaking;
  only whole new top-level declarations are additive. Documentation and crate
  version changes are informational. Output is deterministic and ABI changes
  are reported first.

All relative paths resolve from the configuration file. Unknown configuration keys are rejected. A minimal configuration is:

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
out = "python/src/example/generated"
library_search = ["../../../../../target/debug"]

[typescript]
out = "typescript/src/generated"

[schema]
out = "schema"
```

`generate` and `check` use the configured features, default-feature policy,
profile, and lockfile policy when compiling the host cdylib. Configured
targets are intentionally ignored by those commands because only a native
library can be loaded to read the manifest; a warning points to `rspyts
build`. The build command stages the native artifact and every configured
target. For the default `dev` profile, for example:

```text
target/rspyts/x86_64-unknown-linux-gnu/debug/libexample.so
target/rspyts/wasm32-unknown-unknown/debug/example.wasm
```

CLI overrides replace config values: `--features`, `--no-features`,
`--no-default-features`/`--default-features`, `--profile`/`--release`, and
`--locked`/`--unlocked`. On `build`, repeated or comma-separated `--target`
replaces `targets`; `--no-targets` stages only the host. Dev, release, and
custom profiles use separate staging directories and never overwrite one
another.

Here, reproducible means locked dependency resolution plus explicit
features, profile, and target inputs with stable output paths. Compiler and
linker versions still determine bytes; the command does not claim
bit-for-bit reproducible compilation across toolchains.

`diff` exits `1` for breaking changes and `0` otherwise. Pass
`--fail-on=any` when CI should also reject additive or informational changes.
Unreadable, malformed, or semantically invalid manifest inputs exit `3`.

Exit codes are stable: `0` success, `1` generated drift or a selected diff
policy failure, `2` usage or configuration error, and `3` build, load, input,
validation, or write failure. Use `rspyts check` in CI after committed
generated files have been produced with `rspyts generate`.

See the complete [code generation specification](https://github.com/2xf-org/rspyts/blob/main/docs/design/codegen.md) and [quickstart](https://github.com/2xf-org/rspyts/blob/main/docs/introduction/quickstart.md).

Licensed under MIT.
