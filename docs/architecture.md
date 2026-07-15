# Architecture

rspyts has one compiler-facing core, one generator, and two small host
runtimes.

## Repository map

| Path | Purpose |
|---|---|
| `crates/rspyts` | Public Rust facade: `#[bridge]`, bridge types, errors, and `export!()` |
| `crates/rspyts-macros` | Macro parsing, validation, shims, and manifest registrations |
| `crates/rspyts-core` | ABI envelopes, attachments, handles, manifest IR, registry, and shim helpers |
| `crates/rspyts-cli` | Build discovery, manifest loading, validation, emitters, drift checks, and manifest inspection |
| `runtimes/python` | PyPI runtime: pydantic base, ctypes loader, envelope codec, and errors |
| `runtimes/typescript` | npm runtime: WebAssembly loader, envelope codec, calls, poisoning, and errors |
| `examples` | End-to-end contracts used by CI |

Applications normally depend on `rspyts`, `rspyts-cli`, and one host runtime.
The core and macro crates are implementation packages.

## A native call

Suppose Python calls a generated `summarize(values, label)` wrapper.

1. The wrapper dumps ordinary parameters with exact wire names.
2. The runtime copies the numeric slice into private, aligned numpy storage.
3. `Library` lazily loads the cdylib, verifies ABI version 3 and the contract fingerprint.
4. The runtime creates one ABI-3 request envelope and calls the exported shim.
5. The shim validates the request, borrows the staged slice, and calls Rust.
6. Rust serializes the result into one response envelope.
7. Python copies attachments out, frees the Rust allocation, and validates the
   generated return type.
8. An application error uses that function's generated error map; a panic
   becomes `RspytsPanicError`.

ctypes releases the GIL during the foreign call. The private slice copy is
necessary because another Python or native thread must not mutate aliased
memory while Rust is reading it.

## A WebAssembly call

The TypeScript path follows the same steps. The differences are mechanical:

- requests and slices are copied into WebAssembly linear memory;
- pointers are unsigned wasm32 values;
- handles cross as `bigint`;
- results are copied out before the Rust allocation is freed;
- a `WebAssembly.RuntimeError` permanently poisons that instance.

Once poisoned, an instance cannot be trusted to preserve Rust allocator or
object invariants. The runtime rejects later calls. Create a new instance; a
compiled `WebAssembly.Module` may still be reused.

## Generation

Every `#[bridge]` expansion submits a manifest record through `inventory`.
`rspyts::export!()` exposes the sorted manifest from the compiled module.

`rspyts generate` uses Cargo's JSON messages to find the host cdylib, loads it,
checks the ABI, reads the manifest, validates the whole surface, and renders
enabled outputs. It never parses Rust source.

Generated output is deterministic: no timestamps, stable ordering, a manifest
hash in every file, and writes only when content changes. `rspyts check` runs
the same pipeline without writing.

## Invariants

- Rust unwinding never crosses a native C ABI boundary.
- Requests belong to the caller; responses belong to the caller after return.
- Every allocation is freed with its exact original length.
- Attachment offsets are bounds-checked and naturally aligned.
- Structured floats are finite; exact 64-bit integers use canonical strings.
- Handles are nonzero, never reused, and smaller than 2^53.
- Unknown object fields are rejected by Rust and generated Python models.
- Generated error dispatch is call-scoped, so packages may reuse error codes.
- `wasm32-unknown-unknown` is the only supported WebAssembly pointer model.

The normative details are in [abi.md](design/abi.md),
[type-system.md](design/type-system.md), and
[codegen.md](design/codegen.md).
