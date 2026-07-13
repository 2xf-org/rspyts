# The rspyts ABI

This document is the normative specification of the binary boundary between a
Rust cdylib/WASM module produced with rspyts and the Python/TypeScript
runtimes that call into it. Every component — the proc macros, the generated
shims, the Python runtime, and the TypeScript runtime — implements exactly
this contract. If code and this document disagree, the code is wrong.

Version: `0.1`. The manifest carries this version; runtimes must reject
modules with an unknown major version.

## 1. Design constraints

- The boundary is a plain C ABI so that CPython (`ctypes`) and WebAssembly
  hosts can both consume it with no per-language native integration.
- Everything that crosses the boundary is one of:
  1. **Serialized bytes** — UTF-8 JSON, for structured data.
  2. **Raw numeric buffers** — pointer + length, for bulk numeric data.
  3. **Opaque handles** — `u64` identifiers for stateful Rust objects.
- No Rust references, lifetimes, or live objects ever cross the boundary.
- Panics never unwind across the boundary (undefined behavior); every
  exported function catches panics and reports them via the envelope.

## 2. Memory management primitives

Every rspyts module exports:

```c
uint8_t *rspyts_alloc(size_t len);          // allocate len bytes, alignment 1
void     rspyts_free(uint8_t *ptr, size_t len);
```

Rules:

- Buffers allocated by `rspyts_alloc` are owned by the caller and must be
  freed with `rspyts_free` using the exact same `len`.
- Buffers returned by bridged functions (envelopes, see §4) are allocated so
  that **capacity == length**; the caller frees them with
  `rspyts_free(ptr, total_len)` where `total_len` is derived from the header.
- `rspyts_alloc(0)` returns a dangling non-null pointer; `rspyts_free` with
  `len == 0` is a no-op. Callers should avoid zero-length round trips.
- Callers write **request payloads** into memory obtained from
  `rspyts_alloc`, then pass (ptr, len) to a bridged function. The callee
  (Rust) reads the request but does NOT free it — the caller frees its own
  request buffer after the call returns. Rationale: on WASM the caller must
  allocate inside linear memory anyway; keeping ownership caller-side makes
  native and WASM identical.

## 3. Exported symbols

For a crate bridged with rspyts, the module exports (all `extern "C"`):

| Symbol | Signature | Purpose |
|---|---|---|
| `rspyts_abi_version` | `() -> u32` | Returns `1`. Checked before anything else. |
| `rspyts_manifest` | `() -> *mut u8` | Returns an envelope (§4) whose JSON payload is the manifest (§7). |
| `rspyts_alloc` | `(usize) -> *mut u8` | §2 |
| `rspyts_free` | `(*mut u8, usize) -> ()` | §2 |
| `rspyts_fn__{name}` | `(args…) -> *mut u8` | One per bridged free function. |
| `rspyts_cls__{Type}__new` | `(args…) -> *mut u8` | Constructor (only when the class has one); envelope JSON payload is the handle (number). |
| `rspyts_cls__{Type}__{name}` | `(args…) -> *mut u8` | One per `#[bridge(static)]` method — **no handle parameter**. A factory static (manifest `returnsSelf`, §7) inserts the instance into the slab and its envelope payload is the fresh handle, exactly like the constructor's. |
| `rspyts_cls__{Type}__{method}` | `(u64 handle, args…) -> *mut u8` | One per bridged method. |
| `rspyts_cls__{Type}__drop` | `(u64 handle) -> ()` | Destroys the object. Idempotent: unknown handles are ignored. |

`{name}`/`{method}` are the Rust snake_case identifiers; `{Type}` is the Rust
type name. Statics and methods share the `rspyts_cls__{Type}__{name}` symbol
form and one name space per class; `new` and `drop` are reserved. The
`rspyts_abi_version`, `rspyts_manifest`, `rspyts_alloc` and
`rspyts_free` symbols are emitted once per module by `rspyts::export!()`.

**v0.1 limitation:** symbol names are not namespaced by crate. Several
bridged crates *can* link into one cdylib — every linked crate's
registrations land in the module's single manifest, with types carrying
their `origin` (§7) — but two crates that both define a **function or
class with the same name** collide at link time, because
`rspyts_fn__{name}` and `rspyts_cls__{Type}__…` are derived from the bare
names. Identically named types and constants have no symbols to collide,
but manifest assembly rejects duplicate names within any section, so that
clash surfaces at `rspyts generate` time. (Roadmap: crate-prefixed
symbols behind a config key.)

### 3.1 Argument passing

A bridged Rust function's parameters are split into three groups, and the C
signature is derived deterministically:

1. **Handle** (methods only): `u64`, always the first C parameter.
2. **Slice parameters** (`&[T]` for supported dtypes, §6): each becomes a
   `(const T *ptr, usize len)` pair, appearing in declaration order **after**
   the args payload.
3. **Everything else** ("plain" parameters): collected into a single JSON
   object keyed by the parameter's camelCase name, passed as
   `(const u8 *args_ptr, usize args_len)`.

C parameter order: `[handle,] args_ptr, args_len[, s1_ptr, s1_len, s2_ptr, s2_len, …]`.

The `(args_ptr, args_len)` pair is always present, even when there are no
plain parameters; callers pass the two-byte JSON payload `{}` (or any empty
object). This keeps signatures uniform and future-proof.

Every bridged function returns `*mut u8` — a response envelope. `__drop`
returns nothing.

## 4. The response envelope

A single allocation with a 12-byte little-endian header:

```
offset  size  field
0       1     status     (0 = ok, 1 = error, 2 = panic)
1       3     reserved   (zero)
4       4     json_len   (u32 LE)
8       4     tail_len   (u32 LE)
12      json_len        UTF-8 JSON payload
12+json_len  tail_len   raw tail (§6)
```

- Total allocation length = `12 + json_len + tail_len`; free with
  `rspyts_free(ptr, total_len)`.
- `status == 0`: JSON payload is the serialized return value (the `T` in
  `Result<T, E>`, or the plain return type). Functions returning `()`
  serialize `null`. Non-finite floats (NaN, ±Inf) in JSON positions
  serialize as `null` (serde_json behavior); return them through
  `Buf`/slices (§6) instead.
- `status == 1`: JSON payload is a bridge error object (§5) produced from the
  function's `Err` value.
- `status == 2`: JSON payload is a bridge error object with code `"panic"`;
  the tail is empty. Runtimes raise their language's internal-error type.

## 5. Errors

The bridge error object:

```json
{ "code": "invalidParams", "message": "human readable", "data": { } }
```

- `code`: machine-readable string, camelCase. Required.
- `message`: human-readable. Required.
- `data`: arbitrary JSON or omitted.

Well-known codes reserved by the bridge itself: `"panic"` (status 2, §4),
`"invalidArgs"` (the args payload failed to deserialize), `"staleHandle"`
(§8), and `"payloadTooLarge"` (the result exceeded the 4 GiB envelope
limits, §4).

Rust error types opt in by implementing `rspyts::BridgeErr` (a conversion to
`rspyts_core::BridgeError`). `#[bridge(error)]` on a fieldless or
struct-variant enum error implements it automatically: the variant name in
camelCase becomes `code`, the `Display` string becomes `message`, and named
fields become `data`.

Runtimes surface `status == 1` as a typed exception/error carrying `code`,
`message`, `data`; `status == 2` as `RspytsPanicError` (Python) /
`RspytsPanicError` (TS).

## 6. Raw numeric buffers

Supported dtypes: `u8`, `i16`, `i32`, `f32`, `f64` (wire names exactly
these strings).

**Input slices** (`&[T]`): passed as `(ptr, len)` C arguments (§3.1);
zero-copy on the Rust side. `len` is the element count, not bytes. The
caller guarantees validity for the duration of the call; alignment must be
natural for `T` (guaranteed on WASM by `rspyts_alloc`? — no: `rspyts_alloc`
is alignment-1, so runtimes MUST allocate slice inputs at natural alignment;
both runtimes over-allocate and align internally).

**Output buffers** (`rspyts::Buf<T>`): an owned numeric vector returned (or
nested inside a returned struct). During result serialization the elements
are appended to the envelope's raw tail (aligned to `align_of::<T>()` by
zero-padding) and the JSON position receives a placeholder object:

```json
{ "__rspyts_buf__": { "off": 128, "len": 4096, "dt": "f64" } }
```

`off` is the byte offset **into the tail**, `len` the element count. Decoders
replace placeholders with native arrays: `numpy.ndarray` (Python, copied out
of the envelope) and `Float64Array`/`Float32Array`/`Uint8Array`/`Int16Array`/
`Int32Array` (TypeScript, copied out of WASM memory — views are never
retained because linear memory may grow/move).

`Buf<T>` may appear anywhere in a return type. It must NOT appear in
parameters (use `&[T]`); the macro rejects it.

**Known limitation (v0.1):** the `__rspyts_buf__` placeholder key is not
unforgeable. A user-returned map value that is itself an object with the
single key `"__rspyts_buf__"` is indistinguishable from a placeholder and
will be misdecoded by the runtimes. Do not use that key in string-keyed
map data.

## 7. The manifest

`rspyts_manifest()` returns an envelope whose JSON payload describes the
module — every bridged type, constant, function, and class, with docs.
Field names are camelCase. Shape (see `rspyts-core/src/ir.rs` for the
authoritative serde types):

```json
{
  "abi": "0.1",
  "crateName": "basic-example",
  "crateVersion": "0.1.0",
  "types": [ { "kind": "errorEnum", "name": "BasicError", "docs": "…",
               "origin": "basic-example",
               "variants": [ { "name": "EmptyInput", "wireCode": "emptyInput",
                               "docs": "…", "fields": [] },
                             { "name": "ZeroFactor", "wireCode": "zeroFactor",
                               "docs": "…", "fields": [ … ] } ] },
             { "kind": "enum", "name": "ParsedNumber", "docs": "…",
               "origin": "basic-example", "tag": "type",
               "variants": [ { "name": "Integer", "wireName": "integer", "docs": "…",
                               "fields": [ … ] } ] },
             { "kind": "stringEnum", "name": "Rounding", "docs": "…",
               "origin": "basic-example",
               "variants": [ { "name": "Up", "wireName": "up", "docs": "…" } ] },
             { "kind": "struct", "name": "Summary", "docs": "…",
               "origin": "basic-example",
               "fields": [ { "name": "item_count", "wireName": "itemCount",
                             "ty": {"kind": "u32"}, "docs": "…", "optional": false } ] } ],
  "constants": [ { "name": "DEFAULT_FACTOR", "docs": "…",
                   "origin": "basic-example",
                   "ty": {"kind": "f64"}, "value": 2.0 } ],
  "functions": [ { "name": "summarize", "docs": "…",
                   "params": [ { "name": "values", "wireName": "values",
                                 "ty": {"kind": "slice", "dt": "f64"} },
                               { "name": "label", "wireName": "label",
                                 "ty": {"kind": "option", "inner": {"kind": "string"}} } ],
                   "ret": {"kind": "ref", "name": "Summary"},
                   "err": "BasicError",
                   "targets": ["python", "typescript"] } ],
  "classes": [ { "name": "Counter", "docs": "…",
                 "constructor": { "docs": "…", "params": [ … ], "err": null },
                 "methods": [ { "name": "increment", "docs": "…", "mutable": true,
                                "params": [ … ], "ret": {"kind": "i32"}, "err": null,
                                "targets": ["python", "typescript"] } ],
                 "statics": [ { "name": "with_label", "docs": "…",
                                "params": [ … ], "ret": {"kind": "unit"},
                                "err": null, "returnsSelf": true,
                                "targets": ["python", "typescript"] } ] } ]
}
```

- `err` on functions, constructors, methods, and statics is the name of an
  `errorEnum` entry in `types`, or `null` when infallible. Every param
  carries both its Rust `name` and its `wireName` (the key in the args JSON
  object; unused for slice params).
- `origin` on types and constants is the defining crate's name. It differs
  from `crateName` for items registered from dependency crates linked into
  the module; emitters use it to import instead of re-emit
  (codegen.md §9).
- `targets` lists the projections the item appears in (`"python"`,
  `"typescript"`; both by default). The shim always exists — emitters
  filter.
- `constructor` is `null` for factory-only classes. `returnsSelf` marks a
  factory static: its `ret` is ignored and its envelope carries a fresh
  handle (§3).
- A constant's `value` is the fully serialized wire form of its Rust
  value, captured inside the compiled module.

The CLI obtains the manifest by building the crate as a **host** cdylib,
`dlopen`ing it, and calling `rspyts_manifest` — never by parsing Rust source.

Manifest ordering is deterministic: types, constants, functions, classes
each sorted lexicographically by name (methods and statics keep
declaration order within their class).

## 8. Handles

- Handles are `u64`, allocated from a per-type slab starting at 1;
  `0` is never a valid handle.
- Handle values stay below 2^53 in practice (per-type monotonic counter), so
  they are representable as JSON numbers and JS `number`s. The WASM boundary
  passes them as `u64` (JS `BigInt`); the TS runtime converts.
- `__drop` is idempotent and infallible. Runtimes call it from destructors
  (`__del__` + context manager in Python; `Symbol.dispose` + explicit
  `free()` + `FinalizationRegistry` best-effort in TS).
- Using a dropped handle yields a `status == 1` error with code
  `"staleHandle"`.
- Methods lock the object for the duration of the call (`Mutex`); concurrent
  calls on the same handle serialize. Reentrancy is impossible (no callbacks
  in v0.1).

## 9. Panics and safety

- Every exported shim wraps the user function in
  `std::panic::catch_unwind(AssertUnwindSafe(…))`.
- A caught panic produces a `status == 2` envelope whose `message` is the
  panic payload if it was a `&str`/`String`, else `"panic (non-string payload)"`.
- If envelope *encoding itself* fails (OOM-class), the shim aborts the
  process rather than returning garbage.

## 10. Threading

- Native: bridged functions are callable from any thread. Handles are
  internally synchronized. `ctypes` releases the GIL during calls, so
  long-running Rust work does not block other Python threads.
- WASM: single-threaded; no additional constraints.

## 11. Explicitly out of scope for ABI 0.1

Callbacks (foreign code invoked from Rust), async functions, streaming,
`u64`/`i64`/`u128`/`i128`/`usize`/`isize` in JSON positions, borrowed return
values, crate-namespaced symbols (§3: linked bridged crates must not both
define identically-named functions or classes). Each is a deliberate
omission; see `docs/design/decisions.md`.
