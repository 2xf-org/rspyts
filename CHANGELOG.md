# Changelog

## 0.4.6

- Accept canonical JSON string values for generated Python string enums and
  aware datetimes while keeping generated Pydantic models strict.
- Reject coercible literal inputs, enum bytes, and numeric datetime timestamps
  so Python and FastAPI preserve the Rust wire contract exactly.

## 0.4.5

- Add integer `le` field constraints across macro validation, semantic IR,
  native codecs, generated Python, and static TypeScript.
- Generate strict Pydantic models so integer fields reject boolean, string,
  and floating-point coercions.
- Advance the semantic contract to IR v6 for the new upper-bound metadata.

## 0.4.4

- Decode dynamic `#[rspyts(bytes)]` function and resource parameters directly
  from Python bytes or JavaScript `Uint8Array` values instead of expanding one
  Rust intermediate value per byte.
- Stream byte sequences through the Rust codec so large binary payloads remain
  bounded by their actual byte size.

## 0.4.3

- Keep repository fixtures product-neutral so protected release scans can
  verify the complete source tree without matching consumer-specific terms.
- Describe rspyts solely as a Cargo-installed build tool and remove historical
  package-registry guidance that is not part of the supported product.
- Run the protected privacy scan on every push to `main`, before an immutable
  release tag can be created.

## 0.4.2

- Scope native contract-discovery symbols to their Cargo package. A contract
  crate can now depend on another crate that also declares `rspyts::module!`
  without duplicate or ambiguous linker exports. Cargo artifacts are selected
  by exact package ID, so a custom `[lib] name` remains supported.
- Use semantic IR v5 and preserve `[u8; N]` as
  `{"kind":"fixedBytes","length":N}`. Generated Python, static TypeScript,
  WASM, and native boundaries all enforce the exact length; `Vec<u8>` and
  slices remain variable-length bytes.
- Use lock schema 3 with deterministic compact single-line JSON. Dependency
  snapshots now record the exact crate version and TypeScript package mode.
- Give every WASM npm package a canonical `./wire` export containing its
  transitively static-safe JSON declarations, enums, constants, and contract
  fingerprint. Types that cannot be represented safely are omitted as a whole.
- Strip Rust debug information from generated release WebAssembly so packaged
  artifacts do not expose build-machine checkout paths.
- Pin npm peers to the locked dependency version and verify dependency
  fingerprints when generated Python or TypeScript packages are imported.
  Static consumers of WASM packages import the owner's `./wire` surface instead
  of copying foreign declarations.
- Keep generated dependency guards as module side effects. Generated npm
  packages do not claim `"sideEffects": false`.
- Represent `i64` and `u64` as `number` only for safe exact static constants and
  fields constrained to a safe literal. Other static uses are rejected;
  executable WASM contracts use `bigint`.
- Represent Rust unit values as JSON `null` in static TypeScript contracts;
  executable WASM returns remain `void`.
- Represent static byte and numeric-buffer values as readonly JSON number
  arrays. Executable WASM keeps `Uint8Array` and numeric typed arrays.
- Validate static constants recursively against their declared Rust contract,
  including integer ranges, finite floats, arrays, maps, tuples, structs,
  enums, required fields, constraints, and JavaScript-safe integers.
- Enforce one recursive `JsonValue` number policy in generated Pydantic models,
  Rust boundary normalization, static constants, and WASM conversion: numbers
  are finite, and integer-valued numbers fit JavaScript's safe range.
- Emit object keys such as `__proto__` without changing object prototypes, and
  avoid collisions with TypeScript built-ins such as `Record` and typed arrays.
- Reject TypeScript-targeted functions and resources in static mode instead of
  silently omitting executable exports.
- Match Serde's eight rename rules exactly, reject aliases and ambiguous rename
  metadata, and allow `skip_serializing_if` only for optional
  `Option::is_none` fields.
- Restrict plain `#[serde(default)]` to direct scalar Rust defaults paired with
  the same explicit rspyts value: `false` for `bool`, `0` for integers, and
  `""` for `String`. Reject missing or mismatched rspyts values, `Option`,
  aliases, and custom Serde default functions.
- Require `#[rspyts(bytes)]` to annotate an exact byte carrier: `Vec<u8>`,
  `[u8; N]`, immutable `&[u8]`, or immutable `&[u8; N]`, while retaining the
  fixed-array length.
- Reject the ineffective `[typescript].source` option; TypeScript output is a
  complete generated package rather than an unpublished source overlay.
- Keep generated files at the single ignored `.rspyts/` output. Remove the
  arbitrary `--staging` path override and report that fixed path as `output`.
- Serialize `build`, `check`, `lock`, and `clean` with one project-local lock.
  Generate directly in a unique project-local temporary directory, validate
  locked checks before publication, and roll back two-rename replacement.
- Refuse to replace symlink, directory, or other non-file lock paths. Lock
  updates use collision-safe temporary files and a durable atomic rename.
- Normalize Python argument and codec failures into a function or resource's
  declared `ContractError` using the stable `invalid_argument` code.
- Keep generated Python dependency-guard names free of leading underscores,
  delete them after the import-time check, and exclude them from the public
  package surface.
- Repair the crate README links to the project documentation.

Install 0.4.6, inspect the current contract, review the reported drift, then
run `rspyts lock`. Update dependency owners before their consumers.
