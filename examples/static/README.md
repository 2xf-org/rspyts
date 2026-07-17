# Static TypeScript acceptance fixture

This fixture models the static-constant portion of a protocol package. Rust is
the only authored source for the exported enum values, structs, string slices,
tuple tables, and constants. `rspyts` compiles those values into an ignored
TypeScript package below `.rspyts/`.

The semantic contract is committed as `rspyts.lock`; generated JavaScript,
declarations, and contract metadata are not committed.

```console
cargo run -p rspyts-cli -- build --config examples/static/rspyts.toml
cd examples/static/typescript
npm test
```

No Rust function body is translated, and the TypeScript test contains no
duplicated implementation table. It imports and checks the emitted Rust data.
