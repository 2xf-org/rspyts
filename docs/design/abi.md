# ABI

This document defines ABI 2.0. It is the contract between a bridged Rust
module and every rspyts runtime.

The ABI is deliberately small. A host allocates one request, calls one symbol,
copies one response, and frees both allocations. Structured values use JSON;
large or opaque binary values use the envelope tail.

## Versioning

Every module exports:

```c
uint32_t rspyts_abi_version(void);
```

ABI 2 modules return `2`. The manifest records the full string `"2.0"`.
Runtimes reject a different major before calling any generated symbol. A
change that alters symbol signatures, ownership, or envelope interpretation
requires a new major.

Only `wasm32-unknown-unknown` is supported for WebAssembly. Its pointers and
lengths are unsigned 32-bit values. Native pointer and length values use the
platform's `usize` width.

## Module exports

`rspyts::export!()` adds four module-level symbols:

```c
uint32_t rspyts_abi_version(void);
uint8_t *rspyts_manifest(void);
uint8_t *rspyts_alloc(size_t len);
void rspyts_free(uint8_t *ptr, size_t len);
```

`rspyts_manifest` returns a normal success envelope. Its JSON value is the
compiled manifest and its tail is empty.

Free functions use their Rust name:

```text
rspyts_fn__{function}
```

Classes use their Rust type and member names:

```text
rspyts_cls__{Class}__new
rspyts_cls__{Class}__{method}
rspyts_cls__{Class}__{static}
rspyts_cls__{Class}__drop
```

Renaming in Serde changes wire fields and variants, not exported symbols.

## Call signatures

A free function, constructor, or static receives one request pointer and
length, followed by one pointer and element count for each top-level numeric
slice parameter:

```c
uint8_t *call(
    const uint8_t *request_ptr,
    size_t request_len,
    const T0 *slice0_ptr,
    size_t slice0_len,
    ...
);
```

An instance method receives its handle first. Slice pairs remain in source
parameter order:

```c
uint8_t *method(
    uint64_t handle,
    const uint8_t *request_ptr,
    size_t request_len,
    ...
);
```

Drop takes only the handle and returns nothing. It is idempotent.

The request JSON is an object containing every non-slice argument under its
wire name. A zero request length is accepted as an empty object for no-argument
calls. Every non-empty request uses the envelope below.

## Ownership

The caller owns request and slice memory. It must keep both valid until the
call returns, then free the request with its original pointer and length.

The module owns a returned pointer until it hands that pointer to the caller.
The caller reads the header, computes the exact total length, copies anything
it needs, and calls `rspyts_free(ptr, total_len)` once.

`rspyts_alloc(0)` may return a non-null dangling pointer. It must not be
dereferenced. `rspyts_free` is a no-op for a zero length or null pointer.

No returned JSON view, typed array, or object may borrow module memory. The
Python and TypeScript runtimes copy responses before freeing them.

## Envelope

Requests and responses share one physical layout:

```text
offset  size        field
0       1           marker or status
1       3           reserved, all zero
4       4           JSON byte length, little-endian u32
8       4           tail byte length, little-endian u32
12      json_len    UTF-8 JSON
12+j    tail_len    binary attachment tail
```

The total length is exactly `12 + json_len + tail_len`. Trailing bytes,
truncation, nonzero reserved bytes, malformed UTF-8 JSON, and overflowing
length arithmetic are protocol errors.

For requests, byte zero is a marker and must be `0`. For responses it is a
closed status value:

| Status | Meaning | Tail |
|---:|---|---|
| `0` | Success | Allowed |
| `1` | Application or protocol error | Empty |
| `2` | Caught Rust panic | Empty |

Any other response status is invalid.

## Binary attachments

`Bytes` and `Buf<T>` values inside owned data are represented in JSON by an
exact placeholder object:

```json
{
  "__rspyts_buf__": {
    "off": 16,
    "len": 4,
    "dt": "f32"
  }
}
```

`off` is a byte offset into the tail. `len` is an element count. `dt` is one
of:

```text
u8 i8 u16 i16 u32 i32 u64 i64 f32 f64 bytes
```

The placeholder has no sibling fields. The attachment range must stay inside
the tail, and `off` must satisfy the element type's natural alignment. Numeric
elements are little-endian. Padding between attachments is zero-filled and is
not part of an attachment.

Opaque bytes use the `bytes` dtype. `Buf<u8>` uses `u8`; the distinction is
preserved by the declared type even though TypeScript exposes both as
`Uint8Array`.

Top-level `&[T]` parameters do not use placeholders. They use the separate
pointer and element-count pairs in the call signature. The host runtime keeps
a private copy alive for the call so concurrent mutation cannot race Rust.

## Reserved JSON wrappers

`rspyts::Json` is schemaless, but arbitrary user JSON must not collide with a
buffer placeholder. Its wire value is therefore wrapped exactly once:

```json
{"__rspyts_json__": <value>}
```

The wrapper has no sibling fields. Generated clients add and remove it at the
typed boundary.

Exact `I64` and `U64` values are canonical decimal strings. They have no plus
sign or leading zeroes; signed zero is `"0"`. Structured `f32` and `f64`
values must be finite. Binary float attachments preserve their IEEE bytes.

## Errors and panics

Status `1` JSON has this shape:

```json
{
  "code": "tooManyValues",
  "message": "too many values",
  "data": {"count": 2048}
}
```

`data` is optional. Generated clients select exception classes from the error
enum declared by the current function or method, so identical codes in two
unrelated APIs do not collide.

Status `2` uses code `panic` and a message. Rust catches unwinding around the
user call, result serialization, and result destruction. Native callers
receive a panic envelope. On WebAssembly builds whose panic strategy aborts, a
panic traps instead; the TypeScript runtime poisons that instance and rejects
later calls.

No Rust unwind may cross a native `extern "C"` boundary. Drop shims swallow a
panic because they cannot return an envelope.

## Handles

A class instance lives in a Rust slab. Constructors and factories return a
nonzero integer handle. Handles are never reused and remain below `2^53`, so
the same value is exact in JavaScript `number`, TypeScript `bigint`, Python
`int`, and Rust `u64` representations.

Every method lookup is synchronized. A missing or already-dropped handle
returns `staleHandle`. Drop is safe to repeat. Host finalizers are only a
backstop; callers should close Python objects or call `free()`/use explicit
resource management in TypeScript.

## Manifest

The manifest is canonical JSON containing:

- `abi`, `crateName`, and `crateVersion`;
- sorted bridged types and errors;
- sorted constants and their captured values;
- sorted functions;
- sorted classes, constructors, methods, and statics.

Declarations contain docs, exact wire names, portable types, origin crate,
target projection, and error association. Unknown manifest fields are
rejected. The CLI validates the complete surface before writing any output.

## Required invariants

- Requests and responses are validated before typed decoding.
- Every allocation is freed with its original pointer and exact length.
- Attachment arithmetic is checked before memory is read.
- Structured values never borrow foreign memory after a call.
- Rust unwinding never crosses native FFI.
- A trapped WebAssembly instance is never called again.
- Handles are nonzero, bounded, and never reused.
- Generated output is derived from the compiled manifest, not source parsing.
