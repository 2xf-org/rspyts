# How rspyts works

rspyts turns Rust definitions into Python and TypeScript APIs.

You mark public Rust types, functions, constants, errors, and impl blocks with
`#[bridge]`. The macros generate a small C ABI shim and register a description
of each item. `rspyts generate` builds the crate, reads that compiled manifest,
and writes the host code.

There is no interface file to keep in sync:

```text
Rust definitions
      │
      ├── native cdylib ── ctypes ── Python
      │
      ├── WebAssembly ────────────── TypeScript
      │
      └── compiled manifest ──────── generated clients + JSON Schema
```

## What crosses

Structured values use JSON inside a strict request or response envelope.
This covers structs, enums, options, lists, maps, tuples, exact 64-bit integer
wrappers, and finite floats.

Large binary values use an aligned attachment tail instead of JSON arrays.
`Bytes` carries opaque bytes. `Buf<T>` carries owned numeric data and may be
nested. A top-level `&[T]` is a borrowed slice argument.

Stateful Rust objects do not cross. They stay in Rust behind integer handles;
generated Python and TypeScript classes own those handles and dispose them
explicitly.

## What is generated

Python gets pydantic models, `StrEnum` values, typed exceptions, function
wrappers, handle classes, and a lazy native-library loader.

TypeScript gets interfaces, literal unions, typed errors, a client bound to one
WebAssembly instance, and disposable handle classes.

Both runtimes speak the same ABI, and their clients are generated from the same
compiled manifest. At load time the runtimes validate the exported ABI major,
not the generated manifest hash; generated wrappers then validate successful
return shapes. `rspyts check` is the build-time guard that recompiles the
manifest and detects stale generated source. Unsupported local shapes fail in
the macro. Conflicts that require the whole manifest, such as duplicate
projected names, fail during `generate` and `check`.

## Why this shape

rspyts is intentionally narrower than PyO3 or wasm-bindgen. It does not try to
make Rust objects look native to either host. It moves typed values, calls Rust,
and keeps state behind handles. That smaller contract is what lets Python and
TypeScript behave alike.

The exact rules live in the [type system](../design/type-system.md),
[ABI](../design/abi.md), and [code generation](../design/codegen.md) documents.
