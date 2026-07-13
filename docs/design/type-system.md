# The rspyts portable type system

rspyts does not try to bridge every Rust type. It defines a deliberate,
closed subset that maps losslessly onto JSON, Python 3.13+ (pydantic v2), and
TypeScript. A type is bridgeable if and only if it appears in this document.
The single source of truth for a shape is the Rust definition; the pydantic
model, the TypeScript type, the JSON Schema, and the bytes on the wire are
all mechanical projections of it.

## 1. Scalars

| Rust | wire (JSON) | Python | TypeScript | JSON Schema |
|---|---|---|---|---|
| `bool` | true/false | `bool` | `boolean` | `boolean` |
| `u8` `u16` `u32` | number | `int` (ge/le bounds) | `number` | `integer` + min/max |
| `i8` `i16` `i32` | number | `int` (ge/le bounds) | `number` | `integer` + min/max |
| `f32` `f64` | number | `float` | `number` | `number` |
| `String` | string | `str` | `string` | `string` |
| `()` (return only) | null | `None` | `void` | `null` |

**Rejected scalars** — `u64`, `i64`, `u128`, `i128`, `usize`, `isize`,
`char`. JavaScript numbers lose integer precision above 2^53; rather than
silently corrupt, rspyts refuses to compile them (the `Bridged` trait is
simply not implemented) with a diagnostic suggesting `u32`/`i32` or `String`.
This is a spec decision, not a limitation to be "fixed".

Integer bounds are enforced at the boundary: pydantic validates
`ge`/`le`; Rust's serde rejects out-of-range on deserialize.

## 2. Composites

| Rust | wire | Python | TypeScript |
|---|---|---|---|
| `Option<T>` | `T` or null | `T \| None` | `T \| null` |
| `Vec<T>` | array | `list[T]` | `T[]` |
| `HashMap<String, T>`, `BTreeMap<String, T>` | object | `dict[str, T]` | `Record<string, T>` |
| bridged struct | object | pydantic model | interface |
| bridged enum | §4 | tagged union | discriminated union |
| `rspyts::Json` (§11) | any JSON value | `Any` | `unknown` |

Nesting is unrestricted (`Vec<Option<Foo>>`, `HashMap<String, Vec<Bar>>`, …).
Map keys must be `String` in v0.1. Tuples are not bridgeable (name the
fields — define a struct).

`Option<T>` fields serialize as `null` when `None` (not omitted); pydantic
models mark them with a `None` default so they may be omitted on input.

## 3. Structs

```rust
#[bridge]
/// Summary statistics for a list of numbers.
pub struct Summary {
    /// How many values were summarized.
    pub item_count: u32,
    pub label: Option<String>,
}
```

- Wire field names are **camelCase** (`itemCount`). Overridable per
  struct: `#[bridge(rename_all = "snake_case")]`.
- Unknown fields are **rejected** on deserialize (Rust `deny_unknown_fields`,
  pydantic `extra="forbid"`). Wire compatibility is explicit, never
  accidental.
- All fields must themselves be bridgeable. Doc comments propagate to
  Python docstrings, TS doc comments, and JSON Schema `description`.
- Generics, lifetimes, and non-`pub` fields are rejected by the macro.

## 4. Enums

**String enums** (all variants fieldless) map to string literals:

```rust
#[bridge]
pub enum Rounding { Up, Down, Nearest }
```

wire: `"up" | "down" | "nearest"` · Python: `enum.StrEnum` subclass ·
TypeScript: `"up" | "down" | "nearest"` · Schema: `enum`.

**Data enums** (any variant with fields) are internally tagged; every variant
must use named fields (struct variants). Tag key defaults to `"type"`,
overridable with `#[bridge(tag = "kind")]`:

```rust
#[bridge(tag = "type")]
pub enum ParsedNumber {
    Integer { value: i32 },
    Decimal { value: f64 },
}
```

wire: `{"type": "integer", "value": 42}` ·
Python: one model per variant + `Annotated[Union[…], Field(discriminator="type")]` ·
TypeScript: `{ type: "integer"; value: number } | …`.

Tuple variants and mixed fieldless-plus-data enums are rejected in v0.1
(fieldless variants inside a data enum would serialize asymmetrically).

Variant wire values (string enums) and discriminator values (data enums)
are always the camelCase variant name — `ChinEmg` is `"chinEmg"` on the
wire, no exceptions.

## 5. Bulk numeric data

| Rust | direction | Python | TypeScript |
|---|---|---|---|
| `&[u8] &[i16] &[i32] &[f32] &[f64]` | param only | `numpy.ndarray` (or any buffer-protocol object of matching dtype; lists accepted and converted) | `Uint8Array` `Int16Array` `Int32Array` `Float32Array` `Float64Array` |
| `rspyts::Buf<T>` (same `T` set) | return only (incl. nested in structs) | `numpy.ndarray` | typed array |

These bypass JSON entirely (see ABI §6). Use them for samples, signals,
images — anything where element count is large. `Vec<f64>` remains available
and serializes as a JSON array; prefer it only for small collections.

## 6. Functions

```rust
#[bridge]
/// Summarize a list of numbers.
pub fn summarize(
    values: &[f64],
    label: Option<String>,
) -> Result<Summary, BasicError> { … }
```

- Parameters: any bridgeable type, plus `&T`/`&[T]` borrows of bridgeable
  types (borrowed params deserialize to owned values on the shim side;
  `&str` is accepted as `String`).
- Return: bridgeable type, `()`, or `Result<bridgeable, E>` where
  `E: BridgeErr`.
- Generated Python: `def summarize(values: np.ndarray, label: str | None) -> Summary` — errors raise.
- Generated TS: `summarize(values: Float64Array, label: string | null): Summary` — errors throw.

## 7. Classes (opaque handles)

```rust
pub struct Counter { /* private state, NOT bridgeable as data */ }

#[bridge]
impl Counter {
    #[bridge(constructor)]
    pub fn new(start: i32, label: String) -> Self { … }
    pub fn increment(&mut self, by: i32) -> i32 { … }
    pub fn current_value(&self) -> i32 { … }
}
```

- A `#[bridge] impl` block makes the type an **opaque class**: state lives in
  Rust; foreign code holds a handle. The struct itself must NOT also be
  `#[bridge]`-annotated as a data type — a type is data or a class, never both.
  (Mark the impl block; leave the struct bare.)
- At most one `#[bridge(constructor)]`; it takes no `self` and returns
  `Self` or `Result<Self, E>`. A class may instead be built entirely
  through factories (below); a class with neither a constructor nor a
  factory is rejected.
- Methods take `&self` or `&mut self` plus bridgeable params; `self`-by-value
  is rejected.
- Python projection: a class with `__enter__`/`__exit__`, `close()`, and
  `__del__`; methods are typed. TS projection: a class with
  `free()` and `[Symbol.dispose]`.

### Statics and factories — `#[bridge(static)]`

```rust
#[bridge]
impl Recording {
    /// Open a recording file.
    #[bridge(static)]
    pub fn open(path: &str) -> Result<Self, IoError> { … }

    /// The library's default window length.
    #[bridge(static)]
    pub fn default_window() -> u32 { 512 }

    pub fn duration_s(&self) -> f64 { … }
}
```

- A `#[bridge(static)]` method takes no `self`; parameters and returns
  follow the free-function rules (§6).
- Returning `Self` or `Result<Self, E>` (written literally) makes it a
  **factory**: the instance is stored in the Rust-side slab and the caller
  receives a fresh handle, exactly as with the constructor. A class may be
  **factory-only** — no `#[bridge(constructor)]` at all, like `Recording`
  above.
- Projections: Python factories are `@classmethod`s returning `Self`,
  other statics are `@staticmethod`s; TypeScript statics live on the class
  bound by the client (`client.Recording.open(…)`). Constructing a
  factory-only class directly raises `TypeError` (Python) / throws (TS).
- Statics share one name space with methods; `new` and `drop` stay
  reserved.

## 8. Errors

```rust
#[bridge(error)]
pub enum BasicError {
    /// The input contained no values.
    EmptyInput,
    NotANumber { text: String },
}
impl std::fmt::Display for BasicError { … }
```

`#[bridge(error)]` derives `BridgeErr`:
variant → camelCase `code`, `Display` → `message`, named fields → `data`.
Python raises a generated exception subclass of `rspyts.BridgeError` with
`.code`, `.message`, `.data`; TypeScript throws a generated subclass of
`RspytsError`. Exhaustive per-code subclasses are generated so callers can
`except BasicErrorNotANumber:` / `catch (e instanceof BasicErrorNotANumber)`.

An error enum is an error, full stop — it cannot also appear as a data type
in fields or return positions (`rspyts check` rejects it). Its variants may
be fieldless or carry named fields, and unlike data enums the two may mix.
The wire `code` is always the camelCase variant name.

## 9. Deliberately unsupported (v0.1)

Datetimes (use ISO-8601 `String`; `chrono` feature on the roadmap), UUIDs
(use `String`), `u64`/`i64` and the other rejected scalars (§1), async
functions and callbacks (every bridged call is synchronous, ABI §11),
`HashSet`, tuples, unit structs, newtypes, generics, trait objects,
duck typing (every shape is a nominal, declared type — there is no
structural typing), references in return position, recursive types
(compile-time cycle = macro error), and per-item wire renames (deliberate:
the camelCase convention IS the contract; no legacy-compat surface).

Every rejection is a compile-time error at the Rust definition site — never
a runtime surprise in Python or TypeScript.

## 10. Constants

```rust
#[bridge]
/// Sample rate every analysis assumes.
pub const SAMPLE_RATE_HZ: f64 = 256.0;

#[bridge]
pub const CHANNEL_LABELS: &[&str] = &["chin_emg", "leg_emg"];
```

The const is re-emitted unchanged; its value is captured with
`serde_json::to_value` when the manifest is built inside the compiled
module, and projected as a **real importable constant** in both languages
(Python: a `Final`-annotated module attribute in `constants.py`;
TypeScript: an `export const … as const` in `constants.ts`). The
SCREAMING_SNAKE_CASE name is kept verbatim.

Accepted const types — exactly these:

- **scalars**: `bool`, `u8`/`u16`/`u32`, `i8`/`i16`/`i32`, `f32`/`f64`;
- **`&'static str`** (a `String` on the wire — `const String` is
  impossible in Rust, so the borrowed form is special-cased);
- **arrays and slices of supported types**: `[T; N]`, `&'static [T]`,
  including `&'static [&'static str]`, nested arbitrarily — each maps to
  a list;
- **any owned `Bridged` + `Serialize` type constructible in const
  context** (e.g. a `#[bridge]` struct with a `const`-buildable shape —
  such constants project as constructed models/enum members, not plain
  dicts).

Everything else — references other than the forms above, raw pointers,
function pointers, trait objects — is rejected at expansion time.
Constants share the generated module's one namespace with types,
functions, and classes; name collisions are rejected at generate time.

## 11. `rspyts::Json` — the schemaless escape hatch

`rspyts::Json` is a serde-transparent newtype over `serde_json::Value`.
On the wire it is the value itself, verbatim. It projects as `Any` in
Python, `unknown` in TypeScript, and an unconstrained schema in JSON
Schema, and it is legal anywhere data is: fields, parameters, returns,
nested inside `Option`/`Vec`/maps.

It exists for payloads whose shape is genuinely decided at runtime —
plugin parameters, free-form attributes. **Prefer real bridged types
wherever the shape is actually known**: `Json` contents cross the
boundary unvalidated, and every consumer inherits the type-checking hole.

## 12. Target scoping

```rust
#[bridge(target = "python")]
pub fn as_numpy_layout(samples: &[f64]) -> Buf<f64> { … }
```

Free functions, methods, and statics (not types, constants, or
constructors) can be limited to a single projection with
`#[bridge(target = "python")]` or `#[bridge(target = "typescript")]`,
combinable as `#[bridge(static, target = "python")]`. The compiled shim
always exists; the emitters simply skip the item when generating the
other language.

On an impl block, `#[bridge(target = "…")]` sets the default for every
method and static in it — a member's own `target` overrides it, and the
class itself (its existence and constructor) stays in both projections.
Larger-scale scoping without touching the Rust source lives in the CLI
config: the `[python]`/`[typescript]` sections take an
`exclude = ["glob", …]` list dropping whole items from one projection
(codegen.md §1).
