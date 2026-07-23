# rspyts

`rspyts` defines a Rust API contract that rspyts compiles into Python and TypeScript packages. Add contract metadata to normal serializable Rust types and public functions; the `rspyts` command handles host code and native artifacts.

## Add the Rust dependencies

```toml
[dependencies]
rspyts = "3"
serde = { version = "1", features = ["derive"] }
```

Add `thiserror` when you export typed errors. Add `chrono` or `serde_json` when those values appear in the contract.

## Export a model and function

```rust
use rspyts::Model;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Model)]
#[serde(rename_all = "camelCase")]
pub struct Greeting {
    pub display_name: String,
    pub message: String,
}

#[rspyts::export]
pub fn greet(display_name: String) -> Greeting {
    Greeting {
        message: format!("Hello, {display_name}!"),
        display_name,
    }
}
```

After `rspyts build`, Python receives a frozen Pydantic model and a `greet` function. TypeScript receives a readonly interface and a `greet` function. Rust module paths become host package namespaces.

`Model` requires a named-field struct, a supported enum, or a transparent one-field tuple struct. Derive both `Serialize` and `Deserialize` so the same wire shape crosses both host boundaries.

## Shape models

### Records and constraints

Use Serde for wire names and rspyts attributes for host-visible field rules:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Model)]
#[serde(rename_all = "camelCase")]
pub struct JobRequest {
    #[rspyts(min_length = 2, max_length = 64)]
    pub display_name: String,

    #[rspyts(ge = 1, le = 100)]
    pub count: u32,

    #[serde(default)]
    #[rspyts(default = false)]
    pub dry_run: bool,

    pub note: Option<String>,
}
```

Supported field options are:

| Attribute | Supported field |
| --- | --- |
| `required` | Force a field, including an `Option<T>`, to be present |
| `default = <literal>` | Boolean, signed 64-bit integer, or string default |
| `literal = <literal>` | Boolean, integer, or string literal constraint |
| `min_length`, `max_length` | String or list length |
| `ge`, `le` | Integer range |
| `bytes` | `Vec<u8>`, `[u8; N]`, or a borrowed byte slice at a function boundary |
| `buffer` | Numeric `Vec<T>` or a borrowed numeric slice at a function boundary |

Python enforces model rules through Pydantic. TypeScript represents the same wire names and types at compile time.

### String enums

Unit-only enums become Python `StrEnum` classes and TypeScript string unions with frozen runtime values:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Model)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    Fast,
    Deterministic,
}
```

### Tagged enums

Internally tagged enums with named fields become discriminated host unions:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Model)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum JobEvent {
    Completed { accepted_count: u32 },
    Rejected { reason: String },
}
```

### Transparent host aliases

A one-field tuple struct marked `#[serde(transparent)]` becomes a host alias. Use `host` when its Rust representation should expose another supported contract type:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Model)]
#[serde(transparent)]
#[rspyts(host = String)]
pub struct JobId(pub String);
```

## Export typed errors

Derive `rspyts::Error` on an error that implements `Display`, then return it through `Result`:

```rust
#[derive(Debug, thiserror::Error, rspyts::Error)]
pub enum MathError {
    #[error("the divisor cannot be zero")]
    DivisionByZero,

    #[error("the value is outside the supported range")]
    #[serde(rename = "out_of_range")]
    OutOfRange,
}

#[rspyts::export]
pub fn divide(value: i32, divisor: i32) -> Result<i32, MathError> {
    if divisor == 0 {
        return Err(MathError::DivisionByZero);
    }
    Ok(value / divisor)
}
```

Generated error classes expose a stable `code` and the Rust display message. The default code is the snake-case type or variant name; `#[serde(rename = "...")]` overrides it.

## Export a resource

Put `#[rspyts::export]` on an inherent implementation to expose stateful Rust values:

```rust
pub struct Counter {
    value: i64,
}

#[rspyts::export]
impl Counter {
    #[rspyts(constructor)]
    pub fn new(value: i64) -> Self {
        Self { value }
    }

    pub fn add(&mut self, delta: i64) -> i64 {
        self.value += delta;
        self.value
    }

    #[rspyts(skip)]
    pub fn reset_for_rust(&mut self) {
        self.value = 0;
    }
}
```

An exported resource needs at least one public method marked `constructor`. Other public methods are exported unless marked `skip`. Generated resources have `close()`; call it when the host is finished with the Rust value.

## Choose bytes or numeric buffers

An ordinary `Vec<T>` is a host list. Mark byte and numeric-buffer boundaries when representation matters:

```rust
#[rspyts::export]
#[rspyts(returns(bytes))]
pub fn reverse_bytes(#[rspyts(bytes)] input: &[u8]) -> Vec<u8> {
    input.iter().rev().copied().collect()
}

#[rspyts::export]
#[rspyts(returns(buffer))]
pub fn scale(#[rspyts(buffer)] values: &[f64], factor: f64) -> Vec<f64> {
    values.iter().map(|value| value * factor).collect()
}
```

Bytes become Python `bytes` and TypeScript `Uint8Array`. Numeric buffers become typed NumPy arrays and JavaScript typed arrays. Add `numpy` to `src-py/pyproject.toml` before building a contract that uses `buffer`.

Binary annotations belong on top-level exported parameters and returns. rspyts rejects `bytes` and `buffer` fields inside models because nested values cannot use the direct native buffer ABI; keep metadata in the model and pass large payloads as separate annotated parameters.

## Supported value types

| Rust contract type | Python | TypeScript |
| --- | --- | --- |
| `()` | `None` | `void` return or `null` value |
| `bool` | `bool` | `boolean` |
| `u8` through `u32`, `i8` through `i32` | `int` | `number` |
| `u64`, `i64` | `int` | `bigint` |
| `f32`, `f64` | `float` | `number` |
| `String`, `str` | `str` | `string` |
| `Option<T>` | `T | None` | `T | null` |
| `Vec<T>`, `[T]` | `list[T]` | `ReadonlyArray<T>` |
| `HashMap<String, T>`, `BTreeMap<String, T>` | `dict[str, T]` | `Readonly<Record<string, T>>` |
| tuples of 2 through 8 items | `tuple[...]` | readonly tuple |
| `chrono::DateTime<Utc>` or `DateTime<FixedOffset>` | `datetime` | ISO-8601 `string` |
| `serde_json::Value` | `Any` | JSON value union; wide integers use `bigint` |
| a `Model` type | generated model | generated interface, union, or alias |

Immutable references to supported values are accepted as function inputs. Exported functions must be synchronous, non-generic public functions with simple identifier parameters.

An `Option<T>` requires an inner wire type that does not already admit null. Contracts such as `Option<Option<T>>`, `Option<()>`, and `Option<serde_json::Value>` are rejected because their distinct Rust states cannot round-trip through Python or JavaScript.

## Naming and Serde behavior

Python functions retain snake case. TypeScript functions and parameters use lower camel case. Model field and enum variant names follow supported Serde `rename`, `rename_all`, `rename_all_fields`, and `tag` attributes.

rspyts uses one name in both serialization directions. Directional rename rules and Serde aliases are rejected at compile time. Generated Python models reject unknown fields.

## Export constants

Public constants and statics can use the same export attribute:

```rust
#[rspyts::export]
pub const CONTRACT_REVISION: u32 = 7;

#[rspyts::export]
pub static DEFAULT_LABEL: &str = "production";
```

## Application bridge

The CLI compiles an internal bridge and calls `application!` on your behalf. Most projects should not place this macro in their own source.

If you are embedding rspyts contract discovery in custom build tooling, `rspyts::application!()` emits the native discovery entrypoints, Python module, and Wasm entrypoint. Optional crate identifiers force linked packages into that bridge:

```rust
rspyts::application!(shared_api, reporting_api);
```

Use the [CLI and configuration reference](https://github.com/2xf-org/rspyts/tree/main/crates/rspyts-cli) for normal project builds. The [repository example](https://github.com/2xf-org/rspyts/tree/main/example) shows a sensor-reading API with matching handwritten Python and TypeScript helpers beside it.
