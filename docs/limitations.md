# Limitations

The 0.4 surface is closed on purpose. Unsupported Rust is rejected instead of
being widened, stringified, or routed through application-authored glue.

## Not supported

- compatibility with any 0.3 macro, generated API, runtime, or wire protocol;
- a generic language-neutral C call ABI;
- Python or npm distributions of rspyts;
- arbitrary Rust export discovery;
- generic, async, callback, iterator, stream, or `impl Trait` exports;
- borrowed values whose lifetime crosses into the host;
- zero-copy NumPy or WebAssembly views of Rust-owned memory;
- `usize` or `isize` in the public contract;
- arbitrary custom Serde behavior without an explicit declared wire shape;
- `flatten`, untagged ambiguity, remote derives, or host-shape guessing;
- structured fields on typed error variants;
- automatic translation of Rust function bodies to static TypeScript;
- a separate Node native backend;
- a guarantee that every WebAssembly panic can be caught as a typed error;
- host-model validation at a moment different from the actual Rust call unless
  that behavior is explicitly generated and tested.

Resources have one or more constructors, export only borrowed-receiver methods,
and reserve the host lifecycle names `close` and `free`. Every backend included
by the resource export target must have at least one constructor after member
target filtering; build configuration does not relax that macro rule. Split
impl blocks or use `#[rspyts(skip)]` to keep unrelated public methods outside
the contract.

## Ownership boundaries

All buffers and bytes are copied or transferred as owned boundary values. A
host cannot retain a borrowed view into Rust memory. Python calls may initially
hold the GIL; release-GIL behavior is not a 0.4 guarantee.

Numeric buffers preserve dtype and element count but are flat: they do not
preserve multidimensional shape or strides.

Resources require explicit deterministic disposal. Python `close()` and
TypeScript `free()`/`Symbol.dispose` are idempotent, but using a disposed
resource is an error. A JavaScript resource is poisoned only by a WebAssembly
runtime trap, not by an ordinary typed domain error.

## Portability

Python targets CPython 3.11 or newer through `abi3-py311`. Executable
TypeScript targets browser WebAssembly through wasm-bindgen. Native path and
file APIs must be Python-only; browser APIs accept portable owned bytes.

Exact 64-bit integers map to Python `int` and TypeScript `bigint`. Structured
floating-point values must be finite. Numeric buffers preserve all IEEE
floating-point values, including NaN and infinity.

Datetime fields are aware `chrono::DateTime<Utc>` or
`chrono::DateTime<FixedOffset>` values. Their wire form is RFC 3339; naive
datetimes and timestamps without an offset are rejected.

## Custom contracts

A declared `#[rspyts(wire = T)]` shape is a statement by the consumer, not a
proof derived from arbitrary serializer code. The compiler checks the declared
type and boundary conversions it can observe; the consumer must keep a
round-trip fixture for the actual custom serializer.

If an existing Rust API cannot fit these rules, prefer a small concrete public
endpoint or a clearer fixed-width domain field. Do not introduce a parallel
`Bridge*` model to make the compiler accept it.
