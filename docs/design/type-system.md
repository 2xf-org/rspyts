# Type system

rspyts supports a closed set of Rust shapes that map predictably to JSON,
Python 3.11+, TypeScript, and JSON Schema.

If a shape is not listed here, it is not part of the bridge.

## Scalars

| Rust | Wire | Python | TypeScript |
|---|---|---|---|
| `bool` | boolean | `bool` | `boolean` |
| `u8`, `u16`, `u32` | bounded integer | `int` | `number` |
| `i8`, `i16`, `i32` | bounded integer | `int` | `number` |
| `rspyts::I64` | canonical signed decimal string | `int` | `bigint` |
| `rspyts::U64` | canonical unsigned decimal string | `int` | `bigint` |
| `f32`, `f64` | finite number | `float` | `number` |
| `String`, `&str` parameter | string | `str` | `string` |
| `()` return | null | `None` | `void` |

Bare `i64`, `u64`, `i128`, `u128`, `isize`, `usize`, and `char` are not
bridgeable. JavaScript cannot represent every 64-bit integer as `number`, so
full-range values must use the explicit wrappers.

Exact integer strings contain no leading zeroes, no plus sign, and no negative
zero. Range and canonical form are checked in both runtimes.

Structured floats must be finite. Generated runtimes reject NaN and infinities
in JSON positions and normalize returned negative zero. Numeric attachments
preserve raw IEEE values.

## Composites

| Rust | Python | TypeScript |
|---|---|---|
| `Option<T>` | `T | None` | `T | null` |
| `Vec<T>` | `list[T]` | `T[]` |
| `HashMap<String, T>` | `dict[str, T]` | `Record<string, T>` |
| `BTreeMap<String, T>` | `dict[str, T]` | `Record<string, T>` |
| tuple of 2–12 items | fixed tuple | fixed tuple |
| bridged newtype | named alias | named alias |
| bridged struct | pydantic model | interface |
| bridged enum | enum or discriminated union | literal or discriminated union |
| `rspyts::Bytes` | `bytes` | `Uint8Array` |
| `rspyts::Buf<T>` | `numpy.ndarray` | typed array |
| `rspyts::Json` | `rspyts.JsonValue` | `unknown` |

Composites may nest. Map keys are strings. A one-item tuple should be a
transparent newtype; tuples longer than 12 should become named structs.

## Structs

```rust
#[bridge]
pub struct Summary {
    pub item_count: u32,
    pub average: f64,
    pub label: Option<String>,
}
```

Default `#[bridge]` owns serialization. It adds Serde derives, camelCase wire
names, unknown-field rejection, and the internal Serde crate path.

Every field must be public and bridgeable. Generic, lifetime-parameterized,
unit, and multi-field tuple structs are rejected.

`Option<T>` fields may be omitted by generated host models, but the wire value
is explicit null when absent.

### Serde adoption

Use adoption when a type already owns a derived Serde contract:

```rust
#[bridge(serde)]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ExistingRecord {
    pub record_id: u32,
    #[serde(rename = "display")]
    pub display_name: String,
}
```

Adoption adds no Serde attributes or derives. Both derives are required.
Object structs and data enums must already use `deny_unknown_fields`, keeping
Rust and Python rejection behavior aligned.

rspyts reflects this closed Serde vocabulary:

- container `rename`, `rename_all`, `tag`, `transparent`, and
  `deny_unknown_fields`;
- field `rename`;
- variant `rename` and `rename_all`.

Directional names, aliases, `flatten`, `skip`, `default`, `untagged`,
`with`, and custom serializers are rejected. They can change a shape in ways
the manifest cannot model. Use `rspyts::Json` for an intentionally schemaless
value.

Adopted names remain exact. Generated Python uses safe binding names plus
pydantic aliases. TypeScript quotes names that are not legal identifiers.

## Transparent newtypes

A public one-field tuple struct is transparent:

```rust
#[bridge]
pub struct PacketId(pub u32);
```

A one-field named struct may use `#[serde(transparent)]`. The wire contains the
inner value, not an object. Newtypes may not form reference cycles.

## Enums

An enum with only fieldless variants is a string enum:

```rust
#[bridge]
pub enum Severity {
    Low,
    Medium,
    High,
}
```

Its default wire values are `"low"`, `"medium"`, and `"high"`.

An enum with any data variant is internally tagged. Data variants use named
fields; unit variants become tag-only objects:

```rust
#[bridge(tag = "kind")]
pub enum Event {
    Pending,
    Accepted { index: u32, value: f64 },
    Rejected { index: u32 },
}
```

The wire uses `{"kind":"pending"}` or an object containing the tag and
named fields. Tuple variants are rejected.

## Functions

```rust
#[bridge]
pub fn summarize(
    values: &[f64],
    label: Option<String>,
) -> Result<Summary, BasicError> {
    // ...
}
```

Plain parameters must be bridgeable. Shared `&T` and `&str` parameters are
decoded into owned values inside the shim and borrowed for the Rust call.
Top-level numeric `&[T]` parameters use the borrowed slice ABI.

Returns may be a bridgeable value, `()`, or `Result<T, E>` where `E`
implements `BridgeErr`. Async functions, generics, mutable references,
callbacks, and arbitrary parameter patterns are rejected.

## Errors

```rust
#[bridge(error)]
pub enum BasicError {
    EmptyInput,
    TooLarge { max: u32 },
}

impl std::fmt::Display for BasicError {
    // ...
}
```

The variant name becomes a camelCase code. `Display` supplies the message.
Named fields become structured data. Error fields may use ordinary structured
types, exact integers, finite floats, and `Json`, but not `Buf` or `Bytes`:
error envelopes have no attachment tail.

Generated runtimes dispatch through a map scoped to the function's declared
error enum.

## Constants

`#[bridge]` supports scalar constants, `&'static str`, arrays and static
slices of supported values, tuples, and const-constructible owned bridged
types.

The generator captures the serialized value from the compiled manifest and
emits a host constant. `Buf` and `Bytes` are rejected because constants have
no attachment tail. Structured float constants must be finite.

## Binary data

The attachment dtypes are:

```text
u8 i8 u16 i16 u32 i32 u64 i64 f32 f64
```

Every numeric element is little-endian and naturally aligned in the envelope
tail.

- `Bytes` carries opaque bytes and may be nested in owned data.
- `Buf<T>` carries owned numeric data and may be nested in owned data.
- `&[T]` is a top-level parameter only and is borrowed by Rust for one call.

Host runtimes copy top-level slice input into private storage before the call.
Owned inputs are decoded into Rust-owned storage. Returned values are copied
into host-owned storage. No attachment creates a cross-language lifetime.

`Bytes` and `Buf<u8>` both appear as `Uint8Array` in TypeScript. Generated
code marks declared `Buf<u8>` values so the runtime keeps the distinction.

## Stateful classes

Mark an impl block, not its backing struct:

```rust
pub struct Counter {
    value: i32,
}

#[bridge]
impl Counter {
    #[bridge(constructor)]
    pub fn new(value: i32) -> Self {
        Self { value }
    }

    pub fn increment(&mut self, by: i32) -> i32 {
        self.value += by;
        self.value
    }
}
```

The foreign class contains only a handle. A class may have one constructor.
`#[bridge(static)]` exposes statics; a static returning `Self` is a factory.
A factory-only class is allowed. A class with neither a constructor nor a
factory is rejected.

Methods take `&self` or `&mut self`. Consuming `self` is unsupported.

## Schemaless JSON

`rspyts::Json` wraps `serde_json::Value`. Generated APIs expose an ordinary
host value, but the wire uses the exact wrapper
`{"__rspyts_json__": value}`. That wrapper prevents user data resembling a
buffer placeholder from being interpreted as protocol metadata.

The shape is not checked. The value must still be valid JSON with finite
numbers. Prefer a real bridged type whenever the shape is known.

## Target scoping

Functions, methods, and statics may use `target = "python"` or
`target = "typescript"`. The Rust shim still exists; the other emitter omits
the wrapper. An impl-level target is the default for its methods and statics,
and a member may override it.

Per-language `exclude` patterns in `rspyts.toml` can remove whole items from
one generated projection. Excluding a still-referenced type is an error.
