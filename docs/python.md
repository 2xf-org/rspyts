# Python

`rspyts generate` writes a typed Python package that calls your native Rust
library through ctypes.

The runtime supports CPython 3.11 through 3.14. It is pure Python; numpy and
pydantic v2 are its only runtime dependencies.

## Generated package

The generated directory contains:

```text
__init__.py   public re-exports
models.py     pydantic models and enums
constants.py bridged constants
errors.py    exception classes and scoped error maps
functions.py free-function wrappers
classes.py   handle-backed classes
library.py   lazy native-library loader
```

Import from the parent package you ship, or directly from `generated` while
prototyping:

```python
import numpy as np

from basic_example.generated import summarize

summary = summarize(np.array([2.0, 4.0, 6.0], dtype=np.float64), "demo")
print(summary.average)
```

## Models

Bridged structs subclass `rspyts.Contract`.

- Python bindings use snake_case.
- Wire keys use the exact manifest names, camelCase by default.
- Supported Serde renames become explicit pydantic aliases.
- Unknown fields are rejected.
- `Option<T>` fields may be omitted and serialize as null.
- Integer ranges match Rust.
- Structured floats must be finite and returned negative zero is normalized.

`Contract` is deliberately not pydantic's global strict mode. It uses the
normal pydantic conversion rules plus the bridge's own range, shape, alias,
and unknown-field checks.

String enums become `StrEnum` classes. Tagged Rust enums become one model per
variant plus a discriminated union.

## Exact integers

Rust `rspyts::I64` and `rspyts::U64` appear as ordinary Python `int` values.
Generated code validates the full signed or unsigned range and uses canonical
decimal strings on the wire.

```python
value = ExactNumbers(signed=-(2**63), unsigned=2**64 - 1)
```

The conversion is recursive through models, options, lists, maps, tuples,
constants, and typed error data. Bare Rust `i64` and `u64` are not bridgeable.

## Schemaless JSON

Rust `rspyts::Json` appears as `rspyts.JsonValue`. The host value remains an
ordinary dict, list, string, number, boolean, or null. Its shape is not
validated, but it must still be JSON-compatible and all numbers must be
finite.

rspyts uses an internal wrapper on the wire so a user object that looks like a
buffer placeholder remains ordinary user data. Generated code adds and removes
that wrapper; applications do not.

## Errors

A `#[bridge(error)]` enum becomes one base exception and one subclass per
variant:

```python
from basic_example.generated import BasicError, BasicErrorEmptyInput, summarize

try:
    summarize(values, None)
except BasicErrorEmptyInput as error:
    print(error.code, error.data)
except BasicError:
    raise
```

Every bridge exception carries `code`, `message`, and optional `data`.
Generated wrappers pass the relevant enum's code-to-class map on each call.
This avoids collisions when two packages use the same short error code.

`RspytsPanicError` means native Rust panicked. The shim catches the unwind, so
the library remains loaded, but the failed operation may have changed
application state. Treat it as a Rust bug and discard handles whose invariants
are uncertain.

`StaleHandleError` means a method used a closed or unknown handle.

## Bytes and numeric buffers

The Rust type chooses the transport:

| Rust | Python | Behavior |
|---|---|---|
| `Bytes` | `bytes` | Opaque owned bytes |
| `Buf<T>` | `numpy.ndarray` | Owned numeric attachment; nesting allowed |
| `&[T]` parameter | array-like input | Borrowed by Rust for one call |
| `Vec<T>` | `list[T]` | JSON array |

For a top-level slice, the runtime converts to the exact dtype and copies into
private, C-contiguous storage before releasing the GIL. Rust borrows that
private copy. The copy prevents concurrent Python or native mutation from
racing the Rust read.

Owned `Buf<T>` and `Bytes` inputs are additionally decoded into Rust-owned
storage because Rust may retain them. Every returned attachment is copied into
fresh Python-owned memory.

Use buffers for large data and `Vec<T>` for small structured collections.

## Native library loading

`Library` loads on the first generated call and caches the result. Resolution
order is:

1. `RSPYTS_LIBRARY`, containing the full library path;
2. `Library.set_path(path)`, called before the first bridge call;
3. the generated `library_search` directories.

The loader normalizes Cargo crate hyphens to underscores and chooses the
platform filename (`.dylib`, `.so`, or `.dll`). It rejects any library that
does not report ABI version 2.

## Stateful classes

A bridged impl block becomes a Python class whose Rust state stays behind a
handle:

```python
with Counter(10, "demo") as counter:
    counter.increment(5)
    print(counter.current_value())
```

Use the context manager or call `close()`. Closing twice is safe. `__del__` is
only a backstop; garbage-collection timing is not a lifecycle strategy.

Factories declared with `#[bridge(static)]` and returning `Self` become
classmethods. A factory-only class cannot be constructed directly.

## Threads and performance

ctypes releases the GIL during the Rust call. Different handles may run in
parallel. Calls on the same handle serialize inside Rust.

Crossing the boundary still has a fixed cost: validation, JSON work, ctypes,
and input staging. Measure your workload and prefer a small number of coarse
calls over a large number of tiny calls.

## Shipping

Commit generated code and gate it with `rspyts check`. Ship the cdylib beside
your package or set `RSPYTS_LIBRARY` at deployment time. A wheel containing the
cdylib is platform-specific even though the `rspyts` runtime wheel itself is
pure Python.

See the [quickstart](introduction/quickstart.md) and the complete
[type system](design/type-system.md).
