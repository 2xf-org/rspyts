# Python

What `rspyts generate` gives you in Python, and how to use it well. The generated package layout and the runtime API it builds on are specified in [codegen.md §4](design/codegen.md); this guide is the user's view.

The runtime (`pip install rspyts`) is pure Python — `ctypes` under the hood, with numpy and pydantic as its only hard dependencies. There is no compiled extension module anywhere ([why — ADR-5](design/decisions.md#adr-5-ctypes-not-the-cpython-c-api)). Examples below use the [basic example](../examples/basic)'s contract — `summarize`, `scale`, and the `Counter` class — plus a `Recording` class and a couple of constants that the basic crate doesn't ship, to illustrate factories and bridged consts.

## Models

Every bridged struct becomes a pydantic v2 model subclassing `rspyts.Contract`:

- **Snake_case in Python, camelCase on the wire.** Fields keep their Rust names (`item_count`); an alias generator maps them to the wire names (`itemCount`). `populate_by_name=True` means either spelling works for construction and validation.
- **`extra="forbid"`.** Unknown fields are rejected — on the Rust side too (`deny_unknown_fields`). A field the contract doesn't know is an error, not a shrug.
- **`Option<T>` fields default to `None`**, so they may be omitted on construction. On the wire they serialize as explicit `null`.
- Integer fields carry `ge`/`le` bounds matching the Rust type (`u32` → `ge=0, le=4294967295`), validated before anything crosses.
- Rust doc comments become docstrings.

```python
summary = Summary(item_count=3, total=12.0, average=4.0)     # label omitted → None
summary = Summary.model_validate({"itemCount": 3, "total": 12.0, "average": 4.0})
wire = summary.model_dump(by_alias=True, mode="json")        # what actually crosses
```

You rarely call `model_dump` yourself — the generated wrappers do it. But the models are ordinary pydantic: use them in FastAPI signatures, settings, tests, wherever.

String enums become `enum.StrEnum` subclasses. Data enums become one model per variant plus a discriminated union, so pydantic dispatches on the tag and type checkers narrow on it.

## Constants

A `#[bridge] pub const` becomes a real module constant — no call, no handle, the value was captured from the compiled module at generation time:

```python
from basic_example.generated import CHANNEL_LABELS, SAMPLE_RATE_HZ

window = int(SAMPLE_RATE_HZ * 30)          # a plain float, usable anywhere
```

Constants are `Final`-annotated in `constants.py`, keep their SCREAMING_SNAKE_CASE names, and are constructed as real typed values at import time: a struct-typed constant is a pydantic model instance, a string-enum constant is the `StrEnum` member. If the Rust value changes, `rspyts check` fails CI until you regenerate — same drift gate as everything else.

## Schemaless payloads — `rspyts::Json`

A field or parameter typed `rspyts::Json` in Rust surfaces as `Any`: pass any JSON-serializable value, get plain dicts, lists, and scalars back. Nothing validates its contents at the boundary — that is both the point and the price. Prefer real bridged types wherever the shape is actually known; reach for `Json` only when the shape is genuinely decided at runtime (plugin parameters, free-form attributes).

## Errors

The hierarchy has three layers, all catchable:

```
Exception
└── rspyts.BridgeError                .code / .message / .data
    ├── rspyts.RspytsPanicError       a Rust panic crossed the boundary
    ├── rspyts.StaleHandleError       method call on a closed handle
    └── BasicError                    generated: one base per error enum
        ├── BasicErrorEmptyInput      generated: one subclass per variant
        └── BasicErrorZeroFactor
```

Catch as precisely as you like:

```python
from rspyts import RspytsPanicError
from basic_example.generated import BasicError, BasicErrorEmptyInput, summarize

try:
    summary = summarize(values, None)
except BasicErrorEmptyInput:
    ...                          # this specific variant
except BasicError:
    ...                          # anything else this enum can raise
except RspytsPanicError:
    ...                          # a bug in the Rust code; report it
```

Error envelopes carry `{"code", "message", "data"}`. The generated `errors.py` registers every class in a code→class registry at import time, and the runtime raises the registered class. `.data` keys are wire-cased (camelCase). `RspytsPanicError` means a Rust panic was caught at the boundary — the call did not complete, but the process is intact and no memory was corrupted. Treat it like the bug it is.

## numpy in, numpy out

Two distinct mechanisms, chosen per parameter and return type in Rust ([type system §5](design/type-system.md)).

**Inputs — `&[T]` slice parameters.** Typed as `np.ndarray` in the wrapper signature. Accepted: anything with a compatible buffer (numpy arrays, `array.array`, `memoryview`) and plain Python lists. The runtime passes `(pointer, element_count)` straight into Rust. An array that is already C-contiguous with the exact dtype (`&[f64]` ↔ `np.float64`) crosses **zero-copy** — Rust reads your array's memory for the duration of the call. Anything else is converted with `np.ascontiguousarray` first, which is a copy. In a hot loop, allocate with the right dtype up front.

**Outputs — `Buf<T>` returns**, including nested inside returned structs. Decoded as fresh `np.ndarray`s **copied** out of the response envelope. They are ordinary arrays: writable, owned by Python, no lifetime ties to Rust.

`Vec<f64>` fields, by contrast, are plain `list[float]` through JSON — fine for a handful of values, wrong for a million.

## Handle classes

A `#[bridge] impl` block becomes a Python class whose state lives entirely in Rust. The Python object holds only an integer handle.

```python
with Counter(10, "demo") as counter:   # __enter__/__exit__ → deterministic close
    counter.increment(5)
    print(counter.current_value())
# counter is closed; the Rust object is dropped

counter.current_value()           # raises rspyts.StaleHandleError
```

Prefer the context manager, or call `close()` explicitly. `__del__` also closes — but CPython finalizer timing is an implementation detail, so relying on it means Rust-side memory lingers until the GC gets around to it. `close()` is idempotent; methods on a closed handle raise `StaleHandleError`, a normal exception, never a crash. One handle is internally locked in Rust, so concurrent calls on the same object serialize — share objects across threads freely, just don't expect parallelism within one object.

Classes can also expose `#[bridge(static)]` methods. Plain statics arrive as `@staticmethod`s; **factories** — statics returning `Self` in Rust — are classmethods that return a new handle-backed instance:

```python
with Recording.open("night-1.edf") as rec:      # factory: a fresh handle
    print(rec.duration_s())
```

A class may be factory-only, with no constructor at all; calling `Recording(...)` directly then raises `TypeError` naming the factories to use instead. Once built, a factory-made instance is indistinguishable from a constructed one — same `close()`, same context manager, same `StaleHandleError` after close.

## The GIL

`ctypes` releases the GIL around every foreign call, so every bridged call is a GIL release point — for free, with no annotations. Long-running Rust work does not block other Python threads; a `ThreadPoolExecutor` fanning out over bridged calls gets real parallelism.

Per-call overhead is JSON encode/decode plus a ctypes dispatch — microseconds. Design your bridged API accordingly: one call that processes a million samples via `&[f64]`, not a million calls that process one.

## Packaging the generated code

The generated directory is yours to ship; the runtime is just a PyPI dependency. Two patterns:

**Inside an application** (the default): point `[python].out` into your app's package, commit the generated files, and gate them with `rspyts check` in CI. Ship the compiled cdylib however you deploy native artifacts, and set `RSPYTS_LIBRARY` (or `library_search`) to match.

**As a wheel**: put the generated package inside your distribution and include the cdylib as package data, with `library_search` pointing at its in-package location (paths resolve relative to the generated package directory, so a sibling directory works). Note that the wheel becomes **platform-specific** — one build per OS/arch tag — and that anything you ship should be built with `--release`.

Either way, regenerate with the same rspyts version you depend on at runtime. The generated code and the runtime are versioned in lockstep, and every generated header records the version and manifest hash.

## Types from other crates

A bridged crate can depend on another bridged crate; types defined in the dependency carry their origin crate in the manifest. Map that origin in `rspyts.toml` and the generated package imports them instead of re-defining them:

```toml
[python.imports]
"example-catalog" = "example.catalog.generated"
```

The generated `models.py` then contains exactly the line you would have written by hand:

```python
from example.catalog.generated import CatalogInfo
```

so `CatalogInfo` is *the same class* in both packages — `isinstance` checks, pydantic validation, and type annotations compose across them, and the dependency's exceptions keep their registrations. An origin with no mapping is emitted locally instead: the package stays self-contained, but the two copies are then nominally distinct classes. Details in [codegen.md §9](design/codegen.md); a working two-crate setup lives at [examples/multi-crate](../examples/multi-crate).

## Questions & Answers

**Can I call bridged functions with** `async`**/**`await`**?** No. Every bridged call is synchronous in v0.1, and there are no callbacks from Rust into Python ([ADR-6](design/decisions.md#adr-6-sync-only-in-v01)). The GIL is released during calls, so `await loop.run_in_executor(None, summarize, values, None)` gives you a perfectly good async wrapper in the meantime.

**Why can't my struct have a** `u64` **field?** Because the same struct must survive JavaScript and JSON, where integers above 2^53 silently lose precision. rspyts refuses at compile time ([ADR-4](design/decisions.md#adr-4-u64i64-rejected-at-compile-time)). Use `u32`/`i32` for arithmetic, or `String` for id-like values.

**How do I pass datetimes?** As `String`, formatted ISO-8601. There is no datetime type in the portable type system ([type system §9](design/type-system.md)) — timezone semantics differ too much between hosts to paper over. Parse at the edges: pydantic validates ISO strings into `datetime` happily in your own models.

**Can I pass a pandas or polars column?** Yes, through numpy. `df["x"].to_numpy()` (pandas) or `series.to_numpy()` (polars) produce arrays the runtime accepts like any other. If the result is `float64` and contiguous, it crosses zero-copy; otherwise it is copied once on the way in.

**Is a bridged call as fast as a PyO3 call?** No. ctypes dispatch plus JSON costs more per call than a compiled extension. It stops mattering when calls carry meaningful payloads — bulk data crosses as raw buffers either way — and in exchange the runtime is a pure-Python wheel that runs on every platform and every CPython ≥ 3.13.
