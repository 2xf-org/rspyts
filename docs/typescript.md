# TypeScript

`rspyts generate` writes a strict TypeScript client for your Rust WebAssembly
module.

The npm runtime supports Node.js 22.12 or newer, with CI on Node.js 22.12+ and
Node 24. Browser support uses standard WebAssembly APIs.

## Load the module

`instantiate` accepts a `WebAssembly.Module`, a `BufferSource`, a `Response`,
or a promise of a `Response`. It verifies ABI version 3 and returns a
`BridgeModule`.

In Node:

```ts
import { readFile } from "node:fs/promises";
import { instantiate } from "rspyts";

const mod = await instantiate(await readFile(wasmPath));
```

In a browser or bundler:

```ts
const mod = await instantiate(fetch(wasmUrl));
```

Streaming compilation needs `Content-Type: application/wasm`.

## Use the generated client

Bind the generated API to that module instance:

```ts
import { createClient } from "./generated/index.js";

const client = createClient(mod);
const summary = client.summarize(new Float64Array([2, 4, 6]), "demo");
console.log(summary.average);
```

Each client belongs to one WebAssembly instance. Instances have separate
linear memory, allocators, and handle stores.

Generated files contain interfaces, literal unions, constants, error classes,
the client, and public re-exports. The generated `codecs.ts` module is private
implementation detail and is not exported from `index.ts`. A self-contained
projection depends only on `rspyts`; configured `[typescript.imports]` mappings
also import declarations from those generated dependency packages.

Generated clients call the ABI-3 runtime through the single `callFn` path from
`rspyts/internal/abi3`. Each response is represented internally as an explicit
`WireResponse` carrying its decoded value and attachment tail; decoders consume
that context rather than a bare wire value. `createClient` verifies the module's
contract fingerprint before exposing any generated operation.

## Types

- Structs become interfaces.
- String enums become string-literal unions.
- Tagged Rust enums become discriminated unions.
- `Option<T>` becomes `T | null`; field-presence metadata independently
  determines whether the object key may be omitted.
- Rust unit data becomes `null`, while a top-level unit return becomes `void`.
- Exact Serde wire names are preserved; non-identifiers become quoted keys.
- Unknown input fields are rejected when Rust deserializes the request.

Structured `f32` and `f64` values must be finite. Generated clients validate
returned structured floats recursively and normalize negative zero. Numeric
buffers retain their raw IEEE values, including NaN and infinities.

## Exact integers

Native Rust `i64` and `u64` become `bigint`. Generated code checks
the full range and converts through canonical decimal strings on the wire:

```ts
const pair = client.echoExactPair([-(1n << 63n), (1n << 64n) - 1n]);
```

This works recursively through every supported composite, constant, and typed
error payload, including 64-bit typed-array elements.

## Schemaless JSON

Rust `serde_json::Value` becomes `unknown`. Narrow or validate it before use.
Values must still be valid JSON with finite numbers.

The value crosses transparently as ordinary JSON. Generated schema-directed
codecs stop traversal at that declared JSON position, so nested objects,
including objects shaped exactly like attachment metadata, remain ordinary user
data. Applications see the original value without a protocol wrapper.

## Bytes and typed arrays

| Rust | TypeScript |
|---|---|
| `Bytes` | `Uint8Array` |
| `Buf<u8>` | `Uint8Array` marked by generated code as numeric `u8` |
| `Buf<T>` | matching typed array |
| `&[T]` parameter | matching typed array |
| `Vec<T>` | `T[]` |

The numeric dtypes are `u8`, `i8`, `u16`, `i16`, `u32`, `i32`, `u64`,
`i64`, `f32`, and `f64`. The 64-bit typed arrays use bigint elements.

The runtime encodes every element little-endian, copies input into aligned
linear memory, and frees the allocation after the call. Returned arrays are
copied out before Rust memory is freed, so they never alias WebAssembly memory
or the input.

## Errors

Error envelopes throw generated subclasses of `RspytsError`:

```ts
try {
  client.summarize(samples, null);
} catch (error) {
  if (error instanceof BasicErrorEmptyInput) {
    console.log(error.code, error.data);
  } else {
    throw error;
  }
}
```

Generated clients pass one call-scoped error map per declared Rust error enum.
Two packages may reuse a code without changing one another's `instanceof`
behavior. Error dispatch has no global registry or cross-package state.

## Handles and disposal

Rust classes stay behind `bigint` handles. Dispose them deterministically:

```ts
{
  using counter = new client.Counter(10, "demo");
  counter.increment(5);
}
```

Or use `try/finally` and `counter.free()`. Both `free()` and
`Symbol.dispose` are idempotent. `FinalizationRegistry` is a leak backstop,
not a replacement for disposal. Generated clients also load in runtimes that
do not provide `FinalizationRegistry`; in that case the backstop is a no-op and
explicit disposal remains the ownership mechanism.

## Traps and poisoned instances

Ordinary Rust application errors use envelopes and leave the instance healthy.
A panic on `wasm32-unknown-unknown` usually aborts and becomes a
`WebAssembly.RuntimeError`. The runtime then marks that instance poisoned,
skips unsafe cleanup, and makes later calls throw `InstancePoisonedError`.

Create a fresh instance. You may reuse a compiled `WebAssembly.Module`.

## Long-running work

Generated calls are synchronous. In a browser, put CPU-heavy bridge work in a
Web Worker and transfer large result buffers when your application no longer
needs the sender's view.

## Shipping

Build release WebAssembly with
`rspyts build --release --target wasm32-unknown-unknown --out-dir <dir>`.
Keep the `.wasm` file as an asset and instantiate it once per desired state domain.
Commit generated TypeScript and run `rspyts check` in CI.

See the [quickstart](introduction/quickstart.md) and the complete
[type system](design/type-system.md).
