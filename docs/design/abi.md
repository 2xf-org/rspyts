# ABI

This document defines ABI 3.0. It is the contract between a bridged Rust
module and every rspyts runtime.

The ABI is deliberately small. A host allocates one request, calls one symbol,
copies one response, and frees both allocations. Structured values use JSON;
large or opaque binary values use the envelope tail.

## Versioning

Every module exports:

```c
uint32_t rspyts_abi_version(void);
```

ABI 3 modules return `3`. The manifest records the exact string `"3.0"`.
The CLI and runtimes accept that exact contract and reject every other major
or minor. There is no ABI-2 compatibility path in the 0.3 runtime.

Runtimes reject a different C ABI major before calling any generated symbol.
A change that alters symbol signatures, ownership, or envelope interpretation
requires a new major. A backwards-compatible manifest vocabulary extension
increments the minor, and each CLI must explicitly declare the contiguous
minor range it understands.

Only `wasm32-unknown-unknown` is supported for WebAssembly. Its pointers and
lengths are unsigned 32-bit values. Native pointer and length values use the
platform's `usize` width.

## Module exports

`rspyts::export!()` adds five module-level symbols:

```c
uint32_t rspyts_abi_version(void);
uint8_t *rspyts_manifest(void);
uint8_t *rspyts_contract_fingerprint(void);
uint8_t *rspyts_alloc(size_t len);
void rspyts_free(uint8_t *ptr, size_t len);
```

`rspyts_manifest` returns a normal success envelope. Its JSON value is the
compiled manifest and its tail is empty.

`rspyts_contract_fingerprint` returns a success envelope containing the
lowercase SHA-256 digest of the exact compact manifest JSON. Generated clients
embed the fingerprint seen during generation and verify it when loading a
module, before any user symbol is called.

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
wire name. Every such key is required, including a parameter whose value type
is `Option<T>`; it may be present with null. A zero request length is accepted
as an empty object for no-argument calls. Every non-empty request uses the
envelope below.

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

The object is protocol metadata only where the declared Rust type is `Bytes`
or `Buf<T>`. Envelope decoding by itself does not scan arbitrary JSON objects
for this key. In particular, `Map<String, T>` permits every string key,
including `__rspyts_buf__` and `__rspyts_json__`; generated clients interpret
placeholders only while decoding a schema position declared as an attachment.

Opaque bytes use the `bytes` dtype. `Buf<u8>` uses `u8`; the distinction is
preserved by the declared type even though TypeScript exposes both as
`Uint8Array`.

Top-level `&[T]` parameters do not use placeholders. They use the separate
pointer and element-count pairs in the call signature. The host runtime keeps
a private copy alive for the call so concurrent mutation cannot race Rust.

## Schema-directed values

`serde_json::Value` is schemaless and crosses transparently, without a JSON
sentinel. The typed decoder stops traversal at a schema position declared as
`Json`, so objects inside it are ordinary user data even when they exactly
resemble an attachment placeholder. The same keys in a declared map are also
ordinary map data. Attachment objects gain protocol meaning only at a schema
position declared as `Bytes` or `Buf<T>`.

Exact `i64` and `u64` values are canonical decimal strings on the wire. They
have no plus sign or leading zeroes; signed zero is `"0"`. Native Rust Serde
still sees ordinary numeric values outside the bridge. Structured `f32` and
`f64` values must be finite. Schemaless JSON additionally restricts integral
numbers to JavaScript's exact safe range; typed exact integers are the path for
larger values. Schemaless JSON also canonicalizes signed zero to positive zero.
Binary float attachments preserve their IEEE bytes.

The Rust shim performs this conversion from the declared IR, recursively
through structs, newtypes, tagged enums, options, lists, maps, and tuples. It
does not scan arbitrary object shapes and it does not rebuild the manifest
during a call. Constants and typed application-error data use the same
normalizer.

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

Declarations contain docs, exact wire names, portable types, field-presence
requirements, origin crate, target projection, and error association. Unknown
manifest fields are rejected. The CLI validates the complete surface before
writing any output.

## Required invariants

- Requests and responses are validated before typed decoding.
- Every allocation is freed with its original pointer and exact length.
- Attachment arithmetic is checked before memory is read.
- Structured values never borrow foreign memory after a call.
- Rust unwinding never crosses native FFI.
- A trapped WebAssembly instance is never called again.
- Handles are nonzero, bounded, and never reused.
- Generated output is derived from the compiled manifest, not source parsing.
- Generated clients verify the compiled module's contract fingerprint before
  calling user symbols.
