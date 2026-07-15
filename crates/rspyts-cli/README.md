# rspyts-cli

`rspyts-cli` generates Python, TypeScript, and JSON Schema contracts from a
Rust `cdylib` built with [rspyts](https://crates.io/crates/rspyts).

## Install

```sh
cargo install rspyts-cli
```

## Commands

```text
rspyts init [--dir <path>]
rspyts generate [--config <path>] [cargo options]
rspyts check [--config <path>] [cargo options]
rspyts manifest [--config <path>] [cargo options]
rspyts build [--config <path>] [cargo options] [--target host|<triple>...] [--out-dir <path>] [--output-format text|json]
```

Each command has one responsibility:

- `generate` compiles and validates the host bridge, then writes generated
  source.
- `check` compiles and validates the same bridge, compares the expected source
  with the filesystem, and never writes generated source or staged libraries.
- `manifest` prints the complete validated manifest and never stages a
  library. Cargo diagnostics remain on stderr, so redirected stdout is clean.
- `build` is the only command that stages compiled artifacts. It does not
  generate source or validate generated-source drift.
- `init` writes a starter `rspyts.toml` without overwriting an existing file.

`build` selects the host when no `--target` is passed. Once `--target` is
present, it selects exactly the listed values; use the literal `host` to
include the host alongside explicit Rust target triples. Values may be
repeated or comma-separated:

```sh
rspyts build                                      # host only
rspyts build --release --target wasm32-unknown-unknown
rspyts build --release --target host,wasm32-unknown-unknown
```

A host build is staged in `<python.out>/lib` when Python output is configured,
or in Cargo's `target/rspyts/native/<profile>` directory otherwise. Explicit
targets are staged in `target/rspyts/<target>/<profile>`. `--out-dir` puts a
single selected artifact directly in the specified directory, which is useful
for wheel and archive assembly without mutating a source package:

```sh
rspyts build --release --locked --out-dir "$WHEEL_INPUT" --output-format json
```

Staging uses an atomic replacement, so readers do not observe partial files.

## Configuration

Paths resolve from the configuration file. A section exists when its output is
wanted and is omitted when it is not. Unknown keys are rejected.

```toml
[crate]
path = "rust"
features = ["bridge"]
no-default-features = false

[python]
out = "python/src/example/_generated"

[typescript]
out = "typescript/src/generated"

[schema]
out = "schema"
```

Generated Python always looks for its native library in the fixed `lib`
directory. Cross-crate type imports remain explicit language mappings:

```toml
[python.imports]
"shared-types" = "shared_types.generated"

[typescript.imports]
"shared-types" = "@scope/shared-types/generated"
```

The positive cargo overrides are `--features`, `--no-default-features`,
`--release`, and `--locked`. Crate features are contract inputs and therefore
live in `[crate]`; build profile and lock enforcement are invocation choices.
Commands use Cargo's development profile without lock enforcement unless
`--release` or `--locked` is passed.

## Machine-readable builds

`build --output-format json` prints one versioned report to stdout. Progress
and Cargo diagnostics stay on stderr. Format version 1 contains resolved Cargo
inputs, the configured Python output when present, and every staged artifact:

```json
{
  "formatVersion": 1,
  "crate": { "name": "example", "version": "1.0.0" },
  "build": {
    "features": ["bridge"],
    "noDefaultFeatures": false,
    "profile": "release",
    "locked": true
  },
  "python": {
    "out": "/workspace/python/src/example/_generated"
  },
  "artifacts": [
    {
      "kind": "native",
      "target": "x86_64-unknown-linux-gnu",
      "path": "/workspace/python/src/example/_generated/lib/libexample.so"
    }
  ]
}
```

Consumers should reject unsupported report versions instead of guessing at
new fields.

Exit codes are `0` for success, `1` for generated drift, `2` for usage or
configuration errors, and `3` for build, load, validation, or write failures.

Licensed under MIT.
